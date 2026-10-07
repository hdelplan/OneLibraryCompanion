//! Bounded direct-source analysis reads, independent of player transport.
use crate::library::{self, Location, Query, Shared};
use prolink::{Slot, consume::nfs::NfsClient};
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
    let (location, _) =
        library::artwork_location(shared, id, generation).ok_or("USB selection changed")?;
    let Location::Direct(ip) = location else {
        return Err("Preview is currently available for manual-IP sources".into());
    };
    let track = catalog
        .library
        .tracks
        .get(&track_id)
        .ok_or("Track not in this catalog")?
        .clone();
    let path = analysis_path(&track.analyze_path)?;
    let (bytes, beats, cues) =
        tokio::time::timeout(Duration::from_secs(35), async {
            let mut client = NfsClient::connect(ip, None)
                .await
                .map_err(|e| e.to_string())?;
            let mount = client
                .mount_slot(Slot::USB)
                .await
                .map_err(|e| e.to_string())?;
            let file = client
                .open(&mount, &path)
                .await
                .map_err(|e| format!("Native three-band waveform (.2EX) unavailable: {e}"))?;
            if file.size() > 16 * 1024 * 1024 {
                return Err("Analysis exceeds 16 MiB limit".into());
            }
            let result = client.read_file(&file).await.map_err(|e| e.to_string());
            let mut beats = vec![];
            let mut cues = vec![];
            if let Ok(file) = client.open(&mount, &track.analyze_path).await
                && file.size() <= 16 * 1024 * 1024
                && let Ok(bytes) = client.read_file(&file).await
                && let Ok(anlz) = prolink_rekordbox::AnlzFile::parse(&bytes)
            {
                crate::cues::collect(&anlz, &mut cues);
                if let Some(grid) = anlz.beat_grid() {
                    beats = grid
                .beats
                .iter()
                .map(|b| json!({"time": f64::from(b.time) / 1000.0, "beatInBar": b.beat_number}))
                .collect();
                }
            }
            let _ = client.unmount(&mount).await;
            result.map(|bytes| (bytes, beats, cues))
        })
        .await
        .map_err(|_| "Waveform read timed out; check the connection and refresh the USB")??;
    let mut value = tokio::task::spawn_blocking(move || {
        let decoded = pioneer_companion_core::decode(&bytes)?;
        serde_json::to_value(decoded).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if library::artwork_location(shared, id, generation).is_none() {
        return Err("USB selection changed during the read; choose the track again".into());
    }
    if !beats.is_empty() {
        value["beats"] = json!(beats);
    }
    value["cues"] = json!(cues);
    value["track"] = catalog.track_metadata(&track);
    if !track.artwork_path.is_empty() {
        value["artworkUrl"] = json!(format!(
            "/api/library/{id}/artwork/{}?generation={generation}",
            track.id
        ));
    }
    Ok(value)
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
