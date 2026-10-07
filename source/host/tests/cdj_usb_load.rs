//! Offline preparation for a legacy CDJ USB load experiment. No sockets.
//! Reference: Deep-Symmetry/beat-link VirtualCdj.LOAD_TRACK_PAYLOAD and
//! sendLoadTrackCommand, plus Util.buildPacket (reviewed 2026-10-02).
//! https://djl-analysis.deepsymmetry.org/djl-analysis/loading_tracks.html
//! The reference vector is constructed from that implementation, NOT a capture.

#[path = "../src/cdj_usb_load.rs"]
mod cdj_usb_load;
use cdj_usb_load::{LoadUsbTrack, TargetStatus};

fn request(source: u8, target: u8) -> LoadUsbTrack {
    LoadUsbTrack {
        sender: 7,
        source,
        target,
        track_id: 0x12345678,
    }
}

#[test]
fn other_cdj_usb_matches_independent_reference_vector() {
    // Sender 7, source player 1 USB, destination player 2, analyzed track.
    let reference = "5173707431576d4a4f4c19
        4f4c430000000000000000000000000000000000
        0100070034070000000103010012345678
        00000032000000000000000000000000
        01000000000000000000000000000000
        0000000000000000";
    let hex: String = reference.split_whitespace().collect();
    let expected: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    assert_eq!(request(1, 2).encode().unwrap().as_slice(), expected);
}

#[test]
fn own_usb_and_remote_usb_keep_source_separate_from_destination() {
    let own = request(1, 1).encode().unwrap();
    let remote = request(1, 2).encode().unwrap();
    let differences: Vec<_> = own
        .iter()
        .zip(remote.iter())
        .enumerate()
        .filter_map(|(i, (a, b))| (a != b).then_some(i))
        .collect();
    assert_eq!(differences, [0x40]);
    assert_eq!(own[0x40], 0);
    assert_eq!(remote[0x40], 1);
    let other_source = request(2, 1).encode().unwrap();
    assert_eq!(other_source[0x28], 2);
    assert_eq!(other_source[0x40], 0);
}

#[test]
fn track_ids_are_unsigned_big_endian_and_usb_is_analyzed() {
    for id in [1, 255, 256, 0x12345678, u32::MAX] {
        let packet = LoadUsbTrack {
            track_id: id,
            ..request(1, 2)
        }
        .encode()
        .unwrap();
        assert_eq!(&packet[0x2c..0x30], &id.to_be_bytes());
        assert_eq!(&packet[0x29..0x2b], &[3, 1]);
        assert_eq!(packet.len(), 0x58);
        assert_eq!(&packet[0x22..0x24], &[0, 0x34]);
    }
}

#[test]
fn invalid_identity_is_rejected_before_any_command_can_be_built() {
    for target in [0, 7, 255] {
        assert!(request(1, target).encode().is_err());
    }
    for source in [0, 7, 255] {
        assert!(request(source, 1).encode().is_err());
    }
    for sender in [0, 1, 2, 16, 255] {
        assert!(
            LoadUsbTrack {
                sender,
                ..request(1, 2)
            }
            .encode()
            .is_err()
        );
    }
    assert!(
        LoadUsbTrack {
            track_id: 0,
            ..request(1, 2)
        }
        .encode()
        .is_err()
    );
    assert!(request(5, 6).encode().is_ok());
}

#[test]
fn only_fresh_explicitly_stopped_status_passes_preflight() {
    for play_state in [0, 5, 6] {
        assert!(
            TargetStatus {
                age_ms: 100,
                playing: false,
                play_state
            }
            .check()
            .is_ok()
        );
    }
    // Loading, playing, cue preview, search, end/unknown: fail closed.
    for play_state in [1, 2, 3, 4, 7, 8, 9, 17, 255] {
        assert!(
            TargetStatus {
                age_ms: 100,
                playing: false,
                play_state
            }
            .check()
            .is_err()
        );
    }
    assert!(
        TargetStatus {
            age_ms: 100,
            playing: true,
            play_state: 6
        }
        .check()
        .is_err()
    );
    assert!(
        TargetStatus {
            age_ms: 1000,
            playing: false,
            play_state: 6
        }
        .check()
        .is_err()
    );
}
