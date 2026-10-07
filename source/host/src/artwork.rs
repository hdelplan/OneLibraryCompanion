//! Bounded, lazy artwork reads, with one NFS client and request per catalog/path.
use crate::library::{self, Location, Query, Shared};
use axum::body::Bytes;
use prolink::consume::nfs::NfsClient;
use std::{
    collections::BTreeMap,
    path::{Component, Path},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, OnceCell, Semaphore};
const MAX_IMAGE: u64 = 1024 * 1024;
static READS: Semaphore = Semaphore::const_new(2);
pub type Image = (&'static str, Bytes);
#[derive(Default)]
pub struct Cache {
    images: Mutex<BTreeMap<String, Arc<OnceCell<Option<Image>>>>>,
    client: Mutex<Option<NfsClient>>,
}
fn safe_path(path: &str) -> bool {
    path.to_ascii_lowercase().starts_with("/pioneer/artwork/")
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}
fn image(bytes: Vec<u8>) -> Option<Image> {
    if bytes.len() as u64 > MAX_IMAGE {
        return None;
    }
    let mime = if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else {
        return None;
    };
    Some((mime, Bytes::from(bytes)))
}
pub async fn get(shared: &Shared, id: &str, track_id: u32, query: &Query) -> Option<Image> {
    let (generation, catalog) = library::catalog(shared, id, query).ok()?;
    let path = catalog.library.tracks.get(&track_id)?.artwork_path.clone();
    if !safe_path(&path) {
        return None;
    }
    let (location, interface) = library::artwork_location(shared, id, generation)?;
    let cell = {
        let mut cache = catalog.artwork.images.lock().await;
        if !cache.contains_key(&path) && cache.len() >= 32 {
            // Keep in-flight cells pinned; fail this thumbnail instead of growing unbounded.
            let key = cache
                .iter()
                .find(|(_, cell)| cell.initialized())
                .map(|(key, _)| key.clone())?;
            cache.remove(&key);
        }
        cache.entry(path.clone()).or_default().clone()
    };
    let result = cell
        .get_or_init(|| async {
            tokio::time::timeout(Duration::from_secs(12), async {
                let _permit = READS.acquire().await.ok()?;
                match location {
                    Location::Remote(..) | Location::Direct(_) => {
                        let (ip, slot, interface) =
                            library::connection(&location, interface).ok()?;
                        let mut connection = catalog.artwork.client.lock().await;
                        if connection.is_none() {
                            *connection =
                                Some(NfsClient::connect(ip, interface.as_ref()).await.ok()?);
                        }
                        let client = connection.as_mut()?;
                        let result = async {
                            let mount = client.mount_slot(slot).await.ok()?;
                            let file = client.open(&mount, &path).await.ok()?;
                            if file.size() > MAX_IMAGE {
                                return None;
                            }
                            image(client.read_file(&file).await.ok()?)
                        }
                        .await;
                        if result.is_none() {
                            *connection = None;
                        }
                        result
                    }
                    Location::Local(database) => tokio::task::spawn_blocking(move || {
                        use std::io::Read;
                        let rekordbox = database.parent()?;
                        if !rekordbox
                            .file_name()?
                            .to_str()?
                            .eq_ignore_ascii_case("rekordbox")
                        {
                            return None;
                        }
                        let pioneer = rekordbox.parent()?;
                        if !pioneer
                            .file_name()?
                            .to_str()?
                            .eq_ignore_ascii_case("PIONEER")
                        {
                            return None;
                        }
                        let root = pioneer.parent()?.canonicalize().ok()?;
                        let file = root
                            .join(path.trim_start_matches('/'))
                            .canonicalize()
                            .ok()?;
                        if !file.starts_with(&root) {
                            return None;
                        }
                        let mut bytes = Vec::new();
                        std::fs::File::open(file)
                            .ok()?
                            .take(MAX_IMAGE + 1)
                            .read_to_end(&mut bytes)
                            .ok()?;
                        image(bytes)
                    })
                    .await
                    .ok()
                    .flatten(),
                }
            })
            .await
            .ok()
            .flatten()
        })
        .await
        .clone();
    // Ejection, refresh or replacement during a download invalidates the result.
    library::catalog(shared, id, query).ok()?;
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_export_artwork_paths_and_raster_images_are_allowed() {
        assert!(safe_path("/PIONEER/Artwork/00001/a.jpg"));
        for path in [
            "/PIONEER/Artwork/../../../secret",
            "/etc/passwd",
            "/PIONEER/Artwork/..\\secret",
        ] {
            assert!(!safe_path(path));
        }
        assert!(image(b"<svg>script</svg>".to_vec()).is_none());
        assert_eq!(image(vec![0xff, 0xd8, 0xff, 0]).unwrap().0, "image/jpeg");
        assert!(image(vec![0; MAX_IMAGE as usize + 1]).is_none());
    }
}
