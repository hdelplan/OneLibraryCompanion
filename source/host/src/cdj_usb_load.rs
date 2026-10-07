//! CDJ-mounted USB load codec shared by the explicit hardware probe and app.
//! Pure encoding and stopped-target policy; this module does not send packets.

#[derive(Clone, Copy, Debug)]
pub struct LoadUsbTrack {
    pub sender: u8,
    pub source: u8,
    pub target: u8,
    pub track_id: u32,
}

impl LoadUsbTrack {
    pub fn encode(self) -> Result<[u8; 88], &'static str> {
        if !(1..=6).contains(&self.source) || !(1..=6).contains(&self.target) {
            return Err("source and target must be physical player numbers 1–6");
        }
        // This experiment uses an observer identity, not a serving player.
        if !(7..=15).contains(&self.sender) {
            return Err("sender must be an observer number 7–15");
        }
        if self.track_id == 0 {
            return Err("a nonzero export.pdb track ID is required");
        }
        let mut packet = [0; 88];
        packet[..10].copy_from_slice(b"Qspt1WmJOL");
        packet[10] = 0x19;
        packet[11..27].copy_from_slice(b"OLC\0\0\0\0\0\0\0\0\0\0\0\0\0");
        packet[0x1f] = 1;
        packet[0x21] = self.sender;
        packet[0x23] = 0x34;
        packet[0x24] = self.sender;
        packet[0x28] = self.source;
        packet[0x29] = 3; // USB
        packet[0x2a] = 1; // rekordbox-analyzed track
        packet[0x2c..0x30].copy_from_slice(&self.track_id.to_be_bytes());
        packet[0x33] = 0x32; // retained from Beat Link's payload
        packet[0x40] = self.target - 1;
        Ok(packet)
    }
}

/// Conservative experiment policy; does not claim these checks are atomic
/// with a later send or replace checking source media and target identity.
pub struct TargetStatus {
    pub age_ms: u64,
    pub playing: bool,
    pub play_state: u8,
}

impl TargetStatus {
    pub fn check(&self) -> Result<(), &'static str> {
        if self.age_ms >= 1000 {
            return Err("target status is stale");
        }
        if self.playing || !matches!(self.play_state, 0 | 5 | 6) {
            return Err("target must be explicitly stopped, paused or cued");
        }
        Ok(())
    }
}
