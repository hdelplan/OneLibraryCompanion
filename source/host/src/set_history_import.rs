//! Imports date-labelled Rekordbox histories; otherwise creates explicit samples.
use crate::set_history::{Event, Set, Track};
use prolink_rekordbox::Library;

pub fn compatible(a: &str, b: &str) -> bool {
    let (Some(a), Some(b)) = (crate::musical_key::rank(a), crate::musical_key::rank(b)) else {
        return false;
    };
    a == b
        || a / 2 == b / 2
        || (a % 2 == b % 2 && ((a / 2 + 1) % 12 == b / 2 || (b / 2 + 1) % 12 == a / 2))
}
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
    origin: &str,
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
        date_only: origin == "imported",
        location: String::new(),
        comment: String::new(),
        origin: origin.into(),
        recovered: false,
        order: events.iter().map(|e| e.id.clone()).collect(),
        events,
    }
}
pub fn prepare(lib: &Library) -> Result<(Vec<Set>, String), String> {
    let cutoff = date("2026-03-01").unwrap();
    let mut sets = vec![];
    let mut undated = 0;
    let mut missing = 0;
    for h in lib.history.values() {
        let Some(day) = date(&h.name) else {
            undated += 1;
            continue;
        };
        if day < cutoff {
            continue;
        }
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
        let mut set = make_set(id, h.name.clone(), day, "imported", tracks);
        set.comment = "Imported from Rekordbox. Date taken from the history name; start/end times and the original played-duration rule are unavailable.".into();
        sets.push(set);
    }
    if !sets.is_empty() {
        return Ok((
            sets,
            format!(
                "Imported dated Rekordbox histories from 1 March 2026. Skipped {undated} undated and {missing} incomplete histories."
            ),
        ));
    }
    let playlist = lib.playlists.values().find(|p|!p.is_folder && p.name.eq_ignore_ascii_case("MAX4.0")).ok_or("No dated histories from 1 March 2026 and no MAX4.0 playlist found. Select another source.")?;
    let mut tracks: Vec<_> = playlist
        .track_ids
        .iter()
        .filter_map(|id| lib.tracks.get(id))
        .filter(|t| crate::musical_key::rank(&t.key).is_some())
        .collect();
    tracks.sort_by_key(|t| t.id);
    tracks.dedup_by_key(|t| t.id);
    if tracks.len() < 20 {
        return Err(
            "MAX4.0 has fewer than 20 tracks with known keys; cannot create the requested samples."
                .into(),
        );
    }
    let mut rng = crate::set_history::now().max(1);
    fn random(seed: &mut u64) -> usize {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed as usize
    }
    for (i, day) in ["2026-03-14", "2026-04-18", "2026-05-23"]
        .iter()
        .enumerate()
    {
        let count = (20 + random(&mut rng) % 11).min(tracks.len());
        let mut chosen = vec![];
        for _ in 0..2000 {
            let mut pool = tracks.clone();
            for n in (1..pool.len()).rev() {
                let k = random(&mut rng) % (n + 1);
                pool.swap(n, k);
            }
            let mut sequence = vec![pool.pop().unwrap()];
            while sequence.len() < count {
                let last = sequence.last().unwrap();
                let Some(next) = pool.iter().position(|t| compatible(&last.key, &t.key)) else {
                    break;
                };
                sequence.push(pool.remove(next));
            }
            if sequence.len() == count {
                chosen = sequence;
                break;
            }
        }
        if chosen.is_empty() {
            return Err(
                "Could not build 20–30 unique tracks with compatible keys from MAX4.0.".into(),
            );
        }
        let start = date(day).unwrap() + 12 * 3_600_000;
        let mut set = make_set(
            format!("sample-max4-v1-{}", i + 1),
            format!("MAX4.0 · Sample set {}", i + 1),
            start,
            "sample",
            chosen,
        );
        set.ended_at = Some(start + set.events.len() as u64 * 240_000);
        set.comment="Demonstration only — not a recorded performance. Random selection from MAX4.0; adjacent tracks use the same Camelot key, ±1 on the same letter, or the relative major/minor. Dates and duration are illustrative.".into();
        sets.push(set);
    }
    Ok((
        sets,
        format!(
            "No usable dated Rekordbox histories from 1 March 2026 ({0} histories; {undated} undated; {missing} incomplete). Created three sample sets from MAX4.0. Sample dates are illustrative.",
            lib.history.len()
        ),
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
    #[test]
    fn harmonic_rules_cover_wrap_and_relative_keys() {
        for (a, b) in [("12A", "1A"), ("8A", "8B"), ("Am", "C"), ("7A", "8A")] {
            assert!(compatible(a, b));
        }
        assert!(!compatible("8A", "9B"));
        assert!(!compatible("", ""));
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
    fn sample_sets_have_unique_tracks_and_compatible_transitions() {
        let lib = library();
        let (sets, _) = prepare(&lib).unwrap();
        assert_eq!(sets.len(), 3);
        for set in sets {
            assert_eq!(set.origin, "sample");
            assert!((20..=30).contains(&set.events.len()));
            let ids: std::collections::BTreeSet<_> =
                set.events.iter().map(|e| e.track.id).collect();
            assert_eq!(ids.len(), set.events.len());
            assert!(
                set.events
                    .windows(2)
                    .all(|pair| compatible(&pair[0].track.key, &pair[1].track.key))
            );
        }
    }
    #[test]
    fn dated_histories_filter_cutoff_preserve_replays_and_deduplicate() {
        let mut lib = library();
        for (id, name, tracks) in [
            (1, "2026-02-28", vec![1]),
            (2, "2026-03-01", vec![2, 1, 2]),
            (3, "HISTORY 003", vec![3]),
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
        assert_eq!(sets.len(), 1);
        assert!(sets[0].date_only);
        assert_eq!(
            sets[0]
                .events
                .iter()
                .map(|e| e.track.id)
                .collect::<Vec<_>>(),
            [2, 1, 2]
        );
        let path =
            std::env::temp_dir().join(format!("pc-import-test-{}", crate::set_history::now()));
        let mut store = crate::set_history::Store::open(path.clone());
        assert_eq!(store.import(&lib).unwrap().len(), 3);
        assert!(store.import(&lib).unwrap().is_empty());
        assert_eq!(store.snapshot()["sets"].as_array().unwrap().len(), 1);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn unusable_samples_fail_without_inventing_tracks() {
        assert!(prepare(&Library::default()).is_err());
        let mut lib = library();
        lib.tracks.clear();
        assert!(prepare(&lib).is_err());
    }
}
