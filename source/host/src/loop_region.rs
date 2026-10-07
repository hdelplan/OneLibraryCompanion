//! Estimated quantized loops from beat returns and high-resolution bar phase.
use prolink::monitor::{BeatObservation, PlayerStatus};
use serde_json::{Value, json};
use std::time::Instant;
#[derive(Default)]
pub struct LoopRegion {
    key: String,
    previous: Option<u32>,
    confirmed: Option<(f64, f64)>, // zero-based fractional grid indices
    pending_range: Option<((f64, f64), u8)>,
    phase: Option<(Instant, f64)>,
    pulse: Option<(Instant, u32, u8)>,
    beat_changed: Option<Instant>,
    short_candidates: std::collections::VecDeque<(f64, f64)>,
    short_supported: Option<Instant>,
}
impl LoopRegion {
    pub fn observe(
        &mut self,
        key: &str,
        status: PlayerStatus,
        pulse: Option<BeatObservation>,
        grid: &[f64],
        now: Instant,
    ) -> Option<Value> {
        let looping = status.play_state.0 == 4;
        let paused = matches!(status.play_state.0, 5 | 6);
        if self.key != key || !(looping || paused) {
            *self = Self {
                key: key.into(),
                ..Self::default()
            };
        }
        if !(looping || paused) {
            return None;
        }
        if let Some((start, end, beats)) = status.active_loop
            && end > start
            && (looping || paused)
        {
            let (start, end) = (start as f64 / 1_000_000.0, end as f64 / 1_000_000.0);
            let count = if beats > 0 {
                f64::from(beats)
            } else {
                (end - start) * f64::from(status.bpm_centi.unwrap_or(12000)) / 6000.0
            };
            return Some(json!({"start":start,"end":end,"beats":count,"estimated":false}));
        }
        let beat = status.beat_number.filter(|b| *b > 0)?;
        if looping
            && status.is_playing
            && !status.forward_transport
            && status.bar_position.is_none()
        {
            // A direction-ambiguous loop may have just been shortened below
            // one beat. Old endpoints are no longer evidence of its size.
            // Fresh fine phase may establish a new range below; frozen phase
            // is removed by the caller before it reaches this estimator.
            self.confirmed = None;
            self.pending_range = None;
            self.previous = None;
            self.phase = None;
            self.pulse = None;
            self.short_candidates.clear();
            self.short_supported = None;
            return None;
        }
        if looping && status.is_playing && !status.jogging && !status.reverse {
            // Playback outside the range disproves it. Never keep wrapping the
            // display inside a spurious two-beat region during a longer loop.
            if self.confirmed.is_some_and(|(start, end)| {
                f64::from(beat - 1) < start || f64::from(beat - 1) >= end.ceil()
            }) {
                self.confirmed = None;
                self.pending_range = None;
                self.short_candidates.clear();
            }
            if self.previous != Some(beat) {
                self.beat_changed = Some(now);
            }
            // A missed boundary packet can make a return look like a shorter
            // loop. Discard tentative endpoints when playback leaves that range.
            if self.pending_range.is_some_and(|((start, end), _)| {
                f64::from(beat - 1) < start || f64::from(beat - 1) >= end
            }) {
                self.pending_range = None;
            }
            if let Some(previous) = self.previous
                && beat < previous
            {
                self.consider_range((f64::from(beat - 1), f64::from(previous)));
            }
            // A >=2-beat range is no longer credible if the deck keeps cycling
            // inside one beat. Clear it even when shorter endpoints are unknown.
            let beat_seconds = 60.0 / status.effective_bpm().unwrap_or(120.0);
            if self.confirmed.is_some_and(|(s, e)| e - s >= 2.0)
                && self
                    .beat_changed
                    .is_some_and(|t| now.duration_since(t).as_secs_f64() > beat_seconds * 1.6)
            {
                self.confirmed = None;
            }
            self.previous = Some(beat);
            if let Some(pulse) = pulse
                && !pulse.is_stale()
                && self.pulse.is_none_or(|(t, _, _)| t != pulse.received_at)
                && let Some(bar) = pulse.beat.beat_in_bar
            {
                if let Some((at, previous_beat, previous_bar)) = self.pulse {
                    let interval = pulse
                        .received_at
                        .saturating_duration_since(at)
                        .as_secs_f64();
                    if previous_beat == beat
                        && previous_bar == bar.index()
                        && (interval / beat_seconds - 1.0).abs() < 0.28
                    {
                        self.consider_range((f64::from(beat - 1), f64::from(beat)));
                    }
                }
                self.pulse = Some((pulse.received_at, beat, bar.index()));
            }
            if let Some(value) = phase_index(status) {
                if let Some((at, previous)) = self.phase
                    && (value - previous).abs() > 0.001
                {
                    let elapsed = now.duration_since(at).as_secs_f64();
                    if value < previous && elapsed < 0.35 {
                        let length = elapsed / beat_seconds + previous - value;
                        if let Some(size) = short_length(length) {
                            let start = (value / size).floor() * size;
                            let candidate = (start, start + size);
                            self.short_candidates.push_back(candidate);
                            if self.short_candidates.len() > 5 {
                                self.short_candidates.pop_front();
                            }
                            let votes = self
                                .short_candidates
                                .iter()
                                .filter(|&&v| v == candidate)
                                .count();
                            if votes >= 3 {
                                self.confirmed = Some(candidate);
                                self.pending_range = None;
                                self.short_supported = Some(now);
                            }
                        }
                        // A single noisy wrap cannot erase or resize a region.
                        // Expire unsupported short ranges instead of retaining them forever.
                        if self.confirmed.is_some_and(|(a, b)| b - a < 2.0)
                            && self
                                .short_supported
                                .is_some_and(|at| now.duration_since(at).as_secs_f64() > 1.5)
                        {
                            self.confirmed = None;
                            self.short_candidates.clear();
                        }
                    }
                    self.phase = Some((now, value));
                } else if self.phase.is_none() {
                    self.phase = Some((now, value));
                }
            }
        } else {
            self.previous = None;
            self.pending_range = None;
            self.phase = None;
            self.pulse = None;
            self.beat_changed = Some(now);
        }
        let (start, end) = self.confirmed?;
        Some(
            json!({"start":grid_time(grid,start)?,"end":grid_time(grid,end)?,"beats":end-start,"estimated":true}),
        )
    }
    fn consider_range(&mut self, candidate: (f64, f64)) {
        if self.confirmed.is_none() || self.confirmed == Some(candidate) {
            self.confirmed = Some(candidate);
            self.pending_range = None;
            return;
        }
        let votes = match self.pending_range {
            Some((previous, votes)) if previous == candidate => votes + 1,
            _ => 1,
        };
        if votes >= 2 {
            self.confirmed = Some(candidate);
            self.pending_range = None;
        } else {
            self.pending_range = Some((candidate, votes));
        }
    }
}
fn short_length(length: f64) -> Option<f64> {
    [1.0, 0.5, 0.25]
        .into_iter()
        .find(|size| (length - size).abs() < size * 0.2)
}
fn phase_index(s: PlayerStatus) -> Option<f64> {
    let (steps, pos) = s.bar_position?;
    let bar = s.beat_in_bar?;
    if steps == 0 || pos >= steps {
        return None;
    }
    let phase = 4.0 * f64::from(pos) / f64::from(steps);
    if phase.floor() as u8 + 1 != bar {
        return None;
    }
    Some(f64::from(s.beat_number?.checked_sub(u32::from(bar))?) + phase)
}
fn grid_time(grid: &[f64], value: f64) -> Option<f64> {
    if value < 0.0 {
        return None;
    }
    let i = value.floor() as usize;
    let start = *grid.get(i)?;
    if value.fract() == 0.0 {
        return Some(start);
    }
    Some(start + value.fract() * (*grid.get(i + 1)? - start))
}
#[cfg(test)]
mod tests {
    use super::*;
    use prolink::monitor::PlayState;
    use std::time::Duration;
    fn status(beat: u32) -> PlayerStatus {
        PlayerStatus {
            usb_present: false,
            sd_present: false,
            jogging: false,
            bar_position: None,
            active_loop: None,
            beat_in_bar: Some(((beat - 1) % 4 + 1) as u8),
            beat_number: Some(beat),
            reverse: false,
            play_state: PlayState(4),
            track: None,
            bpm_centi: Some(12000),
            pitch: None,
            motion_pitch: None,
            forward_transport: true,
            is_tempo_master: true,
            is_synced: false,
            is_playing: true,
            yielding_to: None,
        }
    }
    #[test]
    fn false_two_beat_loop_is_disproved_then_sixteen_beat_loop_recovers() {
        let grid: Vec<_> = (0..80).map(|n| n as f64 * 0.5).collect();
        let mut tracker = LoopRegion::default();
        let now = Instant::now();
        for beat in [9, 10, 9] {
            tracker.observe("a", status(beat), None, &grid, now);
        }
        assert_eq!(tracker.confirmed, Some((8., 10.)));
        assert!(tracker.observe("a", status(11), None, &grid, now).is_none());
        for beat in 12..=24 {
            tracker.observe("a", status(beat), None, &grid, now);
        }
        let region = tracker.observe("a", status(9), None, &grid, now).unwrap();
        assert_eq!(region["beats"], 16.);
    }
    #[test]
    fn extended_status_decodes_loop_bounds_but_legacy_packet_does_not() {
        let mut bytes = prolink_proto::status::CdjStatus::builder()
            .play_state(4)
            .build()
            .into_bytes();
        assert!(
            PlayerStatus::from_packet(&prolink_proto::status::CdjStatus::parse(&bytes).unwrap())
                .active_loop
                .is_none()
        );
        bytes.resize(0x200, 0);
        bytes[0x1be..0x1c2].copy_from_slice(&128u32.to_be_bytes());
        bytes[0x1c8..0x1ca].copy_from_slice(&16u16.to_be_bytes());
        let decoded =
            PlayerStatus::from_packet(&prolink_proto::status::CdjStatus::parse(&bytes).unwrap());
        assert_eq!(decoded.active_loop, Some((0, 8_388_608, 16)));
    }
    #[test]
    fn reported_loop_is_immediate_and_does_not_require_grid_or_return() {
        let mut tracker = LoopRegion::default();
        let mut s = status(1);
        s.active_loop = Some((0, 8_000_000, 16));
        let region = tracker.observe("a", s, None, &[], Instant::now()).unwrap();
        assert_eq!(region["start"], 0.);
        assert_eq!(region["end"], 8.);
        assert_eq!(region["beats"], 16.);
        assert_eq!(region["estimated"], false);
        s.active_loop = Some((0, 250_000, 0));
        assert_eq!(
            tracker.observe("a", s, None, &[], Instant::now()).unwrap()["beats"],
            0.5
        );
    }
    #[test]
    fn first_return_and_pause_retention() {
        let grid: Vec<_> = (0..40).map(f64::from).collect();
        let mut t = LoopRegion::default();
        let now = Instant::now();
        for b in 9..=16 {
            assert!(t.observe("a", status(b), None, &grid, now).is_none());
        }
        assert_eq!(
            t.observe("a", status(9), None, &grid, now).unwrap()["beats"],
            8.0
        );
        let mut paused = status(9);
        paused.play_state = PlayState(5);
        paused.is_playing = false;
        assert_eq!(
            t.observe("a", paused, None, &grid, now).unwrap()["beats"],
            8.0
        );
        assert!(t.observe("b", paused, None, &grid, now).is_none());
    }
    #[test]
    fn half_beat_phase_wrap_and_fractional_grid_mapping() {
        let grid: Vec<_> = (0..40).map(|n| f64::from(n) * 0.5).collect();
        let mut t = LoopRegion::default();
        let now = Instant::now();
        let mut s = status(9);
        s.bar_position = Some((2000, 200));
        // P3 can be ambiguous in fractional loops; fresh phase still supplies
        // real wrap evidence even when speed extrapolation is disabled.
        s.forward_transport = false;
        t.observe("a", s, None, &grid, now);
        s.bar_position = Some((2000, 50));
        assert!(
            t.observe("a", s, None, &grid, now + Duration::from_millis(100))
                .is_none()
        );
        for base in [250, 500] {
            s.bar_position = Some((2000, 200));
            t.observe("a", s, None, &grid, now + Duration::from_millis(base));
            s.bar_position = Some((2000, 50));
            t.observe("a", s, None, &grid, now + Duration::from_millis(base + 100));
        }
        let v = t
            .observe("a", s, None, &grid, now + Duration::from_millis(610))
            .unwrap();
        assert_eq!(v["beats"], 0.5);
        assert_eq!(v["start"], 4.0);
        assert_eq!(v["end"], 4.25);
    }
    #[test]
    fn missed_boundary_packets_do_not_shrink_a_confirmed_loop() {
        let grid: Vec<_> = (0..40).map(f64::from).collect();
        let mut tracker = LoopRegion::default();
        let now = Instant::now();
        for beat in [9, 10, 11, 12, 9] {
            tracker.observe("a", status(beat), None, &grid, now);
        }
        // Missing 11/12 at the end, or 9/10 at the start, used to
        // immediately replace the four-beat region with a two-beat one.
        for beat in [10, 9, 10, 11, 12, 11, 12, 9] {
            let value = tracker
                .observe("a", status(beat), None, &grid, now)
                .unwrap();
            assert_eq!(value["beats"], 4.0);
            assert_eq!(value["start"], 8.0);
        }
    }
    #[test]
    fn genuine_loop_resize_requires_two_consistent_returns() {
        let grid: Vec<_> = (0..40).map(f64::from).collect();
        let mut tracker = LoopRegion::default();
        let now = Instant::now();
        for beat in [9, 10, 11, 12, 9, 10, 9] {
            tracker.observe("a", status(beat), None, &grid, now);
        }
        assert_eq!(tracker.confirmed, Some((8.0, 12.0)));
        for beat in [10, 9] {
            tracker.observe("a", status(beat), None, &grid, now);
        }
        assert_eq!(tracker.confirmed, Some((8.0, 10.0)));
        let mut paused = status(9);
        paused.play_state = PlayState(5);
        paused.is_playing = false;
        assert_eq!(
            tracker.observe("a", paused, None, &grid, now).unwrap()["beats"],
            2.0
        );
        assert!(tracker.observe("new", paused, None, &grid, now).is_none());
    }
    #[test]
    fn one_beat_loop_uses_distinct_beat_events_not_status_polling() {
        use prolink_proto::{
            DeviceName, DeviceNumber,
            beat::{Beat, BeatInBar, Pitch, Timings},
        };
        let grid: Vec<_> = (0..40).map(|n| n as f64 * 0.5).collect();
        let t = Instant::now();
        let mut tracker = LoopRegion::default();
        let beat = Beat {
            name: DeviceName::new("CDJ-2000nexus"),
            device: DeviceNumber::ONE,
            timings: Timings::default(),
            pitch: Pitch::UNITY,
            bpm_centi: 12000,
            beat_in_bar: BeatInBar::new(1),
            scratching: false,
        };
        let first = BeatObservation {
            beat,
            received_at: t,
            age: std::time::Duration::ZERO,
        };
        assert!(
            tracker
                .observe("a", status(9), Some(first), &grid, t)
                .is_none()
        );
        assert!(
            tracker
                .observe("a", status(9), Some(first), &grid, t)
                .is_none()
        );
        let at = t + std::time::Duration::from_millis(500);
        let next = BeatObservation {
            beat,
            received_at: at,
            age: std::time::Duration::ZERO,
        };
        let p = tracker
            .observe("a", status(9), Some(next), &grid, at)
            .unwrap();
        assert_eq!(p["beats"], 1.);
        assert_eq!(p["start"], 4.);
        assert_eq!(p["end"], 4.5);
    }
    #[test]
    fn nexus_fractional_transition_clears_old_one_beat_range_without_fine_evidence() {
        let now = Instant::now();
        let grid: Vec<_> = (0..250).map(|n| n as f64 * 0.461).collect();
        let mut tracker = LoopRegion::default();
        tracker.observe("a", status(225), None, &grid, now);
        tracker.consider_range((224., 225.));
        // State/phase values from build 19 test 2: forward short loop, no
        // changing fine phase, P3=1. No reverse control was used in the video.
        let mut raw = prolink_proto::status::CdjStatus::builder()
            .play_state(4)
            .playing(true)
            .build()
            .into_bytes();
        raw.resize(0x11c, 0);
        raw[0x8b] = 0xfa;
        raw[0x9d] = 1;
        raw[0xa0..0xa4].copy_from_slice(&225u32.to_be_bytes());
        raw[0xa6] = 1;
        raw[0x116..0x118].copy_from_slice(&1918u16.to_be_bytes());
        let mut s =
            PlayerStatus::from_packet(&prolink_proto::status::CdjStatus::parse(&raw).unwrap());
        assert!(!s.reverse);
        assert!(s.position_requires_direct_updates());
        let mut fine = crate::bar_position::BarPosition::default();
        for offset in [200, 400, 600] {
            let at = now + Duration::from_millis(offset);
            assert!(fine.position("a", s, &grid, at, at).is_none());
        }
        s.bar_position = None; // Same filtering as both live host adapters.
        for offset in [200, 400, 600] {
            assert!(
                tracker
                    .observe("a", s, None, &grid, now + Duration::from_millis(offset))
                    .is_none()
            );
        }
        assert!(tracker.confirmed.is_none());
        // Exact loop endpoints from newer hardware remain authoritative.
        s.active_loop = Some((1_000_000, 1_230_500, 0));
        assert_eq!(
            tracker.observe("a", s, None, &grid, now).unwrap()["estimated"],
            false
        );
    }
    #[test]
    fn reject_ambiguous_short_periods() {
        assert_eq!(short_length(0.5), Some(0.5));
        assert_eq!(short_length(0.125), None);
    }
}
