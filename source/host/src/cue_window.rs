//! Opt-in cue telemetry experiment. Pure scheduler; direct_live owns UDP sends.
use prolink_proto::status::CdjStatus;
use serde_json::{Value, json};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

const DEFAULT_DURATION: u64 = 15_000;
const LOAD_WAIT: u64 = 60_000;
const TIMEOUT: u64 = 1500;
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Acquire,
    Hold,
    Restore,
}
#[derive(Clone)]
pub struct Deck {
    pub status: CdjStatus,
    pub age: u64,
    pub pending: bool,
}
impl Deck {
    fn key(&self) -> (u32, u8, u8) {
        (
            self.status.track_id(),
            self.status.source_player().map(|n| n.get()).unwrap_or(0),
            self.status.source_slot().0,
        )
    }
    fn synced(&self) -> bool {
        self.status
            .flags()
            .is_none_or(|f| f.is_synced() || f.0 & 2 != 0)
    }
    fn master(&self) -> bool {
        self.status.is_tempo_master() == Some(true)
            && self.status.flags().is_some_and(|f| f.is_tempo_master())
            && self.status.yielding_to().is_none()
    }
    fn paused(&self) -> bool {
        self.status.flags().is_some_and(|f| !f.is_playing())
            && matches!(self.status.play_state(), Some(4 | 5 | 6 | 8 | 9))
    }
    fn playing(&self) -> bool {
        self.status.flags().is_some_and(|f| f.is_playing())
            && matches!(self.status.play_state(), Some(3 | 4))
    }
}
struct Window {
    target: usize,
    original: usize,
    duration: u64,
    phase: Phase,
    since: u64,
    keys: [(u32, u8, u8); 2],
    attempts: u8,
}
struct Engine {
    enabled: bool,
    lease: Option<u64>,
    duration: u64,
    recovery: Option<usize>,
    loading: [bool; 2],
    previous: [Option<(u32, u8, u8)>; 2],
    sync: [Option<bool>; 2],
    on: [Option<u64>; 2],
    pending_load: [Option<u64>; 2],
    window: Option<Window>,
    message: String,
}
#[derive(Debug, PartialEq)]
enum Action {
    Start(usize, &'static str),
    Command(usize),
    Event(&'static str),
    Finish(bool),
}
impl Default for Engine {
    fn default() -> Self {
        Self {
            enabled: true,
            lease: None,
            duration: DEFAULT_DURATION,
            recovery: None,
            loading: [false; 2],
            previous: [None; 2],
            sync: [None; 2],
            on: [None; 2],
            pending_load: [None; 2],
            window: None,
            message: "Cue mode enabled; waiting for connected players.".into(),
        }
    }
}
impl Engine {
    fn arm(&mut self, now: u64) {
        self.enabled = true;
        self.lease = Some(now);
        self.previous = [None; 2];
        self.sync = [None; 2];
        self.on = [None; 2];
        self.pending_load = [None; 2];
        self.message =
            "Armed: load a new track or double-tap SYNC on the paused non-master.".into();
    }
    fn step(
        &mut self,
        now: u64,
        decks: &[Option<Deck>; 2],
        capture_available: bool,
    ) -> Vec<Action> {
        let mut out = vec![];
        let foreground = self.lease.is_some_and(|t| now.saturating_sub(t) <= 2500);
        let valid = decks.iter().enumerate().all(|(i, d)| {
            d.as_ref().is_some_and(|d| {
                d.age <= 1000 && d.status.sender().map(|n| n.get()) == Some(i as u8 + 1)
            })
        });
        let sync_off = valid && decks.iter().all(|d| !d.as_ref().unwrap().synced());
        let sole = |n: usize| {
            valid
                && decks[n].as_ref().unwrap().master()
                && decks[1 - n].as_ref().unwrap().status.is_tempo_master() == Some(false)
                && decks[1 - n]
                    .as_ref()
                    .unwrap()
                    .status
                    .yielding_to()
                    .is_none()
        };
        // Keep track history through loading/status gaps. Only Sync edges need continuous observation.
        let mut triggers = [None; 2];
        for i in 0..2 {
            let Some(d) = decks[i].as_ref().filter(|d| d.age <= 1000) else {
                self.sync[i] = None;
                self.on[i] = None;
                continue;
            };
            let key = d.key();
            if self.previous[i].is_some() && (key.0 == 0 || d.status.play_state() == Some(2)) {
                self.loading[i] = true;
            }
            if key.0 != 0 && key.1 != 0 {
                if self.previous[i].is_some_and(|p| p != key) || self.loading[i] {
                    self.pending_load[i] = Some(now);
                }
                self.previous[i] = Some(key);
                self.loading[i] = false;
            } else if self.previous[i].is_none() {
                self.previous[i] = Some(key);
            }
            let sync = d.synced();
            if self.sync[i] == Some(false) && sync {
                self.on[i] = Some(now);
            }
            if self.sync[i] == Some(true)
                && !sync
                && self.on[i].take().is_some_and(|on| now - on <= 700)
            {
                triggers[i] = Some("sync-double-tap");
            }
            self.sync[i] = Some(sync);
            if let Some(at) = self.pending_load[i] {
                if now - at > LOAD_WAIT {
                    self.pending_load[i] = None;
                } else if triggers[i].is_none() {
                    triggers[i] = Some("track-load");
                }
            }
        }
        if let Some(w) = &mut self.window {
            let original_ok =
                sync_off && decks[w.original].as_ref().unwrap().key() == w.keys[w.original];
            // Brief missing status is not a user disabling the feature. Wait up to 3 s,
            // but never send a command until both players are fresh again.
            let prolonged_gap = decks
                .iter()
                .any(|d| d.as_ref().is_none_or(|d| d.age > 3000));
            let reason = if !self.enabled {
                Some("Disabled by user")
            } else if !foreground {
                Some("App backgrounded or foreground connection lost")
            } else if prolonged_gap {
                Some("Player status unavailable")
            } else if valid && !sync_off {
                Some("SYNC enabled")
            } else if valid && decks[w.original].as_ref().unwrap().key() != w.keys[w.original] {
                Some("Original master track changed")
            } else if valid && decks[w.target].as_ref().unwrap().key() != w.keys[w.target] {
                Some("Cueing track changed")
            } else if valid
                && (!(decks[w.target].as_ref().unwrap().paused()
                    || decks[w.target].as_ref().unwrap().status.play_state() == Some(7))
                    || !decks[w.original].as_ref().unwrap().playing())
            {
                Some("Playback state changed")
            } else if w.phase == Phase::Hold && now - w.since >= w.duration {
                Some("Duration complete")
            } else {
                None
            };
            if let Some(reason) = reason
                && w.phase != Phase::Restore
            {
                self.message = format!("{reason}; returning master to CDJ{}.", w.original + 1);
                w.phase = Phase::Restore;
                w.since = now;
                w.attempts = 0;
                out.push(Action::Event("restoring-at-stop"));
            }
            match w.phase {
                Phase::Acquire => {
                    if sole(w.target)
                        && decks
                            .iter()
                            .all(|d| now.saturating_sub(d.as_ref().unwrap().age) > w.since)
                    {
                        w.phase = Phase::Hold;
                        w.since = now;
                        self.message = format!(
                            "CDJ{} master for {} seconds.",
                            w.target + 1,
                            w.duration / 1000
                        );
                        out.push(Action::Event("target-master-confirmed"));
                    } else if now - w.since >= TIMEOUT {
                        w.phase = Phase::Restore;
                        w.since = now;
                        w.attempts = 0;
                        self.message="Master request not confirmed; returning original master. Mode remains enabled.".into();
                    }
                }
                Phase::Hold => {
                    if valid && !sole(w.target) {
                        w.phase = Phase::Restore;
                        w.since = now;
                        w.attempts = 0;
                        self.message = "Master role changed; returning original master.".into();
                    }
                }
                Phase::Restore => {
                    if sole(w.original)
                        && decks
                            .iter()
                            .all(|d| now.saturating_sub(d.as_ref().unwrap().age) > w.since)
                    {
                        out.push(Action::Event("original-master-confirmed"));
                        out.push(Action::Finish(true));
                        self.window = None;
                        return out;
                    } else if now - w.since >= TIMEOUT {
                        self.recovery = Some(w.original);
                        self.message = format!(
                            "Press CDJ{} MASTER manually. Mode remains enabled; waiting for restoration.",
                            w.original + 1
                        );
                        out.push(Action::Finish(false));
                        self.window = None;
                        return out;
                    }
                }
            }
            if w.phase == Phase::Restore
                && original_ok
                && (w.attempts == 0 || (w.attempts == 1 && now - w.since >= 500))
                && !sole(w.original)
            {
                w.attempts += 1;
                out.push(Action::Command(w.original));
            }
            return out;
        }
        if let Some(original) = self.recovery {
            if sync_off && sole(original) {
                self.recovery = None;
                self.message = "Original master restored; cue mode ready.".into();
            } else {
                return out;
            }
        }
        if !self.enabled || !foreground || !capture_available {
            self.pending_load = [None; 2];
            return out;
        }
        if !sync_off {
            return out;
        }
        for target in 0..2 {
            let Some(trigger) = triggers[target] else {
                continue;
            };
            let original = 1 - target;
            let t = decks[target].as_ref().unwrap();
            let o = decks[original].as_ref().unwrap();
            if !t.paused()
                || !o.playing()
                || !sole(original)
                || decks.iter().any(|d| {
                    let d = d.as_ref().unwrap();
                    d.pending || d.key().0 == 0 || d.key().1 == 0 || d.status.track_type() != 1
                })
            {
                continue;
            }
            self.pending_load[target] = None;
            self.window = Some(Window {
                target,
                original,
                duration: self.duration,
                phase: Phase::Acquire,
                since: now,
                keys: [
                    decks[0].as_ref().unwrap().key(),
                    decks[1].as_ref().unwrap().key(),
                ],
                attempts: 0,
            });
            self.message = format!("Requesting CDJ{} MASTER for cueing.", target + 1);
            out.push(Action::Start(target, trigger));
            out.push(Action::Command(target));
            break;
        }
        out
    }
}
struct Runtime {
    engine: Engine,
    start: Instant,
    reports: Vec<Value>,
}
static RUNTIME: LazyLock<Mutex<Runtime>> = LazyLock::new(|| {
    Mutex::new(Runtime {
        engine: Engine::default(),
        start: Instant::now(),
        reports: vec![],
    })
});
fn now(r: &Runtime) -> u64 {
    r.start.elapsed().as_millis() as u64
}
pub fn occupied() -> bool {
    let r = RUNTIME.lock().unwrap();
    r.engine.window.is_some()
}
pub fn state() -> Value {
    let r = RUNTIME.lock().unwrap();
    let e = &r.engine;
    json!({"enabled":e.enabled,"active":e.window.is_some(),"message":e.message,"remainingMs":e.window.as_ref().map(|w|if w.phase==Phase::Hold {(w.since+w.duration).saturating_sub(now(&r))} else if w.phase==Phase::Restore {0} else {w.duration}),"target":e.window.as_ref().map(|w|w.target+1),"original":e.window.as_ref().map(|w|w.original+1),"phase":e.window.as_ref().map(|w|format!("{:?}",w.phase)),"reports":r.reports.len(),"durationSeconds":e.duration/1000})
}
pub fn control(action: &str, duration: Option<u64>, enabled: Option<bool>) -> Result<(), String> {
    if !crate::EXPERIMENTS {
        return Err("Experiments are unavailable in this build".into());
    }
    let mut r = RUNTIME.lock().unwrap();
    let at = now(&r);
    if let Some(seconds) = duration {
        if ![5, 10, 15, 20, 25, 30].contains(&seconds) {
            return Err("Choose 5, 10, 15, 20, 25 or 30 seconds".into());
        }
        r.engine.duration = seconds * 1000;
    }
    match action {
        "arm" => {
            if r.engine.window.is_some() {
                return Err("Wait for master restoration".into());
            }
            r.engine.arm(at);
        }
        "heartbeat" => {
            r.engine.lease = Some(at);
        }
        "preferences" => {
            if let Some(enabled) = enabled {
                r.engine.enabled = enabled;
            }
            r.engine.lease = Some(at);
            if r.engine.window.is_none() {
                r.engine.message = if r.engine.enabled {
                    "Cue mode enabled; waiting for a new track or SYNC double tap."
                } else {
                    "Cue mode disabled."
                }
                .into();
            }
        }
        "configure" => {}
        "suspend" => {
            r.engine.lease = None;
        }
        "disarm" => {
            r.engine.enabled = false;
            r.engine.message =
                "Disabled; any active window will restore the original master.".into();
        }
        "clear" if r.engine.window.is_none() => r.reports.clear(),
        _ => return Err("Unknown action or experiment still armed".into()),
    }
    Ok(())
}
pub fn reports() -> Value {
    json!({"version":1,"windows":RUNTIME.lock().unwrap().reports})
}
pub fn connection_closed() {
    let mut r = RUNTIME.lock().unwrap();
    r.engine.lease = None;
    if let Some(w) = r.engine.window.take() {
        r.engine.recovery = Some(w.original);
        r.engine.message = format!(
            "Connection closed. Press CDJ{} MASTER manually.",
            w.original + 1
        );
        crate::jog_trace::event(json!({"phase":"connection-closed","restored":false}));
        crate::jog_trace::command(false);
        let mut report = crate::jog_trace::report();
        report["session"] = crate::jog_trace::state()["session"].clone();
        report["experiment"]["outcome"] = json!("connection-closed");
        r.reports.push(report);
        if r.reports.len() > 3 {
            r.reports.remove(0);
        }
    }
}
/// Called only by the shared direct-IP session; returns unicast destination player numbers.
pub fn tick(decks: [Option<Deck>; 2], receivers: bool) -> Vec<u8> {
    if !crate::EXPERIMENTS {
        return vec![];
    }
    let mut r = RUNTIME.lock().unwrap();
    let at = now(&r);
    let trace = crate::jog_trace::state();
    let available = receivers
        && if r.engine.window.is_some() {
            true
        } else {
            trace["active"] != true
        };
    let actions = r.engine.step(at, &decks, available);
    let mut commands = vec![];
    for action in actions {
        match action {
            Action::Start(target, trigger) => {
                crate::jog_trace::command(true);
                crate::jog_trace::experiment(
                    json!({"profile":"cue-window","revision":2,"pattern":"continuous-master","durationMs":r.engine.window.as_ref().unwrap().duration,"target":target+1,"original":2-target,"trigger":trigger,"outcome":"recording","initialPlayers":decks.iter().map(|d|d.as_ref().map(|d|crate::handoff_capture::evidence(&d.status))).collect::<Vec<_>>() }),
                );
            }
            Action::Command(target) => commands.push(target as u8 + 1),
            Action::Event(phase) => crate::jog_trace::event(json!({"phase":phase})),
            Action::Finish(restored) => {
                crate::jog_trace::event(
                    json!({"phase":"window-finished","restored":restored,"message":r.engine.message}),
                );
                crate::jog_trace::command(false);
                let mut report = crate::jog_trace::report();
                report["session"] = crate::jog_trace::state()["session"].clone();
                report["experiment"]["outcome"] = json!(if restored {
                    "restored"
                } else {
                    "restore-unconfirmed"
                });
                r.reports.push(report);
                if restored && r.engine.enabled {
                    r.engine.message = format!(
                        "{} Original master confirmed; ready for next trigger.",
                        r.engine.message
                    );
                }
                if r.reports.len() > 3 {
                    r.reports.remove(0);
                }
            }
        }
    }
    commands
}
pub fn send_failed(error: String) {
    let mut r = RUNTIME.lock().unwrap();
    let at = now(&r);
    if let Some(w) = &mut r.engine.window {
        w.since = at;
        w.phase = Phase::Restore;
        w.attempts = 0;
    }
    r.engine.message = format!("Master command failed: {error}. Restoring if possible.");
    crate::jog_trace::event(json!({"phase":"send-failed","error":error}));
}
/// Pro DJ Link 0x2a / 0x01: appoint recipient master; never enable Sync.
/// Layout verified against Beat Link VirtualCdj.sendSyncControlCommand / Util.buildPacket.
pub fn master_packet() -> [u8; 44] {
    let mut b = [0; 44];
    b[..10].copy_from_slice(b"Qspt1WmJOL");
    b[10] = 0x2a;
    b[11..27].copy_from_slice(b"OLC\0\0\0\0\0\0\0\0\0\0\0\0\0");
    b[31..].copy_from_slice(&[1, 0, 15, 0, 8, 0, 0, 0, 15, 0, 0, 0, 1]);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use prolink_proto::{DeviceNumber, Slot, status::LoadedTrack};
    fn deck(n: u8, master: bool, playing: bool, track: u32, sync: bool) -> Option<Deck> {
        Some(Deck {
            status: CdjStatus::builder()
                .device_number(DeviceNumber::new(n).unwrap())
                .loaded_track(Some(LoadedTrack {
                    source_player: DeviceNumber::new(1).unwrap(),
                    slot: Slot::USB,
                    id: track,
                }))
                .play_state(if playing { 3 } else { 5 })
                .playing(playing)
                .tempo_master(master)
                .synced(sync)
                .build(),
            age: 0,
            pending: false,
        })
    }
    fn pair() -> [Option<Deck>; 2] {
        [
            deck(1, true, true, 11, false),
            deck(2, false, false, 22, false),
        ]
    }
    fn step(e: &mut Engine, t: u64, d: &[Option<Deck>; 2]) -> Vec<Action> {
        e.lease = Some(t);
        e.step(t, d, true)
    }
    fn trigger() -> (Engine, [Option<Deck>; 2]) {
        let mut e = Engine::default();
        let mut d = pair();
        assert!(step(&mut e, 1, &d).is_empty());
        d[1] = deck(2, false, false, 23, false);
        assert_eq!(
            step(&mut e, 10, &d),
            vec![Action::Start(1, "track-load"), Action::Command(1)]
        );
        (e, d)
    }
    fn target_master(d: &mut [Option<Deck>; 2]) {
        d[0] = deck(1, false, true, 11, false);
        d[1] = deck(2, true, false, 23, false);
    }
    #[test]
    fn defaults_enabled_fifteen_seconds_but_needs_foreground() {
        let mut e = Engine::default();
        assert!(e.enabled);
        assert_eq!(e.duration, 15000);
        let mut d = pair();
        e.step(1, &d, true);
        d[1] = deck(2, false, false, 23, false);
        assert!(e.step(10, &d, true).is_empty());
    }
    #[test]
    fn full_continuous_duration_starts_after_confirmation_for_every_option() {
        for seconds in [5, 10, 15, 20, 25, 30] {
            let (mut e, mut d) = trigger();
            e.window.as_mut().unwrap().duration = seconds * 1000;
            assert!(step(&mut e, 100, &d).is_empty());
            target_master(&mut d);
            assert_eq!(
                step(&mut e, 200, &d),
                vec![Action::Event("target-master-confirmed")]
            );
            for t in (250..200 + seconds * 1000).step_by(50) {
                assert!(step(&mut e, t, &d).is_empty());
            }
            assert!(step(&mut e, 200 + seconds * 1000, &d).contains(&Action::Command(0)));
            d[0] = deck(1, true, true, 11, false);
            d[1] = deck(2, false, false, 23, false);
            assert!(step(&mut e, 300 + seconds * 1000, &d).contains(&Action::Finish(true)));
            assert!(e.enabled);
        }
    }
    #[test]
    fn duration_change_does_not_change_active_hold() {
        let (mut e, _) = trigger();
        e.duration = 30000;
        assert_eq!(e.window.unwrap().duration, 15000);
    }
    #[test]
    fn track_load_survives_status_gap_and_waits_for_pending_load() {
        let mut e = Engine::default();
        let mut d = pair();
        step(&mut e, 1, &d);
        d[1].as_mut().unwrap().age = 2000;
        assert!(step(&mut e, 2001, &d).is_empty());
        d[1] = deck(2, false, false, 23, false);
        d[1].as_mut().unwrap().pending = true;
        assert!(step(&mut e, 2100, &d).is_empty());
        d[1].as_mut().unwrap().pending = false;
        assert!(step(&mut e, 2400, &d).contains(&Action::Start(1, "track-load")));
    }
    #[test]
    fn load_detected_even_while_other_player_stale() {
        let mut e = Engine::default();
        let mut d = pair();
        step(&mut e, 1, &d);
        d[0].as_mut().unwrap().age = 1100;
        d[1] = deck(2, false, false, 23, false);
        assert!(step(&mut e, 1200, &d).is_empty());
        d[0].as_mut().unwrap().age = 0;
        assert!(step(&mut e, 1300, &d).contains(&Action::Start(1, "track-load")));
    }
    #[test]
    fn same_track_reload_detected_through_empty_state() {
        let mut e = Engine::default();
        let mut d = pair();
        step(&mut e, 1, &d);
        d[1] = deck(2, false, false, 0, false);
        step(&mut e, 100, &d);
        d[1] = deck(2, false, false, 22, false);
        assert!(step(&mut e, 200, &d).contains(&Action::Start(1, "track-load")));
    }
    #[test]
    fn sync_on_aborts_hold_without_disabling_mode_or_sending_commands() {
        let (mut e, mut d) = trigger();
        target_master(&mut d);
        step(&mut e, 100, &d);
        d[0] = deck(1, false, true, 11, true);
        assert!(
            !step(&mut e, 200, &d)
                .iter()
                .any(|a| matches!(a, Action::Command(_)))
        );
        assert_eq!(step(&mut e, 1700, &d), vec![Action::Finish(false)]);
        assert!(e.enabled);
        assert_eq!(e.recovery, Some(0));
    }
    #[test]
    fn foreground_loss_restores_but_mode_stays_enabled() {
        let (mut e, mut d) = trigger();
        target_master(&mut d);
        step(&mut e, 100, &d);
        assert!(e.step(2601, &d, true).contains(&Action::Command(0)));
        assert!(e.enabled);
    }
    #[test]
    fn brief_stale_status_during_hold_does_not_end_window() {
        let (mut e, mut d) = trigger();
        target_master(&mut d);
        step(&mut e, 100, &d);
        d[1].as_mut().unwrap().age = 1500;
        assert!(step(&mut e, 2000, &d).is_empty());
        assert_eq!(e.window.unwrap().phase, Phase::Hold);
    }
    #[test]
    fn timeout_restores_and_preserves_enabled_setting() {
        let (mut e, d) = trigger();
        assert!(!step(&mut e, 1510, &d).contains(&Action::Command(1)));
        assert!(step(&mut e, 1511, &d).contains(&Action::Finish(true)));
        assert!(e.enabled);
    }
    #[test]
    fn user_disable_is_retained() {
        let (mut e, mut d) = trigger();
        target_master(&mut d);
        step(&mut e, 100, &d);
        e.enabled = false;
        assert!(step(&mut e, 200, &d).contains(&Action::Command(0)));
        assert!(!e.enabled);
    }
    #[test]
    fn more_than_three_windows_do_not_disable_mode() {
        let mut e = Engine::default();
        let mut d = pair();
        step(&mut e, 1, &d);
        for i in 0..5 {
            let t = 100 + i * 20000;
            let track = 23 + i as u32;
            d[1] = deck(2, false, false, track, false);
            assert!(step(&mut e, t, &d).contains(&Action::Command(1)));
            d[0] = deck(1, false, true, 11, false);
            d[1] = deck(2, true, false, track, false);
            step(&mut e, t + 100, &d);
            assert!(step(&mut e, t + 15100, &d).contains(&Action::Command(0)));
            d[0] = deck(1, true, true, 11, false);
            d[1] = deck(2, false, false, track, false);
            assert!(step(&mut e, t + 15200, &d).contains(&Action::Finish(true)));
            assert!(e.enabled);
        }
    }
    #[test]
    fn reverse_target_sync_gesture_needs_observed_on_then_off() {
        let mut e = Engine::default();
        let mut d = [
            deck(1, false, false, 11, false),
            deck(2, true, true, 22, false),
        ];
        step(&mut e, 1, &d);
        assert!(step(&mut e, 50, &d).is_empty());
        d[0] = deck(1, false, false, 11, true);
        assert!(step(&mut e, 100, &d).is_empty());
        d[0] = deck(1, false, false, 11, false);
        assert!(step(&mut e, 350, &d).contains(&Action::Start(0, "sync-double-tap")));
    }
    #[test]
    fn packet_only_appoints_master() {
        let p = master_packet();
        assert_eq!(&p[..11], b"Qspt1WmJOL\x2a");
        assert_eq!(&p[31..], &[1, 0, 15, 0, 8, 0, 0, 0, 15, 0, 0, 0, 1]);
    }
}
