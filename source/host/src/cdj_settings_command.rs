//! Pure encoder for the documented Pro DJ Link Load Settings command (0x34).
//!
//! Framing and field offsets independently implemented from Deep Symmetry's
//! Loading Tracks and Settings protocol documentation, cross-checked against
//! beat-link VirtualCdj.sendLoadSettingsCommand (LOAD_SETTINGS_PAYLOAD).
//! https://djl-analysis.deepsymmetry.org/djl-analysis/loading_tracks.html#_loading_settings
//! The special header must not use the usual UDP 50002 response encoder.

/// Build a 116-byte command from the 32-byte MYSETTING.DAT settings block.
/// The full saved profile is sent; fields belonging to MYSETTING2 are left zero.
/// This does not announce a sender, authorize a target, send, or confirm apply.
pub fn encode(
    name: &[u8; 20],
    sender: u8,
    target: u8,
    settings: &[u8],
) -> Result<[u8; 116], &'static str> {
    // Use an identity already announced by OLC: serving players 1–4, desktop
    // observer 7, or Direct-IP observer 15. This command does not require us
    // to claim a new source slot. Target support is currently first-generation NXS.
    if !matches!(sender, 1..=4 | 7 | 15) || !(1..=4).contains(&target) || sender == target {
        return Err("Use an existing OLC sender identity and a distinct target player 1–4");
    }
    if settings.len() != 32 || !(0x81..=0x85).contains(&settings[1]) {
        return Err("Expected a 32-byte My Settings profile with valid LCD brightness");
    }
    let mut raw = [0; 116];
    raw[..10].copy_from_slice(b"Qspt1WmJOL");
    raw[10] = 0x34;
    raw[11..31].copy_from_slice(name);
    // Unlike a normal settings response: no subtype; sender is at 0x20.
    raw[0x1f] = 2;
    raw[0x20] = sender;
    raw[0x21] = target;
    raw[0x22..0x24].copy_from_slice(&0x50u16.to_be_bytes());
    raw[0x24..0x28].copy_from_slice(&0x1234_5678u32.to_be_bytes());
    raw[0x28..0x2c].copy_from_slice(&3u32.to_be_bytes());
    raw[0x2c..0x4c].copy_from_slice(settings);
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_documented_command_framing_and_setting_offsets() {
        let mut name = [0; 20];
        name[..3].copy_from_slice(b"OLC");
        let mut settings = [0x80; 32];
        settings[1] = 0x85;
        settings[3] = 0x88;
        settings[24] = 0x81;
        let p = encode(&name, 4, 1, &settings).unwrap();
        // Independent reference header: name padding then command-specific bytes.
        assert_eq!(&p[..11], b"Qspt1WmJOL\x34");
        assert_eq!(
            &p[0x1f..0x2c],
            &[2, 4, 1, 0, 0x50, 0x12, 0x34, 0x56, 0x78, 0, 0, 0, 3]
        );
        assert_eq!(p.len(), 116);
        assert_eq!(p[0x2d], 0x85); // LCD brightness
        assert_eq!(p[0x2f], 0x88); // Auto cue threshold
        assert_eq!(p[0x44], 0x81); // Time mode
        assert_eq!(&p[0x2c..0x4c], &settings);
        assert!(p[0x4c..].iter().all(|b| *b == 0));
    }

    #[test]
    fn rejects_invalid_target_and_truncated_profiles() {
        for (sender, target) in [(0, 1), (4, 0), (4, 4), (5, 1), (4, 5)] {
            assert!(encode(&[0; 20], sender, target, &[0x81; 32]).is_err());
        }
        assert!(encode(&[0; 20], 4, 1, &[0x81; 31]).is_err());
        let mut bad = [0x81; 32];
        bad[1] = 0x80;
        assert!(encode(&[0; 20], 4, 1, &bad).is_err());
    }
}
