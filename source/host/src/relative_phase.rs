//! Relative beat timing refines normal forward non-master playback. Absolute
//! beat identity still comes from BeatPosition; relative phase cannot resolve it.
use crate::live::Asset;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    time::Instant,
};

#[derive(Clone)]
struct Clock {
    key: String,
    pulse: String,
    raw: f64,
    predicted: f64,
    velocity: f64,
    age: f64,
    period: f64,
}
struct Sample {
    at: Instant,
    phase: f64,
    velocity: f64,
}
#[derive(Default)]
struct Pair {
    identity: String,
    pulse: String,
    samples: VecDeque<Sample>,
}
#[derive(Default)]
pub(crate) struct RelativePhase {
    pairs: BTreeMap<u64, Pair>,
}
fn wrap(v: f64) -> f64 {
    (v + 0.5).rem_euclid(1.0) - 0.5
}
fn coordinate(grid: &[f64], seconds: f64) -> Option<(f64, f64)> {
    if !seconds.is_finite() {
        return None;
    }
    let next = grid.partition_point(|v| *v <= seconds);
    if next == 0 || next >= grid.len() {
        return None;
    }
    let period = grid[next] - grid[next - 1];
    (period.is_finite() && period > 0.).then(|| {
        (
            (next - 1) as f64 + (seconds - grid[next - 1]) / period,
            period,
        )
    })
}
fn seconds(grid: &[f64], coordinate: f64) -> Option<f64> {
    if !coordinate.is_finite() || coordinate < 0. {
        return None;
    }
    let i = coordinate.floor() as usize;
    Some(*grid.get(i)? + coordinate.fract() * (grid.get(i + 1)? - grid[i]))
}
fn clock(deck: &Value, assets: &BTreeMap<String, Asset>) -> Option<Clock> {
    if deck["connection"] != "connected"
        || deck["playing"] != true
        || deck["reverse"] == true
        || deck["manualMotion"] == true
        || deck["playState"] == "looping"
        || !deck["loop"].is_null()
        || deck["positionSource"] != "beat-motion"
        || deck["statusAgeMs"].as_f64()? > 500.
    {
        return None;
    }
    let key = deck["trackKey"].as_str()?;
    let grid = &assets.get(key)?.beats;
    let rate = deck["motionRate"].as_f64()?;
    if !rate.is_finite() || rate <= 0. || rate > 4. {
        return None;
    }
    let age = deck["positionAgeMs"].as_f64()? / 1000.;
    let beat_age = deck["beatAgeMs"].as_f64()? / 1000.;
    let raw_seconds = deck["rawBeatPosition"].as_f64()? + age * rate;
    let (raw, period) = coordinate(grid, raw_seconds)?;
    if !(0.0..=1.5 * period / rate).contains(&beat_age) || !(0.0..=1.).contains(&age) {
        return None;
    }
    let (predicted, _) = coordinate(grid, deck["position"].as_f64()? + age * rate)?;
    Some(Clock {
        key: key.into(),
        pulse: deck["beatObservationId"].as_str()?.into(),
        raw,
        predicted,
        velocity: rate / period,
        age,
        period,
    })
}
impl RelativePhase {
    pub(crate) fn refine(
        &mut self,
        decks: &mut [Value],
        assets: &BTreeMap<String, Asset>,
        now: Instant,
    ) {
        let masters: Vec<_> = decks
            .iter()
            .enumerate()
            .filter(|(_, d)| d["master"] == true)
            .collect();
        if masters.len() != 1 {
            self.pairs.clear();
            return;
        }
        let (master_index, master_deck) = masters[0];
        let Some(master) = clock(master_deck, assets) else {
            self.pairs.clear();
            return;
        };
        let master_number = master_deck["number"].as_u64().unwrap_or(0);
        self.pairs
            .retain(|n, _| decks.iter().any(|d| d["number"].as_u64() == Some(*n)));
        for (index, deck) in decks.iter_mut().enumerate() {
            let Some(number) = deck["number"].as_u64() else {
                continue;
            };
            if index == master_index {
                self.pairs.remove(&number);
                continue;
            }
            let Some(slave) = clock(deck, assets) else {
                self.pairs.remove(&number);
                continue;
            };
            let pair = self.pairs.entry(number).or_default();
            let identity = format!("{master_number}:{}:{}", master.key, slave.key);
            if pair.identity != identity
                || pair
                    .samples
                    .back()
                    .is_some_and(|s| now.duration_since(s.at).as_secs_f64() > 1.5 / slave.velocity)
            {
                *pair = Pair {
                    identity,
                    ..Pair::default()
                };
            }
            // Polling the same UDP event never supplies another measurement.
            if pair.pulse != slave.pulse {
                pair.pulse = slave.pulse.clone();
                pair.samples.push_back(Sample {
                    at: now,
                    phase: wrap(slave.raw - master.raw),
                    velocity: slave.velocity - master.velocity,
                });
                while pair.samples.len() > 3 {
                    pair.samples.pop_front();
                }
            }
            if pair.samples.len() < 3 {
                continue;
            }
            let predicted_phase = wrap(slave.predicted - master.predicted);
            let mut errors: Vec<_> = pair
                .samples
                .iter()
                .map(|s| {
                    wrap(
                        s.phase + now.duration_since(s.at).as_secs_f64() * s.velocity
                            - predicted_phase,
                    )
                })
                .collect();
            errors.sort_by(f64::total_cmp);
            // Reject inconsistent timing and large corrections which may indicate
            // seeking or ambiguous beat association. Never snap to beat sync.
            if errors[2] - errors[0] > 0.04 || errors[1].abs() > 0.2 {
                continue;
            }
            let correction = if errors[1].abs() > 0.004 {
                errors[1]
            } else {
                0.
            };
            let grid = &assets[&slave.key].beats;
            let Some(refined_now) = seconds(grid, slave.predicted + correction) else {
                continue;
            };
            let refined_anchor = refined_now - slave.age * slave.velocity * slave.period;
            if refined_anchor < 0. {
                continue;
            }
            deck["position"] = json!(refined_anchor);
            deck["positionSource"] = json!("relative-phase");
            deck["relativePhaseBeats"] = json!(wrap(predicted_phase + correction));
            deck["relativePhaseMaster"] = json!(master_number);
            deck["relativePhaseCorrectionMs"] = json!(correction / slave.velocity * 1000.);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn assets() -> BTreeMap<String, Asset> {
        ["master", "slave"]
            .into_iter()
            .map(|key| {
                (
                    key.into(),
                    Asset {
                        artwork: None,
                        analysis: Value::Null,
                        beats: (0..100).map(|n| n as f64 * 0.5).collect(),
                        warning: None,
                    },
                )
            })
            .collect()
    }
    fn decks(n: u64, behind: f64) -> Vec<Value> {
        // The independent slave tracker is 30 ms ahead of its real beat clock.
        // The true slave beat occurred `behind` seconds after the master beat.
        let mut result = Vec::new();
        for (number, key, delay, bias) in [(1, "master", 0., 0.), (2, "slave", behind, 0.030)] {
            result.push(json!({"number":number,"master":number == 1,"trackKey":key,
                "connection":"connected","playing":true,"reverse":false,"manualMotion":false,
                "loop":null,"positionSource":"beat-motion","positionQuality":"beat",
                "statusAgeMs":10.,"positionAgeMs":100.,"beatAgeMs":(0.1-delay)*1000.,
                "motionRate":1.,"beatAnchorNumber":n+1,"beatObservationId":format!("{n}"),
                "rawBeatPosition":n as f64 * 0.5-delay,
                "position":n as f64 * 0.5-delay+bias}));
        }
        result
    }
    #[test]
    fn corrects_slave_from_three_real_pulses_and_preserves_master_and_age() {
        let mut tracker = RelativePhase::default();
        let assets = assets();
        let t = Instant::now();
        for n in 1..=3 {
            let mut d = decks(n, 0.02);
            let master = d[0].clone();
            tracker.refine(&mut d, &assets, t + Duration::from_millis(n * 500));
            assert_eq!(d[0], master);
            if n < 3 {
                assert_eq!(d[1]["positionSource"], "beat-motion");
            } else {
                assert_eq!(d[1]["positionSource"], "relative-phase");
                assert!((d[1]["position"].as_f64().unwrap() - 1.48).abs() < 1e-9);
                assert!((d[1]["relativePhaseBeats"].as_f64().unwrap() + 0.04).abs() < 1e-9);
                assert_eq!(d[1]["positionAgeMs"], 100.);
            }
        }
    }
    #[test]
    fn repeated_snapshots_do_not_calibrate_and_master_handoff_resets() {
        let mut tracker = RelativePhase::default();
        let assets = assets();
        let t = Instant::now();
        for i in 0..20 {
            let mut d = decks(1, 0.02);
            tracker.refine(&mut d, &assets, t + Duration::from_millis(i));
            assert_eq!(d[1]["positionSource"], "beat-motion");
        }
        for n in 2..=3 {
            tracker.refine(
                &mut decks(n, 0.02),
                &assets,
                t + Duration::from_millis(n * 500),
            );
        }
        let mut d = decks(4, 0.02);
        d[0]["master"] = json!(false);
        d[1]["master"] = json!(true);
        tracker.refine(&mut d, &assets, t + Duration::from_secs(2));
        assert_eq!(d[0]["positionSource"], "beat-motion");
        assert_eq!(d[1]["positionSource"], "beat-motion");
    }
    #[test]
    fn rejects_jitter_then_follows_a_consistent_phase_change() {
        let mut tracker = RelativePhase::default();
        let assets = assets();
        let t = Instant::now();
        for n in 1..=7 {
            let delay = if n <= 2 { 0.02 } else { 0.06 };
            let mut d = decks(n, delay);
            tracker.refine(&mut d, &assets, t + Duration::from_millis(n * 500));
            if n == 3 || n == 4 {
                assert_eq!(d[1]["positionSource"], "beat-motion");
            }
            if n >= 5 {
                assert!(
                    (d[1]["position"].as_f64().unwrap() - (n as f64 * 0.5 - delay)).abs() < 1e-9
                );
            }
        }
    }
    #[test]
    fn stale_paused_looping_reverse_missing_and_ambiguous_masters_fall_back() {
        let assets = assets();
        let t = Instant::now();
        for (index, field, value) in [
            (0, "beatAgeMs", json!(1000.)),
            (1, "playing", json!(false)),
            (1, "loop", json!({"start":0,"end":2})),
            (1, "reverse", json!(true)),
            (1, "playState", json!("looping")),
            (0, "master", json!(false)),
            (1, "master", json!(true)),
            (1, "trackKey", json!("unknown")),
        ] {
            let mut tracker = RelativePhase::default();
            for n in 1..=3 {
                tracker.refine(
                    &mut decks(n, 0.02),
                    &assets,
                    t + Duration::from_millis(n * 500),
                );
            }
            let mut d = decks(4, 0.02);
            d[index][field] = value;
            tracker.refine(&mut d, &assets, t + Duration::from_secs(2));
            assert_eq!(d[1]["positionSource"], "beat-motion");
        }
    }
    #[test]
    fn different_tempos_drift_without_forcing_alignment() {
        let mut tracker = RelativePhase::default();
        let assets = assets();
        let t = Instant::now();
        for n in 1..=3 {
            let elapsed = n as f64 * 0.5;
            let mut d = decks(n, 0.02);
            let raw = n as f64 * 0.5 - 0.02 + elapsed * 0.02;
            d[1]["rawBeatPosition"] = json!(raw);
            d[1]["position"] = json!(raw + 0.03);
            d[1]["motionRate"] = json!(1.02);
            tracker.refine(&mut d, &assets, t + Duration::from_millis(n * 500));
            if n == 3 {
                assert_eq!(d[1]["positionSource"], "relative-phase");
                assert!((d[1]["position"].as_f64().unwrap() - raw).abs() < 1e-9);
                assert!(d[1]["relativePhaseBeats"].as_f64().unwrap() > 0.);
            }
        }
    }
    #[test]
    fn phase_wrap_and_large_errors_keep_absolute_beat_identity() {
        let mut tracker = RelativePhase::default();
        let assets = assets();
        let t = Instant::now();
        for n in 1..=3 {
            let mut d = decks(n, 0.02);
            // Almost a whole beat behind is a small lead modulo one beat.
            d[1]["rawBeatPosition"] = json!(n as f64 * 0.5 - 0.49);
            d[1]["position"] = json!(n as f64 * 0.5 - 0.46);
            tracker.refine(&mut d, &assets, t + Duration::from_millis(n * 500));
            if n == 3 {
                assert!((d[1]["position"].as_f64().unwrap() - 1.01).abs() < 1e-9);
            }
        }
        let mut tracker = RelativePhase::default();
        for n in 1..=3 {
            let mut d = decks(n, 0.02);
            d[1]["position"] = json!(n as f64 * 0.5 + 0.18);
            tracker.refine(&mut d, &assets, t + Duration::from_millis(n * 500));
            assert_eq!(d[1]["positionSource"], "beat-motion");
        }
    }
    #[test]
    fn grid_coordinate_round_trip_supports_variable_tempo() {
        let grid = [0., 0.5, 1.1, 1.8];
        let (beat, period) = coordinate(&grid, 0.8).unwrap();
        assert!((beat - 1.5).abs() < 1e-9);
        assert!((period - 0.6).abs() < 1e-9);
        assert!((seconds(&grid, beat).unwrap() - 0.8).abs() < 1e-9);
    }
}
