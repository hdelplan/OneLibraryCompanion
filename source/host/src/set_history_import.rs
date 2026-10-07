//! Imports complete, date-labelled Rekordbox histories.
use crate::set_history::{Event, Set, Track};
use prolink_rekordbox::Library;

// Only unambiguous ISO dates are accepted. HISTORY 001 is not a date.
fn date(name: &str) -> Option<u64> {
    for bytes in name.as_bytes().windows(10) {
        if bytes[4] != b'-' || bytes[7] != b'-' {
            continue;
        }
        let s = std::str::from_utf8(bytes).ok()?;
        let y: u32 = s[..4].parse().ok()?;
        let m: u32 = s[5..7].parse().ok()?;
        let d: u32 = s[8..].parse().ok()?;
        if !(1970..=2100).contains(&y) || !(1..=12).contains(&m) {
            continue;
        }
        let leap =
            |y: u32| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
        let months = [
            31,
            if leap(y) { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        if d == 0 || d > months[(m - 1) as usize] {
            continue;
        }
        let days: u64 = (1970..y)
            .map(|year| if leap(year) { 366 } else { 365 })
            .sum::<u64>()
            + months[..(m - 1) as usize]
                .iter()
                .map(|n| *n as u64)
                .sum::<u64>()
            + d as u64
            - 1;
        return Some(days * 86_400_000);
    }
    None
}
fn make_set(
    id: String,
    title: String,
    started_at: u64,
    tracks: Vec<&prolink_rekordbox::Track>,
) -> Set {
    let events: Vec<_> = tracks
        .into_iter()
        .enumerate()
        .map(|(i, t)| Event {
            id: format!("entry-{i}"),
            deck: None,
            source: "rekordbox-export".into(),
            track: Track::from_library(t),
        })
        .collect();
    Set {
        id: id.clone(),
        source_id: Some(id),
        title,
        started_at,
        ended_at: Some(started_at),
        date_only: true,
        location: String::new(),
        comment: String::new(),
        origin: "imported".into(),
        recovered: false,
        order: events.iter().map(|e| e.id.clone()).collect(),
        events,
    }
}
pub fn prepare(lib: &Library) -> Result<(Vec<Set>, String), String> {
    let mut sets = vec![];
    let mut undated = 0;
    let mut missing = 0;
    for h in lib.history.values() {
        let Some(day) = date(&h.name) else {
            undated += 1;
            continue;
        };
        // Do not silently import incomplete playlists.
        let tracks: Vec<_> = h
            .track_ids
            .iter()
            .filter_map(|id| lib.tracks.get(id))
            .collect();
        if tracks.len() != h.track_ids.len() || tracks.is_empty() {
            missing += 1;
            continue;
        }
        let signature = format!(
            "{}:{:?}:{}",
            h.name,
            h.track_ids,
            tracks
                .iter()
                .map(|t| t.file_path.as_str())
                .collect::<Vec<_>>()
                .join("|")
        );
        let digest =
            prolink_rekordbox::stable_digest(format!("{}{}", "0".repeat(64), signature).as_bytes());
        let id = format!("import-{digest}");
        let mut set = make_set(id, h.name.clone(), day, tracks);
        set.comment = "Imported from Rekordbox. Date taken from the history name; start/end times and the original played-duration rule are unavailable.".into();
        sets.push(set);
    }
    if !sets.is_empty() {
        return Ok((
            sets,
            format!(
                "Imported dated Rekordbox histories. Skipped {undated} undated and {missing} incomplete histories."
            ),
        ));
    }
    Err(format!(
        "No complete dated Rekordbox histories found. Skipped {undated} undated and {missing} incomplete histories. History names must contain YYYY-MM-DD."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn date_filter_does_not_invent_dates() {
        assert_eq!(date("HISTORY 001"), None);
        assert_eq!(date("2026-02-29"), None);
        assert!(date("2026-03-01").unwrap() > date("2026-02-28").unwrap());
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;
    fn library() -> Library {
        let mut lib = Library::default();
        for id in 1..=40 {
            lib.tracks.insert(
                id,
                prolink_rekordbox::Track {
                    id,
                    title: format!("Track {id}"),
                    key: if id % 2 == 0 { "8A" } else { "9A" }.into(),
                    file_path: format!("/music/{id}.mp3"),
                    ..Default::default()
                },
            );
        }
        lib.playlists.insert(
            1,
            prolink_rekordbox::library::Playlist {
                id: 1,
                name: "MAX4.0".into(),
                track_ids: (1..=40).collect(),
                ..Default::default()
            },
        );
        lib
    }
    #[test]
    fn a_playlist_without_histories_does_not_create_sets() {
        assert!(prepare(&library()).is_err());
    }
    #[test]
    fn dated_histories_preserve_dates_replays_and_deduplicate() {
        let mut lib = library();
        for (id, name, tracks) in [
            (1, "2026-02-28", vec![1]),
            (2, "2026-03-01", vec![2, 1, 2]),
            (3, "HISTORY 003", vec![3]),
            (4, "2026-04-01", vec![1, 99]),
        ] {
            lib.history.insert(
                id,
                prolink_rekordbox::library::HistoryPlaylist {
                    id,
                    name: name.into(),
                    track_ids: tracks,
                },
            );
        }
        let (sets, _) = prepare(&lib).unwrap();
        assert_eq!(sets.len(), 2);
        let replay = sets.iter().find(|s| s.title == "2026-03-01").unwrap();
        assert!(sets.iter().any(|s| s.title == "2026-02-28"));
        assert!(sets[0].date_only);
        assert_eq!(
            replay.events.iter().map(|e| e.track.id).collect::<Vec<_>>(),
            [2, 1, 2]
        );
        let path =
            std::env::temp_dir().join(format!("pc-import-test-{}", crate::set_history::now()));
        let mut store = crate::set_history::Store::open(path.clone());
        assert_eq!(store.import(&lib).unwrap().len(), 4);
        assert!(store.import(&lib).unwrap().is_empty());
        assert_eq!(store.snapshot()["sets"].as_array().unwrap().len(), 2);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn empty_library_cannot_create_history() {
        assert!(prepare(&Library::default()).is_err());
        let mut lib = library();
        lib.tracks.clear();
        assert!(prepare(&lib).is_err());
    }
}
