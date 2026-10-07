use prolink::monitor::PlayerStatus;
use prolink_proto::{DeviceNumber, status::CdjStatus};

#[test]
fn synced_fader_difference_is_not_manual_motion_on_either_deck() {
    for number in [1, 2] {
        for master in [false, true] {
            let mut raw = CdjStatus::builder()
                .device_number(DeviceNumber::new(number).unwrap())
                .play_state(3)
                .playing(true)
                .build()
                .into_bytes();
            raw[0x89] |= 0x10;
            raw[0x9e] = u8::from(master);
            raw[0x8b] = 0x7a;
            raw[0x8c..0x90].copy_from_slice(&1_060_949u32.to_be_bytes());
            raw[0x98..0x9c].copy_from_slice(&1_048_576u32.to_be_bytes());
            let decode = |raw: &[u8]| PlayerStatus::from_packet(&CdjStatus::parse(raw).unwrap());
            assert!(!decode(&raw).jogging);
            raw[0x8b] = 0x7e;
            assert!(decode(&raw).jogging, "synced platter hold still detected");
            raw[0x8b] = 0x7a;
            raw[0x89] &= !0x10;
            assert!(
                decode(&raw).jogging,
                "existing unsynced jog detection retained"
            );
        }
    }
}

#[test]
fn motion_speed_decode_is_independent_of_master_and_excludes_ambiguous_transport() {
    for master in [false, true] {
        let mut raw = CdjStatus::builder()
            .play_state(3)
            .playing(true)
            .build()
            .into_bytes();
        raw[0x9e] = u8::from(master);
        raw[0x8b] = 0xfa;
        raw[0x9d] = 9;
        raw[0x98..0x9c].copy_from_slice(&983_564u32.to_be_bytes());
        let decode = |raw: &[u8]| PlayerStatus::from_packet(&CdjStatus::parse(raw).unwrap());
        assert!(decode(&raw).forward_transport);
        assert_eq!(decode(&raw).motion_pitch.unwrap().0, 983_564);
        raw[0x9d] = 13;
        assert!(decode(&raw).forward_transport);
        raw[0x9d] = 1;
        assert!(!decode(&raw).forward_transport, "reverse");
        raw[0x9d] = 9;
        raw[0x8b] |= 4;
        assert!(!decode(&raw).forward_transport, "platter held");
        raw[0x8b] &= !4;
        raw[0x7b] = 4;
        assert!(
            decode(&raw).forward_transport,
            "forward loop; tracker requires endpoints"
        );
        raw[0x9d] = 1;
        assert!(!decode(&raw).forward_transport, "ambiguous loop direction");
        assert!(
            !decode(&raw).reverse,
            "fractional loop is not proof of reverse"
        );
        assert!(decode(&raw).position_requires_direct_updates());
        raw[0x9d] = 9;
        raw[0x7b] = 3;
        raw[0x89] &= !0x40;
        assert!(!decode(&raw).forward_transport, "paused");
    }
}

#[test]
fn reverse_requires_active_transport_not_a_paused_or_held_platter() {
    for primary in [3, 4] {
        let mut raw = CdjStatus::builder()
            .play_state(primary)
            .playing(true)
            .build()
            .into_bytes();
        raw[0x8b] = 0xfa;
        raw[0x9d] = 1;
        let decode = |raw: &[u8]| PlayerStatus::from_packet(&CdjStatus::parse(raw).unwrap());
        assert_eq!(decode(&raw).reverse, primary == 3);
        assert!(decode(&raw).position_requires_direct_updates());
        raw[0x8b] |= 4;
        assert!(!decode(&raw).reverse, "held platter is ambiguous");
        raw[0x8b] &= !4;
        raw[0x89] &= !0x40;
        assert!(!decode(&raw).reverse, "paused is not reverse");
    }
    let mut raw = CdjStatus::builder()
        .play_state(7)
        .playing(false)
        .build()
        .into_bytes();
    raw[0x8b] = 0xfa;
    raw[0x9d] = 9;
    let s = PlayerStatus::from_packet(&CdjStatus::parse(&raw).unwrap());
    assert!(
        s.is_playing && s.forward_transport,
        "cue audition advances with playing flag clear"
    );
}
