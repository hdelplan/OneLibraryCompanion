//! Independent per-player forward position from real beat arrivals and motion speed.
//! No master/sync coupling; paused scratches and loops retain the existing paths.
use prolink::monitor::PlayerStatus;
use prolink_proto::beat::Beat;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct Position {
    /// Position at `at`, not at publication time.
    pub seconds: f64,
    pub rate: f64,
    pub at: Instant,
    pub beat_at: Instant,
    pub beat_number: u32,
    pub correction_seconds: f64,
    pub arrival_residual_seconds: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GridLoop {
    start: f64,
    end: f64,
    first: u32,
    limit: u32,
}
impl GridLoop {
    fn wrap(self, seconds: f64) -> f64 {
        self.start + (seconds - self.start).rem_euclid(self.end - self.start)
    }
    fn is_wrap(self, old: u32, new: u32) -> bool {
        old >= self.limit.saturating_sub(2)
            && old < self.limit
            && new >= self.first
            && new <= self.first + 1
    }
}
#[derive(Default)]
pub struct BeatPosition {
    key: String,
    last_status: Option<Instant>,
    last_pulse: Option<Instant>,
    status: Option<(PlayerStatus, Instant)>,
    consistent: Option<(PlayerStatus, Instant)>,
    relation: Option<u32>,
    candidate: Option<(u32, u8, u32)>,
    position: Option<Position>,
    valid_after: Option<Instant>,
    beat_period: f64,
    phase_evidence: VecDeque<(Instant, f64)>,
    loop_range: Option<GridLoop>,
}
impl BeatPosition {
    /// Enable only observed, whole-beat grid-aligned loops. Unknown/fractional
    /// loops remain on the old conservative path; no endpoints are invented.
    pub fn configure_loop(&mut self, key: &str, region: Option<(f64, f64)>, grid: &[f64]) {
        if key != self.key {
            *self = Self {
                key: key.into(),
                ..Self::default()
            };
        }
        let next = region.and_then(|(start, end)| {
            if !start.is_finite() || !end.is_finite() || end <= start {
                return None;
            }
            let first = grid.binary_search_by(|v| v.total_cmp(&start)).ok()?;
            let limit = grid.binary_search_by(|v| v.total_cmp(&end)).ok()?;
            let size = limit.checked_sub(first)?;
            if size == 0 {
                return None;
            }
            Some(GridLoop {
                start,
                end,
                first: first as u32 + 1,
                limit: limit as u32 + 1,
            })
        });
        if next != self.loop_range {
            self.loop_range = next;
            self.position = None;
            self.phase_evidence.clear();
            self.consistent = None;
            self.valid_after = self.last_status;
        }
    }
    pub fn observe(
        &mut self,
        key: &str,
        mut status: PlayerStatus,
        at: Instant,
        pulse: Option<(Beat, Instant)>,
        grid: &[f64],
        now: Instant,
    ) -> Option<Position> {
        if key != self.key {
            *self = Self {
                key: key.into(),
                ..Self::default()
            };
        }
        let advancing_loop = self.status.is_some_and(|(old, _)| {
            old.play_state.0 == 4
                && (old.forward_transport
                    || old
                        .beat_number
                        .zip(status.beat_number)
                        .is_some_and(|(a, b)| b > a && b <= a.saturating_add(2)))
        });
        status.forward_transport &= matches!(status.play_state.0, 3 | 7)
            || (status.play_state.0 == 4 && (self.loop_range.is_some() || advancing_loop));
        // Desktop snapshots can contain two unseen events. Process their actual
        // receive order; repeated publication must not integrate speed twice.
        if let Some((beat, beat_at)) = pulse.filter(|(_, beat_at)| *beat_at < at) {
            self.pulse(beat, beat_at, grid);
        }
        self.status(status, at);
        // A beat can precede the status that identifies its absolute grid index.
        // Retry only that unanchored event with a closely following coherent status.
        if let Some((beat, beat_at)) = pulse.filter(|(_, t)| {
            *t < at
                && self.position.is_none_or(|p| p.beat_at != *t)
                && at.duration_since(*t) < Duration::from_millis(150)
        }) {
            self.last_pulse = None;
            self.pulse(beat, beat_at, grid);
        }
        if let Some((beat, beat_at)) = pulse.filter(|(_, beat_at)| *beat_at >= at) {
            self.pulse(beat, beat_at, grid);
        }
        let p = self.position?;
        if !status.forward_transport
            || now.saturating_duration_since(at) > Duration::from_secs(1)
            || now.saturating_duration_since(p.beat_at).as_secs_f64()
                > (2.5 * self.beat_period).clamp(0.75, 2.0)
        {
            return None;
        }
        Some(p)
    }
    fn status(&mut self, status: PlayerStatus, at: Instant) {
        if self.last_status.is_some_and(|previous| at <= previous) {
            return;
        }
        let previous = self.status;
        self.last_status = Some(at);
        self.status = Some((status, at));
        let rate = status.motion_pitch.map(|p| p.multiplier());
        let seek = previous.is_some_and(|(old, old_at)| {
            at.duration_since(old_at) > Duration::from_secs(1)
                || old
                    .beat_number
                    .zip(status.beat_number)
                    .is_some_and(|(a, b)| {
                        (b < a && !self.loop_range.is_some_and(|r| r.is_wrap(a, b)))
                            || b > a.saturating_add(2)
                    })
        });
        if !status.forward_transport || rate.is_none_or(|r| !(0.0..=4.0).contains(&r)) || seek {
            self.position = None;
            self.phase_evidence.clear();
            self.consistent = None;
            self.valid_after = Some(at);
        }
        if !status.forward_transport {
            return;
        }
        if let Some(p) = self.position.as_mut()
            && at > p.at
        {
            p.seconds += at.duration_since(p.at).as_secs_f64() * p.rate;
            if let Some(region) = self.loop_range {
                p.seconds = region.wrap(p.seconds);
            }
            p.at = at;
            p.rate = rate.unwrap_or(p.rate);
        }
        let Some((number, within)) = status
            .beat_number
            .zip(status.beat_in_bar)
            .filter(|(n, b)| *n > 0 && *n < u32::MAX && (1..=4).contains(b))
        else {
            return;
        };
        let relation = (number % 4 + 4 - u32::from(within % 4)) % 4;
        if self.relation != Some(relation) {
            // Repeated transitional statuses can establish a wrong startup
            // mapping. Recover only after three distinct beat counts agree;
            // repeated packets within one beat cannot overturn calibration.
            let count = self.candidate.filter(|(r, _, _)| *r == relation).map_or(
                1,
                |(_, count, last_number)| {
                    if self.relation.is_none() || number != last_number {
                        count.saturating_add(1)
                    } else {
                        count
                    }
                },
            );
            self.candidate = Some((relation, count, number));
            let required = if self.relation.is_none() { 2 } else { 3 };
            if count >= required {
                self.relation = Some(relation);
                self.candidate = None;
                self.position = None;
                self.phase_evidence.clear();
                self.consistent = None;
                self.valid_after = Some(at);
            }
        } else {
            self.candidate = None;
        }
        // Hardware can update within-bar before the running beat counter. Do
        // not let such a transitional status move the absolute beat association.
        if self.relation == Some(relation) {
            self.consistent = Some((status, at));
        }
    }
    fn pulse(&mut self, beat: Beat, at: Instant, grid: &[f64]) {
        if self.last_pulse.is_some_and(|previous| at <= previous) {
            return;
        }
        self.last_pulse = Some(at);
        let Some((latest, latest_at)) = self.status else {
            return;
        };
        let Some((status, status_at)) = self.consistent else {
            return;
        };
        let Some(within) = beat.beat_in_bar.map(|b| b.get()) else {
            return;
        };
        let Some(period) = beat.beat_interval().map(|t| t.as_secs_f64()) else {
            return;
        };
        let rate = latest.motion_pitch.unwrap_or(beat.pitch).multiplier();
        if !latest.forward_transport
            || beat.scratching
            || !(0.0..=4.0).contains(&rate)
            || rate == 0.0
            || period <= 0.0
            || self.valid_after.is_some_and(|t| at <= t)
            || (at < latest_at
                && (latest_at.duration_since(at).as_secs_f64() > (period * 0.3).min(0.15)
                    || status.beat_in_bar != Some(within)))
            || at.saturating_duration_since(status_at).as_secs_f64() > (period * 0.75).min(0.5)
        {
            return;
        }
        let Some((number, status_bar)) = status.beat_number.zip(status.beat_in_bar) else {
            return;
        };
        let delta = (within + 4 - status_bar) % 4;
        // A recent coherent status is either already in this beat or still in
        // its predecessor. A larger mismatch is ambiguous (seek/loss/late UDP).
        let returning = self.loop_range.filter(|r| {
            self.relation
                .is_some_and(|relation| within == ((r.first + 3 - relation) % 4 + 1) as u8)
                && number >= r.limit.saturating_sub(2)
                && number < r.limit
        });
        if delta > 1 && returning.is_none() {
            return;
        }
        let Some(mut number) = returning
            .filter(|_| delta > 1)
            .map(|r| r.first)
            .or_else(|| number.checked_add(u32::from(delta)))
        else {
            return;
        };
        if let Some(region) = self.loop_range {
            if number == region.limit {
                number = region.first;
            }
            if number < region.first || number >= region.limit {
                return;
            }
        }
        if self.position.is_some_and(|p| number == p.beat_number)
            && self.loop_range.is_none_or(|r| r.limit - r.first != 1)
        {
            return;
        }
        // One-beat loops legitimately repeat the same beat number. Require a
        // plausible new cycle so duplicate/bunched pulses cannot restart it.
        if self.loop_range.is_some_and(|r| r.limit - r.first == 1)
            && self.position.is_some_and(|p| {
                at.saturating_duration_since(p.beat_at).as_secs_f64() < period * 0.55
            })
        {
            return;
        }
        if self.position.is_some_and(|p| {
            number <= p.beat_number
                && !self
                    .loop_range
                    .is_some_and(|r| r.is_wrap(p.beat_number, number))
        }) {
            return;
        }
        let Some(seconds) = number
            .checked_sub(1)
            .and_then(|i| grid.get(i as usize))
            .copied()
            .filter(|s| s.is_finite() && *s >= 0.0)
        else {
            return;
        };
        // A beat receive time includes unknown delivery delay. Keep a short
        // window of motion-compensated observations and use its least-delayed
        // evidence, not every arrival as an exact phase measurement. Rate
        // changes are still applied on each status; no jog-speed smoothing.
        let (seconds, correction, residual) = if let Some(previous) = self.position {
            let predicted = previous.seconds
                + at.saturating_duration_since(previous.at).as_secs_f64() * previous.rate;
            let predicted = self.loop_range.map_or(predicted, |r| r.wrap(predicted));
            let residual = self.loop_range.map_or(seconds - predicted, |r| {
                let length = r.end - r.start;
                (seconds - predicted + length / 2.0).rem_euclid(length) - length / 2.0
            });
            self.phase_evidence.push_back((at, residual));
            while self.phase_evidence.len() > 8
                || self.phase_evidence.front().is_some_and(|(t, _)| {
                    at.saturating_duration_since(*t).as_secs_f64() > (8.0 * period).clamp(2.0, 6.0)
                })
            {
                self.phase_evidence.pop_front();
            }
            let best = self
                .phase_evidence
                .iter()
                .map(|(_, e)| *e)
                .fold(f64::NEG_INFINITY, f64::max);
            // Ignore millisecond grid quantization. A backward recalibration
            // requires a full window in agreement, rather than one late packet.
            let correction = if best > 0.002 {
                best
            } else if self.phase_evidence.len() == 8 && best < -0.008 {
                best + 0.004
            } else {
                0.0
            };
            for (_, offset) in &mut self.phase_evidence {
                *offset -= correction;
            }
            (
                self.loop_range
                    .map_or(predicted + correction, |r| r.wrap(predicted + correction)),
                correction,
                residual,
            )
        } else {
            self.phase_evidence.clear();
            self.phase_evidence.push_back((at, 0.0));
            (seconds, 0.0, 0.0)
        };
        self.beat_period = period;
        self.position = Some(Position {
            seconds,
            rate,
            at,
            beat_at: at,
            beat_number: number,
            correction_seconds: correction,
            arrival_residual_seconds: residual,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prolink_proto::{
        DeviceName, DeviceNumber,
        beat::{BeatInBar, Pitch, Timings},
        status::CdjStatus,
    };
    fn status(number: u32, within: u8, master: bool, rate: f64) -> PlayerStatus {
        let mut raw = CdjStatus::builder()
            .device_number(DeviceNumber::ONE)
            .play_state(3)
            .playing(true)
            .build()
            .into_bytes();
        raw[0x89] = if master { 0xf4 } else { 0xd4 };
        raw[0x8b] = 0xfa;
        raw[0x9d] = 9;
        raw[0xa0..0xa4].copy_from_slice(&number.to_be_bytes());
        raw[0xa6] = within;
        raw[0x98..0x9c].copy_from_slice(&((rate * 1048576.0) as u32).to_be_bytes());
        PlayerStatus::from_packet(&CdjStatus::parse(&raw).unwrap())
    }
    fn beat(within: u8) -> Beat {
        Beat {
            name: DeviceName::new("CDJ-2000nexus"),
            device: DeviceNumber::ONE,
            timings: Timings::default(),
            pitch: Pitch::UNITY,
            bpm_centi: 12000,
            beat_in_bar: BeatInBar::new(within),
            scratching: false,
        }
    }
    #[test]
    fn beat_before_identifying_status_uses_its_original_arrival_time() {
        let grid: Vec<_> = (0..80).map(|n| n as f64 * 0.5).collect();
        let t = Instant::now();
        let mut tracker = BeatPosition::default();
        for ms in [0, 10] {
            let at = t + Duration::from_millis(ms);
            tracker.observe("a", status(9, 1, false, 1.), at, None, &grid, at);
        }
        // Previous status too old to identify this beat; following status can.
        let pulse_at = t + Duration::from_millis(500);
        let at = pulse_at + Duration::from_millis(60);
        let p = tracker
            .observe(
                "a",
                status(10, 2, false, 1.),
                at,
                Some((beat(2), pulse_at)),
                &grid,
                at,
            )
            .unwrap();
        assert_eq!(p.beat_number, 10);
        assert_eq!(p.at, pulse_at);
        assert_eq!(p.seconds, 4.5);
    }
    #[test]
    fn independent_anchors_and_speed_updates_ignore_master_role_and_status_phase_noise() {
        let t = Instant::now();
        let grid: Vec<_> = (0..40).map(|n| n as f64 * 0.5).collect();
        for master in [true, false] {
            let mut tracker = BeatPosition::default();
            let mut run = |ms, s, b: Option<(Beat, u64)>| {
                let at = t + Duration::from_millis(ms);
                tracker.observe(
                    "a",
                    s,
                    at,
                    b.map(|(b, ms)| (b, t + Duration::from_millis(ms))),
                    &grid,
                    at,
                )
            };
            assert!(run(0, status(10, 2, master, 1.), None).is_none());
            assert!(run(100, status(10, 2, master, 1.), None).is_none());
            let p = run(200, status(10, 2, master, 1.), Some((beat(3), 200))).unwrap();
            assert_eq!(p.seconds, 5.);
            // Transitional status says beat 10, within-bar 3. Integrate speed,
            // but don't use its inconsistent beat number as a phase correction.
            let p = run(300, status(10, 3, master, 1.1), Some((beat(3), 200))).unwrap();
            assert!((p.seconds - 5.1).abs() < 1e-8);
            let p = run(400, status(11, 3, master, 1.1), Some((beat(3), 200))).unwrap();
            assert!((p.seconds - 5.21).abs() < 1e-6);
            let p = run(700, status(11, 3, !master, 1.), Some((beat(4), 700))).unwrap();
            // A late pulse does not pull a valid speed-integrated position back.
            assert!(p.seconds > 5.5 && p.seconds < 5.55);
        }
    }
    #[test]
    fn late_beats_do_not_pull_steady_motion_back_and_speed_changes_are_immediate() {
        let t = Instant::now();
        let grid: Vec<_> = (0..100).map(|n| n as f64 * 0.5).collect();
        let mut tracker = BeatPosition::default();
        for ms in [0, 100] {
            let at = t + Duration::from_millis(ms);
            tracker.observe("a", status(9, 1, false, 1.), at, None, &grid, at);
        }
        for i in 1..=32u32 {
            let delay = [10, 120, 210, 30, 140, 80, 15, 190][(i as usize - 1) % 8];
            let now = t + Duration::from_millis(u64::from(i) * 500 + delay);
            let at = now - Duration::from_millis(5);
            let n = 9 + i;
            let within = ((n - 1) % 4 + 1) as u8;
            let p = tracker
                .observe(
                    "a",
                    status(n, within, false, 1.),
                    at,
                    Some((beat(within), now)),
                    &grid,
                    now,
                )
                .unwrap();
            let truth = 4.0 + f64::from(i) * 0.5 + delay as f64 / 1000.;
            assert!(
                (p.seconds - truth).abs() < 0.022,
                "beat {i}: error {}",
                p.seconds - truth
            );
            assert!(p.correction_seconds.abs() < 0.003);
        }
        let last = tracker.position.unwrap();
        let at = last.at + Duration::from_millis(20);
        let n = last.beat_number;
        let p = tracker
            .observe(
                "a",
                status(n, ((n - 1) % 4 + 1) as u8, false, 1.2),
                at,
                None,
                &grid,
                at,
            )
            .unwrap();
        assert!((p.rate - 1.2).abs() < 0.000001);
        assert!((p.seconds - last.seconds - 0.02).abs() < 0.000001);
    }
    #[test]
    fn whole_bar_loops_keep_independent_phase_through_repeated_wraps() {
        let t = Instant::now();
        let grid: Vec<_> = (0..100).map(|n| n as f64 * 0.5).collect();
        for master in [false, true] {
            let mut tracker = BeatPosition::default();
            tracker.configure_loop("a", Some((4., 8.)), &grid);
            for ms in (0..=16000u64).step_by(50) {
                let n = 9 + ((ms / 500) % 8) as u32;
                let within = ((n - 1) % 4 + 1) as u8;
                let mut s = status(n, within, master, 1.);
                s.play_state = prolink::monitor::PlayState(4);
                let at = t + Duration::from_millis(ms);
                tracker.observe("a", s, at, None, &grid, at);
                if ms % 500 == 0 {
                    let now = at + Duration::from_millis(20);
                    let p = tracker.observe("a", s, at, Some((beat(within), now)), &grid, now);
                    if ms >= 500 {
                        let p = p.expect("loop must retain beat tracking");
                        let expected = 4. + (ms % 4000) as f64 / 1000.;
                        assert!(
                            ((p.seconds - expected + 2.0).rem_euclid(4.0) - 2.0).abs() < 0.003,
                            "{master} {ms}: {} != {expected}",
                            p.seconds
                        );
                        assert_eq!(p.beat_number, n);
                    }
                }
            }
            tracker.configure_loop("a", Some((4., 6.)), &grid);
            assert!(
                tracker.position.is_none(),
                "resize discards previous anchors"
            );
            tracker.configure_loop("a", None, &grid);
            assert!(tracker.loop_range.is_none());
        }
    }
    #[test]
    fn one_and_two_beat_loops_keep_phase_and_ignore_duplicate_pulses() {
        let t = Instant::now();
        let grid: Vec<_> = (0..100).map(|n| n as f64 * 0.5).collect();
        for size in [1u32, 2] {
            let mut tracker = BeatPosition::default();
            tracker.configure_loop("a", Some((4., 4. + size as f64 * 0.5)), &grid);
            for ms in [0, 100] {
                let mut s = status(9, 1, false, 1.);
                s.play_state = prolink::monitor::PlayState(4);
                let at = t + Duration::from_millis(ms);
                tracker.observe("a", s, at, None, &grid, at);
            }
            for i in 1..=20u32 {
                let n = 9 + i % size;
                let within = ((n - 1) % 4 + 1) as u8;
                let mut s = status(n, within, false, 1.);
                s.play_state = prolink::monitor::PlayState(4);
                let pulse_at = t + Duration::from_millis(i as u64 * 500 + 20);
                let at = pulse_at - Duration::from_millis(5);
                let p = tracker
                    .observe("a", s, at, Some((beat(within), pulse_at)), &grid, pulse_at)
                    .unwrap();
                let length = size as f64 * 0.5;
                let expected = 4. + (i % size) as f64 * 0.5;
                assert!(
                    ((p.seconds - expected + length / 2.).rem_euclid(length) - length / 2.).abs()
                        < 0.003
                );
                let duplicate = pulse_at + Duration::from_millis(2);
                let again = tracker
                    .observe(
                        "a",
                        s,
                        at,
                        Some((beat(within), duplicate)),
                        &grid,
                        duplicate,
                    )
                    .unwrap();
                assert_eq!(again.beat_at, p.beat_at, "duplicate must not restart loop");
            }
        }
    }
    #[test]
    fn two_beat_wrap_can_arrive_before_the_status_counter_returns() {
        let t = Instant::now();
        let grid: Vec<_> = (0..40).map(|n| n as f64 * 0.5).collect();
        let mut tracker = BeatPosition::default();
        tracker.configure_loop("a", Some((4., 5.)), &grid);
        let mut s = status(10, 2, false, 1.);
        s.play_state = prolink::monitor::PlayState(4);
        tracker.observe("a", s, t, None, &grid, t);
        let at = t + Duration::from_millis(100);
        tracker.observe("a", s, at, None, &grid, at);
        let now = t + Duration::from_millis(200);
        let p = tracker
            .observe("a", s, at, Some((beat(1), now)), &grid, now)
            .unwrap();
        assert_eq!(p.beat_number, 9);
        assert_eq!(p.seconds, 4.);
    }
    #[test]
    fn unknown_long_loop_scrolls_between_beats_before_first_return() {
        let t = Instant::now();
        let grid: Vec<_> = (0..80).map(|n| n as f64 * 0.5).collect();
        let mut tracker = BeatPosition::default();
        for (ms, n) in [(0, 9), (100, 9), (500, 10), (600, 10)] {
            let mut s = status(n, ((n - 1) % 4 + 1) as u8, false, 1.);
            s.play_state = prolink::monitor::PlayState(4);
            let at = t + Duration::from_millis(ms);
            tracker.observe("a", s, at, None, &grid, at);
        }
        let mut s = status(11, 3, false, 1.);
        s.play_state = prolink::monitor::PlayState(4);
        let at = t + Duration::from_millis(1000);
        let pulse = Some((beat(3), at));
        let p = tracker.observe("a", s, at, pulse, &grid, at).unwrap();
        let later = at + Duration::from_millis(100);
        let q = tracker.observe("a", s, later, pulse, &grid, later).unwrap();
        assert!((q.seconds - p.seconds - 0.1).abs() < 0.000001);
    }
    #[test]
    fn unknown_fractional_loops_never_use_beat_prediction() {
        let t = Instant::now();
        let grid: Vec<_> = (0..100).map(|n| n as f64 * 0.5).collect();
        for region in [None, Some((4., 4.25)), Some((4.1, 4.6))] {
            let mut tracker = BeatPosition::default();
            tracker.configure_loop("a", region, &grid);
            for ms in [0, 100, 500] {
                let mut s = status(9, 1, false, 1.);
                s.play_state = prolink::monitor::PlayState(4);
                let at = t + Duration::from_millis(ms);
                assert!(
                    tracker
                        .observe("a", s, at, Some((beat(1), at)), &grid, at)
                        .is_none()
                );
            }
        }
    }
    #[test]
    fn recovers_wrong_startup_mapping_only_after_distinct_consistent_beats() {
        let t = Instant::now();
        let grid: Vec<_> = (0..80).map(|n| n as f64 * 0.5).collect();
        let mut tracker = BeatPosition::default();
        for ms in [0, 10] {
            let at = t + Duration::from_millis(ms);
            tracker.observe("a", status(33, 1, false, 1.), at, None, &grid, at);
        }
        assert_eq!(tracker.relation, Some(0));
        for (ms, number, bar) in [
            (100, 34, 1),
            (150, 34, 1),
            (200, 34, 1),
            (250, 34, 1),
            (400, 35, 2),
        ] {
            let at = t + Duration::from_millis(ms);
            tracker.observe("a", status(number, bar, false, 1.), at, None, &grid, at);
            assert_eq!(tracker.relation, Some(0));
        }
        let at = t + Duration::from_millis(900);
        tracker.observe("a", status(36, 3, false, 1.), at, None, &grid, at);
        assert_eq!(tracker.relation, Some(1));
        let pulse_at = at + Duration::from_millis(100);
        let p = tracker
            .observe(
                "a",
                status(36, 3, false, 1.),
                at,
                Some((beat(3), pulse_at)),
                &grid,
                pulse_at,
            )
            .unwrap();
        assert_eq!(p.beat_number, 36);
        assert_eq!(p.seconds, 17.5);
        // One transitional mismatch must not undo the recovered calibration.
        let at = pulse_at + Duration::from_millis(10);
        tracker.observe("a", status(36, 4, false, 1.), at, None, &grid, at);
        assert_eq!(tracker.relation, Some(1));
    }
    #[test]
    fn pause_loop_reverse_seek_track_change_and_stale_beats_do_not_reuse_old_anchors() {
        let t = Instant::now();
        let grid: Vec<_> = (0..40).map(|n| n as f64 * 0.5).collect();
        for mode in ["pause", "loop", "reverse", "seek", "track", "stale"] {
            let mut tracker = BeatPosition::default();
            tracker.observe("a", status(10, 2, false, 1.), t, None, &grid, t);
            tracker.observe(
                "a",
                status(10, 2, false, 1.),
                t + Duration::from_millis(100),
                None,
                &grid,
                t + Duration::from_millis(100),
            );
            let pulse = Some((beat(3), t + Duration::from_millis(200)));
            assert!(
                tracker
                    .observe(
                        "a",
                        status(11, 3, false, 1.),
                        t + Duration::from_millis(250),
                        pulse,
                        &grid,
                        t + Duration::from_millis(250)
                    )
                    .is_some()
            );
            let mut s = status(11, 3, false, 1.);
            if matches!(mode, "pause" | "loop" | "reverse") {
                s.forward_transport = false;
            }
            if mode == "seek" {
                s.beat_number = Some(2);
                s.beat_in_bar = Some(2);
            }
            let now = t + Duration::from_millis(if mode == "stale" { 1800 } else { 300 });
            assert!(
                tracker
                    .observe(
                        if mode == "track" { "b" } else { "a" },
                        s,
                        now,
                        pulse,
                        &grid,
                        now
                    )
                    .is_none(),
                "{mode}"
            );
        }
    }
}
