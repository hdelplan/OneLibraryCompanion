//! Observed Sync flag pulses, not physical button press detection.
use serde_json::{Value, json};
#[derive(Default)]
pub struct SyncTap {
    previous: Option<bool>,
    on_at: Option<f64>,
    edges: usize,
    pulses: usize,
    short_pulses: usize,
}
impl SyncTap {
    pub fn observe(&mut self, sync: bool, ms: f64) -> Option<Value> {
        let previous = self.previous.replace(sync);
        if previous == Some(sync) {
            return None;
        }
        if previous.is_none() {
            self.on_at = None;
            return None;
        }
        self.edges += 1;
        if sync {
            self.on_at = Some(ms);
            Some(json!({"phase":"sync-edge","player":2,"sync":true}))
        } else {
            let duration = self.on_at.take().map(|start| ms - start);
            if let Some(duration) = duration {
                self.pulses += 1;
                if (0.0..=700.0).contains(&duration) {
                    self.short_pulses += 1;
                }
            }
            Some(json!({"phase":"sync-edge","player":2,"sync":false,
                "observedOnMs":duration,"shortPulseCandidate":duration.map(|d|(0.0..=700.0).contains(&d)),
                "interpretation":"Observed flag changes only; not proof of two physical button presses."}))
        }
    }
    pub fn summary(&self) -> Value {
        json!({"edges":self.edges,"pulses":self.pulses,"shortPulseCandidates":self.short_pulses,
            "candidateMaxMs":700,"sync":self.previous,
            "interpretation":"Receiver-observed OFF/ON/OFF transitions; button presses can be missed between status packets."})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_flags_and_missing_middle_state_are_not_double_taps() {
        let mut t = SyncTap::default();
        assert!(t.observe(false, 0.0).is_none());
        assert!(t.observe(false, 200.0).is_none());
        assert_eq!(t.summary()["pulses"], 0);
        t.observe(true, 300.0);
        assert!(t.observe(true, 400.0).is_none());
        let end = t.observe(false, 550.0).unwrap();
        assert_eq!(end["observedOnMs"], 250.0);
        assert_eq!(end["shortPulseCandidate"], true);
        assert_eq!(t.summary()["edges"], 2);
        t.observe(true, 1000.0);
        t.observe(false, 2100.0);
        assert_eq!(t.summary()["pulses"], 2);
        assert_eq!(t.summary()["shortPulseCandidates"], 1);
    }
    #[test]
    fn starting_on_does_not_invent_an_onset() {
        let mut t = SyncTap::default();
        t.observe(true, 0.0);
        assert!(t.observe(false, 100.0).unwrap()["observedOnMs"].is_null());
        assert_eq!(t.summary()["pulses"], 0);
    }
}
