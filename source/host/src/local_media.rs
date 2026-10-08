//! Five-second local-volume reconciliation. OS mounting and file access grants
//! belong to platform adapters; validation and catalog registration are shared.
use crate::{library, onelibrary};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::{Duration, SystemTime},
};
const MAX_USB: usize = 3;
static ROOTS: LazyLock<Mutex<BTreeMap<String, PathBuf>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static STATUS: Mutex<Value> = Mutex::new(Value::Null);
static NAMES: LazyLock<Mutex<BTreeMap<String, String>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
pub fn status() -> Value {
    let mut value = STATUS.lock().unwrap().clone();
    if value.is_object() {
        value["serving"] = crate::local_serving::status();
    }
    value
}
#[cfg(target_os = "ios")]
pub fn register(id: &str, path: &Path, label: &str) -> Result<(), String> {
    if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err("Invalid USB identity".into());
    }
    let path = path
        .canonicalize()
        .map_err(|e| format!("Cannot access selected USB folder: {e}"))?;
    if !path.is_dir() {
        return Err("Choose the USB's root folder".into());
    }
    let mut roots = ROOTS.lock().map_err(|e| e.to_string())?;
    if roots.iter().any(|(key, p)| p == &path && key != id) {
        return Err("This USB folder is already registered".into());
    }
    if !roots.contains_key(id) && roots.len() >= MAX_USB {
        return Err(
            "Up to three local USBs are supported. Forget one before adding another.".into(),
        );
    }
    roots.insert(id.into(), path);
    NAMES
        .lock()
        .unwrap()
        .insert(id.into(), label.trim().to_owned());
    Ok(())
}
#[cfg(target_os = "ios")]
pub fn forget(id: &str) {
    ROOTS.lock().unwrap().remove(id);
    NAMES.lock().unwrap().remove(id);
}
pub fn reset() {
    ROOTS.lock().unwrap().clear();
    NAMES.lock().unwrap().clear();
    *STATUS.lock().unwrap() = Value::Null;
}
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    device: u64,
    inode: u64,
    wal: Option<(u64, Option<SystemTime>)>,
}
fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let m = path.metadata().ok()?;
    Some(Stamp {
        len: m.len(),
        modified: m.modified().ok(),
        device: m.dev(),
        inode: m.ino(),
        wal: path
            .with_file_name("exportLibrary.db-wal")
            .metadata()
            .ok()
            .map(|m| (m.len(), m.modified().ok())),
    })
}
struct Cached {
    stamp: Stamp,
    valid: bool,
    error: Option<String>,
    checked: std::time::Instant,
}
#[derive(Default)]
struct Scanner {
    cache: BTreeMap<String, Cached>,
    admitted: BTreeSet<String>,
}
impl Scanner {
    fn scan(&mut self, shared: &library::Shared, mut roots: Vec<(String, PathBuf)>) -> Value {
        roots.sort_by_key(|(id, _)| (!self.admitted.contains(id), id.clone()));
        let mut seen = BTreeSet::new();
        let mut ready = vec![];
        let mut volumes = vec![];
        for (id, root) in roots {
            if !seen.insert(id.clone()) {
                continue;
            }
            let label = NAMES
                .lock()
                .unwrap()
                .get(&id)
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| {
                    root.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
            let database = root.join(onelibrary::DATABASE);
            let Some(current) = stamp(&database) else {
                self.cache.remove(&id);
                volumes.push(json!({"id":id,"label":label,"state":if root.is_dir(){"No OneLibrary library"}else{"Disconnected"}}));
                continue;
            };
            // Do not let a database symlink escape the explicitly granted root.
            let safe = database
                .canonicalize()
                .ok()
                .zip(root.canonicalize().ok())
                .is_some_and(|(db, root)| db.starts_with(root));
            if !safe {
                volumes.push(
                    json!({"id":id,"label":label,"state":"Database is outside the selected USB"}),
                );
                continue;
            }
            if ready.len() == MAX_USB {
                volumes
                    .push(json!({"id":id,"label":label,"state":"Three local USBs already active"}));
                continue;
            }
            let changed = self.cache.get(&id).is_none_or(|c| {
                c.stamp != current
                    || (c.valid && !self.admitted.contains(&id))
                    || (!c.valid && c.checked.elapsed() >= Duration::from_secs(30))
            });
            let mut catalog = None;
            if changed {
                match onelibrary::read(&database) {
                    Ok((library, tags, fingerprint)) => {
                        catalog = Some(library::local_catalog(library, tags, fingerprint));
                        self.cache.insert(
                            id.clone(),
                            Cached {
                                stamp: current,
                                valid: true,
                                error: None,
                                checked: std::time::Instant::now(),
                            },
                        );
                    }
                    Err(error) => {
                        self.cache.insert(
                            id.clone(),
                            Cached {
                                stamp: current,
                                valid: false,
                                error: Some(error),
                                checked: std::time::Instant::now(),
                            },
                        );
                    }
                }
            }
            let valid = self.cache.get(&id).is_some_and(|c| c.valid);
            if valid {
                ready.push((
                    format!("local-usb:{id}"),
                    format!("LOCAL USB · {label}"),
                    database,
                    catalog,
                ));
            }
            volumes.push(json!({"id":id,"label":label,"state":if valid {"Ready"}else{"Invalid OneLibrary"},"error":self.cache.get(&id).and_then(|c|c.error.clone())}));
        }
        self.cache.retain(|key, _| seen.contains(key));
        self.admitted = ready
            .iter()
            .map(|(id, _, _, _)| id.trim_start_matches("local-usb:").to_owned())
            .collect();
        let count = ready.len();
        library::sync_local(shared, ready);
        json!({"limit":MAX_USB,"count":count,"intervalSeconds":5,"needsFolderAccess":cfg!(target_os="ios"),"volumes":volumes})
    }
}
pub fn start(shared: library::Shared) {
    let weak = std::sync::Arc::downgrade(&shared);
    tokio::spawn(async move {
        let mut scanner = Scanner::default();
        loop {
            let Some(shared) = weak.upgrade() else {
                break;
            };
            let result = tokio::task::spawn_blocking(move || {
                let result = scanner.scan(&shared, platform_roots());
                (scanner, result)
            })
            .await;
            match result {
                Ok((next, result)) => {
                    scanner = next;
                    *STATUS.lock().unwrap() = result;
                }
                Err(_) => break,
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}
#[cfg(target_os = "ios")]
fn platform_roots() -> Vec<(String, PathBuf)> {
    ROOTS
        .lock()
        .unwrap()
        .iter()
        .map(|(id, p)| (id.clone(), p.clone()))
        .collect()
}
#[cfg(target_os = "macos")]
fn platform_roots() -> Vec<(String, PathBuf)> {
    let mut roots = vec![];
    for entry in std::fs::read_dir("/Volumes")
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if path.is_symlink() || !path.is_dir() {
            continue;
        }
        // diskutil classifies the mounted volume, excluding internal/network disks.
        let Ok(output) = std::process::Command::new("/usr/sbin/diskutil")
            .args(["info"])
            .arg(&path)
            .env("LC_ALL", "C")
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let field = |key: &str| {
            text.lines().find_map(|line| {
                line.trim()
                    .split_once(':')
                    .filter(|(k, _)| *k == key)
                    .map(|(_, v)| v.trim().to_owned())
            })
        };
        if field("Device Location").as_deref() != Some("External") {
            continue;
        }
        let id = field("Volume UUID").unwrap_or_else(|| {
            format!(
                "{:?}",
                prolink_rekordbox::stable_digest(path.to_string_lossy().as_bytes())
            )
        });
        roots.push((id, path));
    }
    roots
}
#[cfg(target_os = "linux")]
fn platform_roots() -> Vec<(String, PathBuf)> {
    let mut roots = vec![];
    for line in std::fs::read_to_string("/proc/self/mountinfo")
        .unwrap_or_default()
        .lines()
    {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 7 {
            continue;
        }
        let device = PathBuf::from(format!("/sys/dev/block/{}", fields[2]));
        let Ok(sys) = device.canonicalize() else {
            continue;
        };
        let removable = sys
            .ancestors()
            .take(3)
            .any(|p| std::fs::read_to_string(p.join("removable")).is_ok_and(|v| v.trim() == "1"));
        if !removable && !sys.to_string_lossy().contains("/usb") {
            continue;
        }
        let path = PathBuf::from(
            fields[4]
                .replace("\\040", " ")
                .replace("\\011", "\t")
                .replace("\\134", "\\"),
        );
        let id = format!(
            "{:?}",
            prolink_rekordbox::stable_digest(
                format!("{}:{}", fields[2], path.display()).as_bytes()
            )
        );
        roots.push((id, path));
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onelibrary::tests::fixture;
    fn available(shared: &library::Shared) -> Vec<String> {
        library::sources(shared)["sources"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["available"] == true)
            .map(|s| s["id"].as_str().unwrap().into())
            .collect()
    }
    #[test]
    fn tracks_three_independent_libraries_without_reparsing_and_handles_reconnect() {
        let fixtures: Vec<_> = (0..4).map(|_| fixture()).collect();
        let roots: Vec<_> = fixtures
            .iter()
            .enumerate()
            .map(|(i, f)| (format!("usb{i}"), f.0.clone()))
            .collect();
        let shared = library::with_local_path(None, None);
        let mut scanner = Scanner::default();
        let result = scanner.scan(&shared, roots.clone());
        assert_eq!(result["count"], 3);
        assert_eq!(available(&shared).len(), 3);
        let generation_of = |id: &str| {
            library::sources(&shared)["sources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["id"] == id)
                .unwrap()["generation"]
                .as_u64()
                .unwrap()
        };
        let query = library::Query::from([(
            "generation".into(),
            generation_of("local-usb:usb0").to_string(),
        )]);
        let (generation, catalog) = library::catalog(&shared, "local-usb:usb0", &query).unwrap();
        assert_eq!(catalog.library.tracks.len(), 2);
        scanner.scan(&shared, roots.clone());
        assert_eq!(
            library::catalog(&shared, "local-usb:usb0", &query)
                .unwrap()
                .0,
            generation
        );
        scanner.scan(&shared, roots[1..].to_vec());
        assert!(!available(&shared).contains(&"local-usb:usb0".into()));
        assert!(available(&shared).contains(&"local-usb:usb3".into()));
        scanner.scan(&shared, vec![roots[0].clone(), roots[1].clone()]);
        let query = library::Query::from([(
            "generation".into(),
            generation_of("local-usb:usb0").to_string(),
        )]);
        assert!(
            library::catalog(&shared, "local-usb:usb0", &query)
                .unwrap()
                .0
                > generation
        );
        assert_eq!(available(&shared).len(), 2);
    }
    #[test]
    fn invalid_or_escaped_database_never_enters_selector() {
        let root = fixture();
        let shared = library::with_local_path(None, None);
        let mut scanner = Scanner::default();
        let roots = vec![("one".into(), root.0.clone())];
        scanner.scan(&shared, roots.clone());
        assert_eq!(available(&shared).len(), 1);
        std::fs::write(root.0.join(onelibrary::DATABASE), vec![0; 8192]).unwrap();
        scanner.scan(&shared, roots.clone());
        assert!(available(&shared).is_empty());
        let other = fixture();
        std::fs::remove_file(root.0.join(onelibrary::DATABASE)).unwrap();
        std::os::unix::fs::symlink(
            other.0.join(onelibrary::DATABASE),
            root.0.join(onelibrary::DATABASE),
        )
        .unwrap();
        scanner.scan(&shared, roots);
        assert!(available(&shared).is_empty());
    }
    #[tokio::test]
    async fn unchanged_scan_does_not_cancel_manual_refresh() {
        let fixture = fixture();
        let roots = vec![("one".into(), fixture.0.clone())];
        let shared = library::with_local_path(None, None);
        let mut scanner = Scanner::default();
        scanner.scan(&shared, roots.clone());
        library::refresh(&shared, "local-usb:one").unwrap();
        scanner.scan(&shared, roots);
        assert_eq!(library::sources(&shared)["sources"][0]["available"], true);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = library::sources(&shared);
                if state["sources"][0]["state"] == "ready" {
                    break;
                }
                assert_ne!(state["sources"][0]["state"], "error");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
}
