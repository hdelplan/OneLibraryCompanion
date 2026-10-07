//! Manual master-handoff experiment: normal observation only, no control packets.
use prolink_proto::status::CdjStatus;
use serde_json::{Value, json};

pub fn evidence(s: &CdjStatus) -> Value {
    let flags = s.flags();
    json!({"number":s.sender().map(|n|n.get()), "track":s.track_id(),
        "sourcePlayer":s.source_player().map(|n|n.get()), "sourceSlot":s.source_slot().0,
        "master":flags.map(|f|f.is_tempo_master()), "masterMeaningful":s.is_tempo_master(),
        "yieldingTo":s.yielding_to().map(|n|n.get()),
        "sync":flags.map(|f|f.is_synced()), "tempoOnlySync":flags.map(|f|f.0 & 2 != 0),
        "playing":flags.map(|f|f.is_playing()), "playState":s.play_state()})
}
pub fn validate(players: [Option<&CdjStatus>; 2], initial: bool) -> Result<(), String> {
    validate_for(players, initial, false)
}
pub fn validate_sync_tap(players: [Option<&CdjStatus>; 2], initial: bool) -> Result<(), String> {
    validate_for(players, initial, true)
}
fn validate_for(
    players: [Option<&CdjStatus>; 2],
    initial: bool,
    sync_tap: bool,
) -> Result<(), String> {
    for (index, player) in players.iter().enumerate() {
        let number = index + 1;
        let s = player.ok_or_else(|| {
            format!("CDJ{number} status is missing or stale. Reconnect and wait for fresh status.")
        })?;
        let flags = s
            .flags()
            .ok_or_else(|| format!("CDJ{number} has no usable Sync/master flags."))?;
        if s.sender().map(|n| usize::from(n.get())) != Some(number) {
            return Err(format!("CDJ{number} identity does not match its status."));
        }
        if (initial || index == 0 || !sync_tap) && (flags.is_synced() || flags.0 & 2 != 0) {
            return Err(format!(
                "Turn Sync OFF on CDJ{number} before starting this capture."
            ));
        }
        if s.track_id() == 0 || s.track_type() != 1 || s.source_player().is_none() {
            return Err(format!(
                "CDJ{number} must report a loaded rekordbox-analyzed track."
            ));
        }
        if index == 0 && (!flags.is_playing() || !matches!(s.play_state(), Some(3 | 4))) {
            return Err("Keep CDJ1 playing during this capture.".into());
        }
        let paused = if initial || sync_tap {
            matches!(s.play_state(), Some(5 | 6))
        } else {
            matches!(s.play_state(), Some(5 | 6 | 8 | 9))
        };
        if index == 1 && (flags.is_playing() || !paused) {
            return Err(
                "Pause CDJ2 with PLAY/PAUSE before the test; keep it paused while jogging.".into(),
            );
        }
        if (initial || sync_tap)
            && (flags.is_tempo_master() != (index == 0)
                || s.is_tempo_master() != Some(index == 0)
                || s.yielding_to().is_some())
        {
            return Err("Keep CDJ1 as the only master; wait for any handoff to finish.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prolink_proto::{DeviceNumber, Slot, status::LoadedTrack};
    fn pair() -> [CdjStatus; 2] {
        [1, 2].map(|n| {
            CdjStatus::builder()
                .device_number(DeviceNumber::new(n).unwrap())
                .loaded_track(Some(LoadedTrack {
                    source_player: DeviceNumber::new(1).unwrap(),
                    slot: Slot::USB,
                    id: 42,
                }))
                .play_state(if n == 1 { 3 } else { 5 })
                .playing(n == 1)
                .tempo_master(n == 1)
                .synced(false)
                .build()
        })
    }
    fn change(s: &CdjStatus, offset: usize, value: u8) -> CdjStatus {
        let mut bytes = s.as_bytes().to_vec();
        bytes[offset] = value;
        CdjStatus::parse(&bytes).unwrap()
    }
    #[test]
    fn preflight_requires_fresh_loaded_paused_sync_off_pair() {
        let [first, second] = pair();
        assert!(validate([Some(&first), Some(&second)], true).is_ok());
        assert!(validate([None, Some(&second)], true).is_err());
        for n in [0, 1] {
            let mut players = pair();
            players[n] = change(&players[n], 0x89, players[n].as_bytes()[0x89] | 0x10);
            assert!(validate([Some(&players[0]), Some(&players[1])], true).is_err());
        }
        let empty = CdjStatus::builder()
            .device_number(DeviceNumber::new(2).unwrap())
            .build();
        assert!(validate([Some(&first), Some(&empty)], true).is_err());
        let audition = change(&second, 0x7b, 7);
        assert!(validate([Some(&first), Some(&audition)], false).is_err());
        let scratch = change(&second, 0x7b, 8);
        assert!(validate([Some(&first), Some(&scratch)], false).is_ok());
        assert!(validate([Some(&first), Some(&scratch)], true).is_err());
    }
    #[test]
    fn handoff_overlap_is_allowed_only_after_start() {
        let [first, second] = pair();
        let other = change(&second, 0x89, second.as_bytes()[0x89] | 0x20);
        assert!(validate([Some(&first), Some(&other)], true).is_err());
        assert!(validate([Some(&first), Some(&other)], false).is_ok());
        let unsynced_bpm = change(&second, 0x89, second.as_bytes()[0x89] | 2);
        assert!(validate([Some(&first), Some(&unsynced_bpm)], false).is_err());
    }
    #[test]
    fn sync_tap_allows_only_target_sync_changes_during_capture() {
        let [first, second] = pair();
        assert!(validate_sync_tap([Some(&first), Some(&second)], true).is_ok());
        let target_on = change(&second, 0x89, second.as_bytes()[0x89] | 0x10);
        assert!(validate_sync_tap([Some(&first), Some(&target_on)], true).is_err());
        assert!(validate_sync_tap([Some(&first), Some(&target_on)], false).is_ok());
        assert!(validate([Some(&first), Some(&target_on)], false).is_err());
        let audible_on = change(&first, 0x89, first.as_bytes()[0x89] | 0x10);
        assert!(validate_sync_tap([Some(&audible_on), Some(&second)], false).is_err());
        let target_master = change(&second, 0x89, second.as_bytes()[0x89] | 0x20);
        assert!(validate_sync_tap([Some(&first), Some(&target_master)], false).is_err());
        assert!(validate_sync_tap([Some(&first), None], false).is_err());
    }
}
