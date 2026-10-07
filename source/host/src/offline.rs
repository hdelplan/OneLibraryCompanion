//! Optional local files are configured independently of the UI and live transport.
use prolink_rekordbox::{Library, library::Track};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Default)]
pub struct OfflineSource {
    pub capture: Option<PathBuf>,
    library: Option<Library>,
}
impl OfflineSource {
    pub fn from_environment() -> Result<Self, Box<dyn std::error::Error>> {
        let capture = crate::distribution::env("CAPTURE").map(PathBuf::from);
        let library = crate::distribution::env("LIBRARY")
            .map(|path| -> Result<Library, Box<dyn std::error::Error>> {
                Ok(Library::parse(&std::fs::read(path)?)?)
            })
            .transpose()?;
        Ok(Self { capture, library })
    }
    pub fn decode(&self, bytes: &[u8]) -> Result<Value, String> {
        let decoded = pioneer_companion_core::decode(bytes)?;
        let mut result = serde_json::to_value(decoded).map_err(|e| e.to_string())?;
        let track = prolink_rekordbox::AnlzFile::parse(bytes)
            .ok()
            .and_then(|file| {
                let path = file.path()?;
                let t = unique_track(self.library.as_ref()?, path)?;
                Some(crate::library::metadata(t))
            });
        result["track"] = track.unwrap_or(Value::Null);
        Ok(result)
    }
}
// Never infer identity from a filename or choose arbitrarily among duplicates.
fn unique_track<'a>(library: &'a Library, path: &str) -> Option<&'a Track> {
    let mut matches = library
        .tracks
        .values()
        .filter(|track| track.file_path == path);
    let track = matches.next()?;
    if matches.next().is_some() {
        None
    } else {
        Some(track)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_requires_one_exact_path_match() {
        let mut library = Library::default();
        library.tracks.insert(
            1,
            Track {
                id: 1,
                file_path: "/music/a.wav".into(),
                ..Track::default()
            },
        );
        assert_eq!(unique_track(&library, "/music/a.wav").unwrap().id, 1);
        assert!(unique_track(&library, "a.wav").is_none());
        assert!(unique_track(&library, "/music/b.wav").is_none());
        library.tracks.insert(
            2,
            Track {
                id: 2,
                file_path: "/music/a.wav".into(),
                ..Track::default()
            },
        );
        assert!(unique_track(&library, "/music/a.wav").is_none());
    }
}
