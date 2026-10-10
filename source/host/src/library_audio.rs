//! Browser-only audio reads; never changes the source advertised to players.
use crate::library::{self, Location, Query, Shared};
use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use prolink::consume::nfs::{NfsClient, RemoteFile};
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

enum Reader {
    Local(std::fs::File, Option<Arc<BrowserCopy>>),
    Remote(Box<NfsClient>, RemoteFile),
}
// Retain one prepared track; active response bodies pin their own copy.
struct BrowserCopy {
    source: PathBuf,
    len: u64,
    modified: std::time::SystemTime,
    path: PathBuf,
}
impl Drop for BrowserCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
static BROWSER_COPY: Mutex<Option<Arc<BrowserCopy>>> = Mutex::new(None);
fn browser_copy(source: &Path) -> Result<Arc<BrowserCopy>, String> {
    let mut cache = BROWSER_COPY.lock().map_err(|e| e.to_string())?;
    let metadata = source.metadata().map_err(|e| e.to_string())?;
    let modified = metadata.modified().map_err(|e| e.to_string())?;
    if let Some(copy) = cache.as_ref().filter(|c| {
        c.source == source && c.len == metadata.len() && c.modified == modified && c.path.is_file()
    }) {
        return Ok(copy.clone());
    }
    if metadata.len() > 512 * 1024 * 1024 {
        return Err("Browser audio exceeds 512 MiB limit".into());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let copy = Arc::new(BrowserCopy {
        source: source.to_owned(),
        len: metadata.len(),
        modified,
        path: std::env::temp_dir().join(format!("olc-browser-{}-{stamp}.wav", std::process::id())),
    });
    crate::audio_conversion::convert(
        source,
        &copy.path,
        crate::audio_conversion::Target {
            rate: 44100,
            depth: 16,
            aiff: false,
        },
        &|| false,
        &mut |_| {},
    )?;
    let now = source.metadata().map_err(|e| e.to_string())?;
    if now.len() != metadata.len() || now.modified().ok() != Some(modified) {
        return Err("Audio source changed while preparing playback".into());
    }
    *cache = Some(copy.clone());
    Ok(copy)
}
fn bounds(range: Option<&str>, size: u64) -> Result<(u64, u64), String> {
    if size == 0 {
        return Err("Empty audio file".into());
    }
    let Some(range) = range else {
        return Ok((0, size - 1));
    };
    let (start, end) = range
        .strip_prefix("bytes=")
        .and_then(|s| s.split_once('-'))
        .ok_or("Invalid byte range")?;
    let (start, end) = if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| "Invalid suffix range")?;
        if suffix == 0 {
            return Err("Empty range".into());
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        (
            start.parse::<u64>().map_err(|_| "Invalid range start")?,
            if end.is_empty() {
                size - 1
            } else {
                end.parse::<u64>()
                    .map_err(|_| "Invalid range end")?
                    .min(size - 1)
            },
        )
    };
    if start >= size || end < start {
        return Err("Range outside audio file".into());
    }
    Ok((start, end))
}
pub async fn get(
    shared: &Shared,
    id: &str,
    track: u32,
    query: &Query,
    headers: HeaderMap,
) -> Result<Response, String> {
    let (generation, catalog) = library::catalog(shared, id, query)?;
    let path = &catalog
        .library
        .tracks
        .get(&track)
        .ok_or("Track not in catalog")?
        .file_path;
    if !path.starts_with('/')
        || path.contains('\\')
        || !Path::new(path)
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err("Invalid exported audio path".into());
    }
    let (location, interface) =
        library::artwork_location(shared, id, generation).ok_or("Source selection changed")?;
    let mut browser_wav = false;
    let (reader, size) = match location {
        Location::Local(database) => {
            let root = database
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .ok_or("Invalid export root")?
                .canonicalize()
                .map_err(|e| e.to_string())?;
            let file = root
                .join(path.trim_start_matches('/'))
                .canonicalize()
                .map_err(|e| e.to_string())?;
            if !file.starts_with(root) {
                return Err("Audio path outside export".into());
            }
            let copy = if matches!(
                file.extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str(),
                "aif" | "aiff" | "aifc"
            ) {
                browser_wav = true;
                let input = file.clone();
                Some(
                    tokio::task::spawn_blocking(move || browser_copy(&input))
                        .await
                        .map_err(|e| e.to_string())??,
                )
            } else {
                None
            };
            let file = if let Some(copy) = &copy {
                std::fs::File::open(&copy.path)
            } else {
                std::fs::File::open(file)
            }
            .map_err(|e| e.to_string())?;
            let size = file.metadata().map_err(|e| e.to_string())?.len();
            (Reader::Local(file, copy), size)
        }
        location => {
            let (ip, slot, interface) = library::connection(&location, interface)?;
            let (client, file) = tokio::time::timeout(Duration::from_secs(15), async {
                let mut client = NfsClient::connect(ip, interface.as_ref())
                    .await
                    .map_err(|e| e.to_string())?;
                let mount = client.mount_slot(slot).await.map_err(|e| e.to_string())?;
                let file = client.open(&mount, path).await.map_err(|e| e.to_string())?;
                Ok::<_, String>((client, file))
            })
            .await
            .map_err(|_| "Audio source timed out")??;
            let size = file.size();
            (Reader::Remote(Box::new(client), file), size)
        }
    };
    let range = headers.get("range").and_then(|h| h.to_str().ok());
    let (start, end) = match bounds(range, size) {
        Ok(b) => b,
        Err(_) => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [("content-range", format!("bytes */{size}"))],
            )
                .into_response());
        }
    };
    let mime = if browser_wav {
        "audio/wav"
    } else {
        match Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "aif" | "aiff" => "audio/aiff",
            "flac" => "audio/flac",
            "m4a" | "mp4" => "audio/mp4",
            "ogg" => "audio/ogg",
            _ => "application/octet-stream",
        }
    };
    let shared = shared.clone();
    let id = id.to_owned();
    let stream =
        futures_util::stream::try_unfold((reader, start), move |(mut reader, position)| {
            let shared = shared.clone();
            let id = id.clone();
            async move {
                if position > end {
                    return Ok::<_, std::io::Error>(None);
                }
                if library::artwork_location(&shared, &id, generation).is_none() {
                    return Err(std::io::Error::other("Audio source changed"));
                }
                let count = (end - position + 1).min(128 * 1024);
                let bytes = match &mut reader {
                    Reader::Local(file, _copy) => {
                        file.seek(SeekFrom::Start(position))?;
                        let mut bytes = vec![0; count as usize];
                        file.read_exact(&mut bytes)?;
                        bytes
                    }
                    Reader::Remote(client, file) => tokio::time::timeout(
                        Duration::from_secs(15),
                        client.read_range(file, position, count),
                    )
                    .await
                    .map_err(|_| std::io::Error::other("Audio read timed out"))?
                    .map_err(std::io::Error::other)?,
                };
                Ok(Some((bytes, (reader, position + count))))
            }
        });
    let mut response = Body::from_stream(stream).into_response();
    *response.status_mut() = if range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let h = response.headers_mut();
    h.insert("content-type", mime.parse().unwrap());
    h.insert("accept-ranges", "bytes".parse().unwrap());
    h.insert("cache-control", "no-store".parse().unwrap());
    h.insert(
        "content-length",
        (end - start + 1).to_string().parse().unwrap(),
    );
    if range.is_some() {
        h.insert(
            "content-range",
            format!("bytes {start}-{end}/{size}").parse().unwrap(),
        );
    }
    Ok(response)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_aiff_copy_is_wav_and_reused_without_modifying_source() {
        let root = std::env::temp_dir().join(format!("olc-browser-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let wave = root.join("input.wav");
        std::fs::write(
            &wave,
            include_bytes!("../tests/fixtures/transcoding/reference.wav"),
        )
        .unwrap();
        let aiff = root.join("input.aiff");
        crate::audio_conversion::convert(
            &wave,
            &aiff,
            crate::audio_conversion::Target {
                rate: 44100,
                depth: 24,
                aiff: true,
            },
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        let original = std::fs::read(&aiff).unwrap();
        let first = browser_copy(&aiff).unwrap();
        let second = browser_copy(&aiff).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let info = crate::audio_header::inspect(&first.path).unwrap();
        assert_eq!(info["encoding"], 1);
        assert_eq!(info["sampleDepth"], 16);
        assert_eq!(info["sampleRate"], 44100);
        assert_eq!(std::fs::read(&aiff).unwrap(), original);
        BROWSER_COPY.lock().unwrap().take();
        let path = first.path.clone();
        drop(first);
        drop(second);
        assert!(!path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn browser_ranges() {
        assert_eq!(bounds(None, 100).unwrap(), (0, 99));
        assert_eq!(bounds(Some("bytes=20-"), 100).unwrap(), (20, 99));
        assert_eq!(bounds(Some("bytes=-10"), 100).unwrap(), (90, 99));
        assert_eq!(bounds(Some("bytes=0-1"), 100).unwrap(), (0, 1));
        for r in ["bytes=100-", "bytes=5-2", "bytes=-0", "bytes=0-1,5-6"] {
            assert!(bounds(Some(r), 100).is_err());
        }
    }
}
