//! Bounded library analysis reads, independent of player transport.
use crate::library::{self, Location, Query, Shared};
use prolink::consume::nfs::NfsClient;
use serde_json::{Value, json};
use std::{
    path::{Component, Path},
    time::Duration,
};
static READ: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn analysis_path(path: &str) -> Result<String, String> {
    if !path.to_ascii_lowercase().starts_with("/pioneer/")
        || path.contains('\\')
        || !Path::new(path)
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err("Track has no valid exported analysis path".into());
    }
    Ok(Path::new(path)
        .with_extension("2EX")
        .to_string_lossy()
        .into_owned())
}

pub async fn get(shared: &Shared, id: &str, track_id: u32, query: &Query) -> Result<Value, String> {
    let _permit = READ
        .try_acquire()
        .map_err(|_| "Another waveform preview is still loading; try again shortly")?;
    let (generation, catalog) = library::catalog(shared, id, query)?;
    let (location, interface) =
        library::artwork_location(shared, id, generation).ok_or("USB selection changed")?;
    let track = catalog
        .library
        .tracks
        .get(&track_id)
        .ok_or("Track not in this catalog")?
        .clone();
    let path = analysis_path(&track.analyze_path)?;
    let (bytes, dat, ext) = if let Location::Local(database) = &location {
        let root = database
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .ok_or("Invalid export root")?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let read = |path: &str| -> Option<Vec<u8>> {
            let file = root
                .join(path.trim_start_matches('/'))
                .canonicalize()
                .ok()?;
            if !file.starts_with(&root) || file.metadata().ok()?.len() > 16 * 1024 * 1024 {
                return None;
            }
            std::fs::read(file).ok()
        };
        (
            read(&path),
            read(&track.analyze_path),
            read(
                &Path::new(&track.analyze_path)
                    .with_extension("EXT")
                    .to_string_lossy(),
            ),
        )
    } else {
        let (ip, slot, interface) = library::connection(&location, interface)?;
        tokio::time::timeout(Duration::from_secs(35), async {
            let mut client = NfsClient::connect(ip, interface.as_ref())
                .await
                .map_err(|e| e.to_string())?;
            let mount = client.mount_slot(slot).await.map_err(|e| e.to_string())?;
            let mut files = Vec::new();
            for path in [
                &path,
                &track.analyze_path,
                &Path::new(&track.analyze_path)
                    .with_extension("EXT")
                    .to_string_lossy()
                    .into_owned(),
            ] {
                let bytes = if let Ok(file) = client.open(&mount, path).await {
                    if file.size() <= 16 * 1024 * 1024 {
                        client.read_file(&file).await.ok()
                    } else {
                        None
                    }
                } else {
                    None
                };
                files.push(bytes);
            }

            let _ = client.unmount(&mount).await;
            Ok::<_, String>((files.remove(0), files.remove(0), files.remove(0)))
        })
        .await
        .map_err(|_| "Waveform read timed out; check the connection and refresh the USB")??
    };
    let mut value = tokio::task::spawn_blocking(move || {
        Ok::<_, String>(
            bytes
                .as_deref()
                .and_then(|bytes| pioneer_companion_core::decode(bytes).ok())
                .and_then(|decoded| serde_json::to_value(decoded).ok())
                .unwrap_or_else(|| json!({"detail":null,"preview":null})),
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    if library::artwork_location(shared, id, generation).is_none() {
        return Err("USB selection changed during the read; choose the track again".into());
    }
    annotations(
        &mut value,
        dat.as_deref(),
        ext.as_deref(),
        f64::from(track.duration),
    );
    value["track"] = catalog.track_metadata(&track);
    if !track.artwork_path.is_empty() {
        value["artworkUrl"] = json!(format!(
            "/api/library/{id}/artwork/{}?generation={generation}",
            track.id
        ));
    }
    Ok(value)
}

fn annotations(value: &mut Value, dat: Option<&[u8]>, ext: Option<&[u8]>, duration: f64) {
    let parse = |bytes| prolink_rekordbox::AnlzFile::parse(bytes).ok();
    let dat = dat.and_then(parse);
    let ext = ext.and_then(parse);
    let mut cues = vec![];
    let mut beats = vec![];
    let mut marks = vec![];
    if let Some(dat) = &dat {
        crate::cues::collect(dat, &mut cues);
        if let Some(grid) = dat.beat_grid() {
            for beat in &grid.beats {
                let time = f64::from(beat.time) / 1000.0;
                beats.push(time);
                marks.push(json!({"time":time,"beatInBar":beat.beat_number}));
            }
        }
    }
    if let Some(ext) = &ext {
        crate::cues::collect(ext, &mut cues);
    }
    value["beats"] = json!(marks);
    value["cues"] = json!(cues);
    if let Some(structure) = ext.as_ref().and_then(|ext| ext.song_structure()) {
        value["phrases"] = json!(crate::phrases::segments(structure, &beats, duration));
        value["phraseMood"] = json!(crate::phrases::mood_label(structure.mood));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn analysis_paths_stay_inside_export() {
        assert_eq!(
            analysis_path("/PIONEER/USBANLZ/P001/ANLZ0000.DAT").unwrap(),
            "/PIONEER/USBANLZ/P001/ANLZ0000.2EX"
        );
        for path in [
            "",
            "/other/file.DAT",
            "/PIONEER/../file.DAT",
            "/PIONEER/a\\b.DAT",
        ] {
            assert!(analysis_path(path).is_err());
        }
    }
}
