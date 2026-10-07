//! Conservative fine positions from real packet times, with a bounded hold.
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub seconds: f64,
    pub held: bool,
}
impl Position {
    pub fn quality(self) -> &'static str {
        if self.held { "held" } else { "fine" }
    }
}
#[derive(Default)]
pub struct BarPosition {
    key: String,
    pair: Option<(u16, u16)>,
    changed: Option<Instant>,
    playing: bool,
    last: Option<(f64, Instant)>,
    observed_at: Option<Instant>,
    paused_trusted: bool,
    cue_trusted: bool,
    master: Option<bool>,
    coherent_changes: u8,
    previous_valid: bool,
    previous_position: Option<f64>,
}
impl BarPosition {
    pub fn observed_at(&self) -> Option<Instant> {
        self.last.map(|(_, at)| at)
    }
    pub fn position(
        &mut self,
        key: &str,
        status: prolink::monitor::PlayerStatus,
        beats: &[f64],
        observed_at: Instant,
        now: Instant,
    ) -> Option<Position> {
        if self.key != key
            || self
                .master
                .is_some_and(|master| master != status.is_tempo_master)
        {
            *self = Self {
                key: key.into(),
                ..Self::default()
            };
        }
        self.master = Some(status.is_tempo_master);
        let mapped = status
            .beat_number
            .zip(status.beat_in_bar)
            .zip(status.bar_position)
            .and_then(|((beat, bar), pair)| map(beat, bar, pair, beats));
        let valid = mapped.is_some();
        if self.observed_at.is_none_or(|at| observed_at > at) {
            let first = self.observed_at.is_none();
            let changed = (status.is_tempo_master
                || mapped
                    .zip(self.previous_position)
                    .is_some_and(|(a, b)| (a - b).abs() > 0.002))
                && valid
                && self.previous_valid
                && self.pair.is_some()
                && self.pair != status.bar_position;
            if changed {
                if self.observed_at.is_some_and(|at| {
                    observed_at.saturating_duration_since(at) > Duration::from_millis(350)
                }) {
                    self.coherent_changes = 0;
                }
                self.coherent_changes = self.coherent_changes.saturating_add(1);
            } else if !valid {
                self.coherent_changes = 0;
            }
            if status.play_state.0 != 6 || status.is_playing || !valid {
                self.cue_trusted = false;
            } else if changed {
                // One real position change followed by CUED is sufficient;
                // remaining stationary on that cue is expected, not stale motion.
                self.cue_trusted = true;
            }
            self.previous_valid = valid;
            self.previous_position = mapped;
            if status.is_playing != self.playing {
                // A paused packet must not revive phase frozen during playback.
                self.paused_trusted = !status.is_playing
                    && (changed
                        || self.last.is_some_and(|(_, at)| {
                            observed_at.saturating_duration_since(at) < Duration::from_millis(200)
                        }));
                if status.is_playing {
                    self.changed = None;
                    self.last = None;
                }
            }
            if first && !status.is_playing {
                self.paused_trusted = true;
            }
            if changed {
                self.changed = Some(observed_at);
                if !status.is_playing {
                    self.paused_trusted = true;
                }
            }
            self.playing = status.is_playing;
            self.pair = status.bar_position;
            self.observed_at = Some(observed_at);
        }
        // Frozen non-master fields are not evidence. Accept this source only
        // after multiple coherent changes, and only while movement is fresh.
        if !status.is_tempo_master
            && !self.cue_trusted
            && (self.coherent_changes < 2
                || self.changed.is_none_or(|at| {
                    now.saturating_duration_since(at) >= Duration::from_millis(350)
                }))
        {
            return None;
        }
        let packet_recent = now.saturating_duration_since(observed_at) < Duration::from_millis(350);
        let phase_recent = self
            .changed
            .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_millis(200));
        if packet_recent
            && (if status.is_playing {
                phase_recent
            } else {
                self.paused_trusted
            })
            && let Some(candidate) = status
                .beat_number
                .zip(status.beat_in_bar)
                .zip(self.pair)
                .and_then(|((beat, bar), pair)| map(beat, bar, pair, beats))
        {
            // Repeated snapshots never extend the evidence lifetime.
            let evidence_at = if status.is_playing {
                self.changed?
            } else {
                observed_at
            };
            self.last = Some((candidate, evidence_at));
            return Some(Position {
                seconds: candidate,
                held: false,
            });
        }
        self.last
            .filter(|(_, at)| now.saturating_duration_since(*at) < Duration::from_millis(350))
            .map(|(seconds, _)| Position {
                seconds,
                held: true,
            })
    }
}

fn map(beat: u32, in_bar: u8, (steps, position): (u16, u16), grid: &[f64]) -> Option<f64> {
    if steps == 0 || position > steps || !(1..=4).contains(&in_bar) {
        return None;
    }
    let phase = 4.0 * f64::from(position) / f64::from(steps);
    // Beat zero is before the first grid marker, not necessarily track zero.
    // Nexus can report position == steps at that boundary (e.g. cue 0.200s).
    if beat == 0 {
        let first = *grid.first()?;
        let period = *grid.get(1)? - first;
        return (period > 0.0).then_some((first + (phase - 4.0) * period).max(0.0));
    }
    if phase.floor() as u8 + 1 != in_bar && !(position == steps && in_bar == 4) {
        return None;
    }
    let index = f64::from(beat.checked_sub(u32::from(in_bar))?) + phase;
    let lower = index.floor() as usize;
    let start = *grid.get(lower)?;
    let end = *grid.get(lower + 1)?;
    (end > start).then_some(start + index.fract() * (end - start))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status() -> prolink::monitor::PlayerStatus {
        prolink::monitor::PlayerStatus {
            usb_present: false,
            sd_present: false,
            jogging: false,
            bar_position: Some((2000, 250)),
            active_loop: None,
            beat_in_bar: Some(1),
            beat_number: Some(5),
            reverse: false,
            play_state: prolink::monitor::PlayState(5),
            track: None,
            bpm_centi: Some(12000),
            pitch: None,
            motion_pitch: None,
            forward_transport: false,
            is_tempo_master: true,
            is_synced: false,
            is_playing: false,
            yielding_to: None,
        }
    }
    #[test]
    fn non_master_changed_cue_position_remains_stable() {
        let grid: Vec<_> = (0..20).map(|n| n as f64 * 0.5).collect();
        let mut tracker = BarPosition::default();
        let mut s = status();
        s.is_tempo_master = false;
        let t = Instant::now();
        assert!(tracker.position("a", s, &grid, t, t).is_none());
        s.play_state = prolink::monitor::PlayState(6);
        s.bar_position = Some((2000, 100));
        for ms in [50, 100, 1000] {
            let at = t + Duration::from_millis(ms);
            assert_eq!(
                tracker.position("a", s, &grid, at, at).unwrap().seconds,
                2.1
            );
        }
        s.play_state = prolink::monitor::PlayState(5);
        let at = t + Duration::from_millis(1500);
        assert!(tracker.position("a", s, &grid, at, at).is_none());
    }
    #[test]
    fn packet_times_bound_fine_and_held_positions() {
        let mut s = status();
        let grid: Vec<_> = (0..20).map(|n| f64::from(n) * 0.5).collect();
        let mut tracker = BarPosition::default();
        let base = Instant::now();
        let mut read = |s, received, now| {
            tracker.position(
                "a",
                s,
                &grid,
                base + Duration::from_millis(received),
                base + Duration::from_millis(now),
            )
        };
        assert_eq!(read(s, 0, 0).unwrap().seconds, 2.25);
        s.is_playing = true;
        assert_eq!(read(s, 10, 10), None);
        s.bar_position = Some((2000, 300));
        assert_eq!(
            read(s, 20, 20),
            Some(Position {
                seconds: 2.3,
                held: false
            })
        );
        // A boundary mismatch holds the old fine position instead of rounding back.
        s.bar_position = Some((2000, 550));
        assert_eq!(
            read(s, 100, 100),
            Some(Position {
                seconds: 2.3,
                held: true
            })
        );
        assert_eq!(
            read(s, 100, 250),
            Some(Position {
                seconds: 2.3,
                held: true
            })
        );
        assert_eq!(read(s, 100, 371), None);
    }
    #[test]
    fn stale_phase_is_not_revived_by_pause_and_new_track_resets() {
        let mut s = status();
        s.is_playing = true;
        let grid: Vec<_> = (0..20).map(|n| f64::from(n) * 0.5).collect();
        let mut tracker = BarPosition::default();
        let now = Instant::now();
        assert_eq!(tracker.position("a", s, &grid, now, now), None);
        s.is_playing = false;
        let later = now + Duration::from_millis(500);
        assert_eq!(tracker.position("a", s, &grid, later, later), None);
        s.bar_position = Some((2000, 350));
        let later = later + Duration::from_millis(50);
        assert_eq!(
            tracker
                .position("a", s, &grid, later, later)
                .unwrap()
                .seconds,
            2.35
        );
        s.is_playing = true;
        assert_eq!(tracker.position("b", s, &grid, later, later), None);
        s.is_tempo_master = false;
        assert_eq!(tracker.position("b", s, &grid, later, later), None);
    }
    #[test]
    fn non_master_fine_data_requires_real_coherent_changes_and_expires() {
        let t = Instant::now();
        let grid: Vec<_> = (0..20).map(|n| n as f64 * 0.5).collect();
        let mut tracker = BarPosition::default();
        let mut s = status();
        s.is_tempo_master = false;
        for ms in [0, 50, 100, 150] {
            s.bar_position = Some((if ms % 100 == 0 { 2000 } else { 2001 }, 250));
            let now = t + Duration::from_millis(ms);
            assert!(tracker.position("a", s, &grid, now, now).is_none());
        }
        s.bar_position = Some((2000, 300));
        let at = t + Duration::from_millis(200);
        assert!(tracker.position("a", s, &grid, at, at).is_none());
        s.bar_position = Some((2000, 400));
        let at = t + Duration::from_millis(250);
        assert_eq!(
            tracker.position("a", s, &grid, at, at).unwrap().seconds,
            2.4
        );
        let at = t + Duration::from_millis(650);
        assert!(
            tracker.position("a", s, &grid, at, at).is_none(),
            "frozen phase expires"
        );
    }
    #[test]
    fn pre_grid_cue_uses_phase_instead_of_forcing_zero() {
        let grid = [0.201, 0.681, 1.161];
        assert_eq!(map(0, 4, (2008, 2008), &grid), Some(0.201));
        assert_eq!(map(0, 4, (2008, 1506), &grid), Some(0.0));
        assert_eq!(map(0, 4, (0, 0), &grid), None);
    }
    #[test]
    fn maps_across_bars_and_rejects_inconsistent_boundaries() {
        let grid: Vec<_> = (0..20).map(|n| f64::from(n) * 0.5).collect();
        assert_eq!(map(4, 4, (2000, 1950), &grid), Some(1.95));
        assert_eq!(map(5, 1, (2000, 250), &grid), Some(2.25));
        assert_eq!(map(5, 1, (2000, 600), &grid), None);
        assert_eq!(map(5, 1, (0, 0), &grid), None);
        assert_eq!(map(25, 1, (2000, 250), &grid), None);
    }
}
