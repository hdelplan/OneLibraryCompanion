//! Read phrase-analysis mood on demand for Browse details.
use crate::library::{self, Location, Query, Shared};
use prolink::consume::nfs::NfsClient;
use serde_json::{Value, json};
use std::{
    path::{Component, Path},
    time::Duration,
};
const MAX_ANALYSIS: u64 = 16 * 1024 * 1024;
static READS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn phrase_path(path: &str) -> Result<String, String> {
    if !path.to_ascii_lowercase().starts_with("/pioneer/usbanlz/")
        || path.contains('\\')
        || !Path::new(path)
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err("No valid exported phrase-analysis path".into());
    }
    Ok(Path::new(path)
        .with_extension("EXT")
        .to_string_lossy()
        .into_owned())
}

pub async fn get(shared: &Shared, id: &str, track_id: u32, query: &Query) -> Result<Value, String> {
    let (generation, catalog) = library::catalog(shared, id, query)?;
    let track = catalog
        .library
        .tracks
        .get(&track_id)
        .ok_or("Track not in this catalog")?;
    let path = phrase_path(&track.analyze_path)?;
    let (location, interface) =
        library::artwork_location(shared, id, generation).ok_or("USB selection changed")?;
    let bytes = tokio::time::timeout(Duration::from_secs(12), async {
        let _permit = READS.acquire().await.map_err(|e| e.to_string())?;
        match location {
            Location::Local(database) => tokio::task::spawn_blocking(move || {
                use std::io::Read;
                let root = database
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::parent)
                    .ok_or("Invalid export location")?
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
                let file = root
                    .join(path.trim_start_matches('/'))
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
                if !file.starts_with(&root) {
                    return Err("Analysis is outside the export".into());
                }
                let mut bytes = Vec::new();
                std::fs::File::open(file)
                    .map_err(|e| e.to_string())?
                    .take(MAX_ANALYSIS + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() as u64 > MAX_ANALYSIS {
                    return Err("Analysis exceeds size limit".into());
                }
                Ok(bytes)
            })
            .await
            .map_err(|e| e.to_string())?,
            _ => {
                let (ip, slot, interface) = library::connection(&location, interface)?;
                let mut client = NfsClient::connect(ip, interface.as_ref())
                    .await
                    .map_err(|e| e.to_string())?;
                let mount = client.mount_slot(slot).await.map_err(|e| e.to_string())?;
                let result = async {
                    let file = client
                        .open(&mount, &path)
                        .await
                        .map_err(|e| e.to_string())?;
                    if file.size() > MAX_ANALYSIS {
                        return Err("Analysis exceeds size limit".into());
                    }
                    client.read_file(&file).await.map_err(|e| e.to_string())
                }
                .await;
                let _ = client.unmount(&mount).await;
                result
            }
        }
    })
    .await
    .map_err(|_| "Phrase analysis read timed out".to_owned())??;
    library::catalog(shared, id, query)?;
    let anlz = prolink_rekordbox::AnlzFile::parse(&bytes).map_err(|e| e.to_string())?;
    let mood = anlz
        .song_structure()
        .and_then(|s| crate::phrases::mood_label(s.mood));
    Ok(json!({"mood": mood}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phrase_paths_reject_escape_and_unrelated_files() {
        assert_eq!(
            phrase_path("/PIONEER/USBANLZ/P001/ANLZ0000.DAT").unwrap(),
            "/PIONEER/USBANLZ/P001/ANLZ0000.EXT"
        );
        for path in [
            "",
            "/etc/passwd",
            "/PIONEER/USBANLZ/../secret",
            "/PIONEER/USBANLZ/a\\b",
        ] {
            assert!(phrase_path(path).is_err());
        }
    }
}
