//! Opt-in, bounded in-memory diagnostics. No network control or disk writes.
use serde_json::{Value, json};
use std::{
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};
#[derive(Default)]
struct Trace {
    start: Option<Instant>,
    session: String,
    samples: Vec<Value>,
    ids: std::collections::BTreeMap<u64, String>,
    receivers: Value,
    raw_bytes: usize,
    experiment: Value,
    events: Vec<Value>,
    sync_tap: crate::sync_tap::SyncTap,
    work_ms: Vec<f64>,
    wake_ms: Vec<f64>,
}
static TRACE: LazyLock<Mutex<Trace>> = LazyLock::new(|| Mutex::new(Trace::default()));
fn active(t: &Trace) -> bool {
    t.start
        .is_some_and(|s| s.elapsed() < Duration::from_secs(45))
        && t.samples.len() < 12000
        && t.raw_bytes < 8 * 1024 * 1024
}
pub fn command(start: bool) -> Value {
    let mut t = TRACE.lock().unwrap();
    if start {
        *t = Trace {
            start: Some(Instant::now()),
            session: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
            ..Default::default()
        };
    } else {
        finish_handoff(
            &mut t,
            "stopped",
            "Capture stopped. Restore CDJ1 MASTER manually, then save the report.",
        );
        t.start = None;
    }
    json!({"active":active(&t),"session":t.session,"limitSeconds":45})
}
pub fn state() -> Value {
    let t = TRACE.lock().unwrap();
    json!({"active":active(&t),"session":t.session,"profile":t.experiment["profile"],"message":t.experiment["message"],"players":t.experiment["players"],"syncTap":t.experiment["syncTap"],"remainingMs":t.start.map(|s|45000u128.saturating_sub(s.elapsed().as_millis())).unwrap_or(0)})
}
pub fn bridge_active() -> bool {
    let t = TRACE.lock().unwrap();
    active(&t) && t.experiment["profile"] == "bridge-c0"
}

#[cfg(test)]
pub fn handoff_active() -> bool {
    let t = TRACE.lock().unwrap();
    active(&t) && t.experiment["profile"] == "manual-master-handoff"
}
pub fn sync_tap_active() -> bool {
    let t = TRACE.lock().unwrap();
    active(&t) && t.experiment["profile"] == "sync-double-tap"
}
fn guided(t: &Trace) -> bool {
    matches!(
        t.experiment["profile"].as_str(),
        Some("manual-master-handoff" | "sync-double-tap")
    )
}
pub fn guided_active() -> bool {
    let t = TRACE.lock().unwrap();
    active(&t) && guided(&t)
}
pub fn start_sync_tap(players: [Value; 2]) {
    command(true);
    let mut t = TRACE.lock().unwrap();
    t.sync_tap.observe(false, 0.0); // Preflight requires target Sync OFF.
    t.experiment = json!({"profile":"sync-double-tap","revision":1,"control":"manual-buttons-only",
        "durationMs":45000,"initialPlayers":players,"players":players,"outcome":"recording",
        "syncTap":t.sync_tap.summary(),
        "notes":["Normal observation only: no master, Sync or play commands.",
            "CDJ1 stays playing/master with Sync OFF; CDJ2 stays paused and non-master.",
            "5–10 s: one slow OFF/ON/OFF pair, about 1 s between presses.",
            "15–20, 25–30, 35–40 s: one quick double tap in each window; otherwise wait.",
            "Return CDJ2 Sync OFF between attempts. Corrections are also recorded as flag changes.",
            "Short observed pulses are candidates only, not proof of double taps or a trigger for mastership.",
            "Status sampling can miss a brief ON state; video can establish actual button presses."]});
}
pub fn start_handoff(players: [Value; 2]) {
    command(true);
    experiment(
        json!({"profile":"manual-master-handoff","control":"manual-buttons-only",
        "durationMs":45000,"initialPlayers":players,"players":players,"outcome":"recording",
        "notes":["Normal observer registration continues; no master/sync/play commands are sent.",
        "At 12 seconds manually select CDJ2 MASTER; at 25 seconds restore CDJ1 MASTER.",
        "Keep Sync off on both decks; raw status establishes actual handoff timing."]}),
    );
}
fn finish_handoff(t: &mut Trace, outcome: &str, message: &str) {
    if guided(t) && t.experiment["outcome"] == "recording" {
        let message = if t.experiment["profile"] == "sync-double-tap" {
            format!("{message} Ensure CDJ2 Sync is OFF.")
        } else {
            message.to_owned()
        };
        t.experiment["outcome"] = json!(outcome);
        t.experiment["message"] = json!(message);
        let ms = t.start.map(|s| s.elapsed().as_secs_f64() * 1000.0);
        t.events
            .push(json!({"hostMs":ms,"detail":{"phase":outcome,"message":message}}));
        t.start = None;
    }
}
pub fn end_handoff(message: &str) {
    finish_handoff(&mut TRACE.lock().unwrap(), "interrupted", message);
}
pub fn finish_handoff_if_expired() {
    let mut t = TRACE.lock().unwrap();
    if !active(&t) {
        let limited = t.samples.len() >= 12000 || t.raw_bytes >= 8 * 1024 * 1024;
        finish_handoff(
            &mut t,
            if limited { "limit-reached" } else { "finished" },
            "Capture ended. Confirm CDJ1 MASTER is restored, then stop and prepare the report.",
        );
    }
}
pub fn update_handoff(players: [Value; 2], validation: Result<(), String>) {
    let mut t = TRACE.lock().unwrap();
    if !active(&t) || !guided(&t) {
        return;
    }
    if t.experiment["profile"] == "sync-double-tap" {
        let ms = t.start.unwrap().elapsed().as_secs_f64() * 1000.0;
        if let Some(sync) = players[1]["sync"].as_bool() {
            if let Some(detail) = t.sync_tap.observe(sync, ms)
                && t.events.len() < 250
            {
                t.events.push(json!({"hostMs":ms,"detail":detail}));
            }
            t.experiment["syncTap"] = t.sync_tap.summary();
        }
    }
    let changed = players.iter().enumerate().any(|(i, p)| {
        ["track", "sourcePlayer", "sourceSlot"]
            .iter()
            .any(|key| p[*key] != t.experiment["initialPlayers"][i][*key])
    });
    if t.experiment["players"] != json!(players) && t.events.len() < 250 {
        let ms = t.start.unwrap().elapsed().as_secs_f64() * 1000.0;
        t.events
            .push(json!({"hostMs":ms,"detail":{"phase":"observed-state","players":players}}));
    }
    t.experiment["players"] = json!(players);
    if let Err(error) = validation {
        finish_handoff(
            &mut t,
            "interrupted",
            &format!("{error} Capture stopped. Restore CDJ1 MASTER manually and save the report."),
        );
    } else if changed {
        finish_handoff(
            &mut t,
            "interrupted",
            "Track changed. Restore CDJ1 MASTER manually and save the report.",
        );
    }
}
pub fn report() -> Value {
    let t = TRACE.lock().unwrap();
    json!({"version":2,"positionTracker":"beat-motion-v5-loop-direction","hostWorkMs":metric(&t.work_ms),"timerWakeDelayMs":metric(&t.wake_ms),"active":active(&t),"receivers":t.receivers,"experiment":t.experiment,"events":t.events,"limitReached":t.samples.len() >= 12000 || t.raw_bytes >= 8*1024*1024,"samples":t.samples,"notes":["Host times are relative to capture start. Browser times use a separate clock.","Raw datagrams from configured peers are captured in direct-IP mode on ports 50000, 50001, 50002 and 50004; published positions are captured in both live modes.","An open socket does not prove that broadcasts reach this device. Zero packets does not identify the cause.","These timings cannot measure jog-to-network or network transit latency without external video/reference measurements."]})
}
fn metric(values: &[f64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({"samples":sorted.len(),"p95":sorted[(sorted.len()-1)*95/100],"max":sorted.last(),"mean":sorted.iter().sum::<f64>()/sorted.len() as f64})
}
/// Work after socket selection and timer wake lateness, not network transit time.
pub fn loop_health(work_ms: f64, wake_ms: Option<f64>) {
    let mut t = TRACE.lock().unwrap();
    if !active(&t) {
        return;
    }
    if t.work_ms.len() < 4096 {
        t.work_ms.push(work_ms);
    }
    if let Some(ms) = wake_ms
        && t.wake_ms.len() < 4096
    {
        t.wake_ms.push(ms);
    }
}
pub fn receivers(value: Value) {
    let mut t = TRACE.lock().unwrap();
    if active(&t) {
        t.receivers = value;
    }
}
pub fn experiment(value: Value) {
    TRACE.lock().unwrap().experiment = value;
}
pub fn event(value: Value) {
    let mut t = TRACE.lock().unwrap();
    if t.events.len() < 256 {
        let ms = t.start.map(|s| s.elapsed().as_secs_f64() * 1000.0);
        t.events.push(json!({"hostMs":ms,"detail":value}));
    }
}
pub fn transmitted(port: u16, to: std::net::SocketAddr, bytes: &[u8]) {
    packet_direction(port, to, bytes, "tx");
}
pub fn packet(port: u16, from: std::net::SocketAddr, bytes: &[u8]) {
    packet_direction(port, from, bytes, "rx");
}
fn packet_direction(port: u16, from: std::net::SocketAddr, bytes: &[u8], direction: &str) {
    let mut t = TRACE.lock().unwrap();
    if !active(&t) {
        return;
    }
    let ms = t.start.unwrap().elapsed().as_secs_f64() * 1000.0;
    // One allocation instead of formatting/allocating once per byte on the
    // UDP receive path. Preserve exactly the same diagnostic hex representation.
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut raw = String::with_capacity(bytes.len().min(2048) * 2);
    for byte in bytes.iter().take(2048) {
        raw.push(HEX[usize::from(byte >> 4)] as char);
        raw.push(HEX[usize::from(byte & 15)] as char);
    }
    t.raw_bytes += bytes.len().min(2048);
    let fields = match (port, prolink_proto::status::decode(bytes)) {
        (50002, Ok(prolink_proto::status::Packet::CdjStatus(packet))) => {
            let status = prolink::monitor::PlayerStatus::from_packet(&packet);
            let speeds: Vec<_> = [0x8c, 0x98, 0xc0, 0xc4]
                .into_iter()
                .map(|offset| {
                    bytes
                        .get(offset..offset + 4)
                        .map(|b| u32::from_be_bytes(b.try_into().unwrap()) as f64 / 1048576.0)
                })
                .collect();
            json!({"packetCounter":packet.packet_counter(),"player":packet.sender().map(|n|n.get()),"beatNumber":status.beat_number,"beatInBar":status.beat_in_bar,"barPosition":status.bar_position,"jogging":status.jogging,"reverse":status.reverse,"playing":status.is_playing,"master":status.is_tempo_master,"playState":status.play_state.0,"speedFields":speeds,"sync":status.is_synced,"tempoOnlySync":bytes.get(0x89).map(|f|f & 2 != 0),"playState2":bytes.get(0x8b),"playState3":bytes.get(0x9d),"playState4":bytes.get(0x113)})
        }
        _ => Value::Null,
    };
    t.samples
        .push(json!({"kind":"udp","direction":direction,"hostMs":ms,"ip":from.ip().to_string(),"sourcePort":from.port(),"port":port,"length":bytes.len(),"truncated":bytes.len()>2048,"fields":fields,"hex":raw}));
}
pub fn snapshot(decks: &[Value]) {
    let mut t = TRACE.lock().unwrap();
    if !active(&t) {
        return;
    }
    for deck in decks {
        let number = deck["number"].as_u64().unwrap_or(0);
        let id = deck["observationId"].as_str().unwrap_or("");
        if t.ids.get(&number).is_some_and(|old| old == id) {
            continue;
        }
        t.ids.insert(number, id.into());
        let ms = t.start.unwrap().elapsed().as_secs_f64() * 1000.0;
        t.samples
            .push(json!({"kind":"published","hostMs":ms,"deck":deck}));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_requires_opt_in_and_deduplicates_observations() {
        command(true);
        let d = json!({"number":1,"observationId":"a","position":1.0});
        snapshot(std::slice::from_ref(&d));
        snapshot(&[d]);
        assert_eq!(report()["samples"].as_array().unwrap().len(), 1);
        // Unknown telemetry must survive capture with its actual port and
        // ephemeral source port; absence from a decoder is not absence on wire.
        receivers(json!({"50001":{"bound":true},"50004":{"bound":false,"error":"busy"}}));
        packet(50001, "127.0.0.1:1402".parse().unwrap(), &[1, 2]);
        let r = report();
        assert_eq!(r["samples"][1]["port"], 50001);
        assert_eq!(r["samples"][1]["sourcePort"], 1402);
        assert_eq!(r["samples"][1]["hex"], "0102");
        assert!(r["samples"][1]["fields"].is_null());
        assert_eq!(r["receivers"]["50004"]["bound"], false);
        command(false);
        packet(50002, "127.0.0.1:50002".parse().unwrap(), &[1, 2]);
        assert_eq!(report()["samples"].as_array().unwrap().len(), 2);
        let probe = crate::bridge_probe::Probe::start();
        assert!(probe.active());
        transmitted(50000, "127.0.0.1:50000".parse().unwrap(), &[6, 192]);
        packet(50001, "127.0.0.1:4567".parse().unwrap(), &[11, 42]);
        assert_eq!(report()["samples"][0]["direction"], "tx");
        assert_eq!(report()["samples"][1]["direction"], "rx");
        assert_eq!(state()["profile"], "bridge-c0");
        command(false);
        assert!(!probe.active());
        drop(probe);
        let probe = crate::bridge_probe::Probe::start();
        drop(probe);
        assert!(!bridge_active());
        let probe = crate::bridge_probe::Probe::start();
        let newer = command(true);
        assert!(!probe.active());
        probe.finish("Old session must not stop a new capture");
        drop(probe);
        assert_eq!(state()["session"], newer["session"]);
        assert_eq!(state()["active"], true);
        command(false);
        let pair = [
            json!({"number":1,"track":42,"sourcePlayer":1,"sourceSlot":3,"master":true}),
            json!({"number":2,"track":55,"sourcePlayer":1,"sourceSlot":3,"master":false}),
        ];
        start_handoff(pair.clone());
        assert!(handoff_active());
        let mut changed = pair.clone();
        changed[1]["master"] = json!(true);
        update_handoff(changed, Ok(()));
        assert!(handoff_active()); // Handoff overlap must remain observable.
        update_handoff(pair.clone(), Err("Sync enabled".into()));
        assert!(!handoff_active());
        assert_eq!(report()["experiment"]["outcome"], "interrupted");
        assert!(
            state()["message"]
                .as_str()
                .unwrap()
                .contains("Restore CDJ1")
        );
        start_handoff(pair.clone());
        let mut changed = pair.clone();
        changed[1]["track"] = json!(56);
        update_handoff(changed, Ok(()));
        assert!(!handoff_active());
        start_handoff(pair.clone());
        TRACE.lock().unwrap().start = Some(Instant::now() - Duration::from_secs(46));
        finish_handoff_if_expired();
        assert_eq!(report()["experiment"]["outcome"], "finished");
        start_handoff(pair);
        command(false);
        assert_eq!(report()["experiment"]["outcome"], "stopped");
        let mut pair = [
            json!({"track":1,"sourcePlayer":1,"sourceSlot":3,"sync":false}),
            json!({"track":2,"sourcePlayer":1,"sourceSlot":3,"sync":false}),
        ];
        start_sync_tap(pair.clone());
        assert!(sync_tap_active());
        assert!(guided_active());
        assert!(!handoff_active());
        pair[1]["sync"] = json!(true);
        update_handoff(pair.clone(), Ok(()));
        pair[1]["sync"] = json!(false);
        update_handoff(pair.clone(), Ok(()));
        assert_eq!(state()["syncTap"]["pulses"], 1);
        update_handoff(pair, Err("CDJ1 Sync is on".into()));
        assert!(!guided_active());
        assert_eq!(report()["experiment"]["outcome"], "interrupted");
        assert!(
            state()["message"]
                .as_str()
                .unwrap()
                .contains("CDJ2 Sync is OFF")
        );
    }
}
