//! Bounded experimental registration. Received candidates never drive playback.
use serde_json::{Value, json};
use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};

pub struct Probe {
    session: String,
    started: Instant,
    next_send: Instant,
}
impl Probe {
    pub fn start() -> Self {
        let state = crate::jog_trace::command(true);
        crate::jog_trace::experiment(json!({
            "profile":"bridge-c0", "transport":"unicast", "identity":"synthetic-local-mac",
            "settleMs":5000, "durationMs":45000,
            "notes":["Unverified on original CDJ-2000nexus; silence is inconclusive.",
                "Normal observer announcements and live position updates are suspended.",
                "Only 54-byte bridge announcements; no master, sync, load or play commands.",
                "Synthetic locally administered MAC; not the hardware interface MAC."]
        }));
        let started = Instant::now();
        Self {
            session: state["session"].as_str().unwrap().into(),
            started,
            next_send: started + Duration::from_secs(5),
        }
    }
    pub fn active(&self) -> bool {
        let state = crate::jog_trace::state();
        self.matches(&state)
            && state["active"] == true
            && self.started.elapsed() < Duration::from_secs(45)
    }
    fn matches(&self, state: &Value) -> bool {
        state["session"] == self.session
    }
    pub fn due(&mut self, now: Instant) -> bool {
        if now < self.next_send {
            return false;
        }
        self.next_send = now + Duration::from_millis(1500);
        true
    }
    pub fn finish(&self, reason: &str) {
        if self.matches(&crate::jog_trace::state()) {
            crate::jog_trace::event(json!({"phase":"finished", "reason":reason}));
            crate::jog_trace::command(false);
        }
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        if self.matches(&crate::jog_trace::state()) && crate::jog_trace::bridge_active() {
            self.finish("Direct session closed; bridge registration stopped");
        }
    }
}
pub fn announcement(local: Ipv4Addr) -> [u8; 54] {
    let mut p = [0; 54];
    p[..10].copy_from_slice(b"Qspt1WmJOL");
    p[10] = 6;
    p[12..27].copy_from_slice(b"TCS-SHOWKONTROL");
    p[32..36].copy_from_slice(&[1, 1, 0, 54]);
    p[36] = 0xc0;
    let ip = local.octets();
    p[38..44].copy_from_slice(&[2, 80, ip[0], ip[1], ip[2], ip[3]]);
    p[44..48].copy_from_slice(&ip);
    p[48] = 3;
    p[52] = 5;
    p[53] = 0x20;
    p
}
pub fn collision(raw: &[u8], local: Ipv4Addr) -> bool {
    if raw.len() < 37 || &raw[..10] != b"Qspt1WmJOL" {
        return false;
    }
    match raw[10] {
        4 | 8 => raw[36] == 0xc0,
        2 if raw.len() >= 47 => raw[46] == 0xc0,
        6 if raw.len() >= 54 => raw[44..48] != local.octets() && (raw[36] == 0xc0 || raw[52] == 5),
        _ => false,
    }
}
pub fn details(local: Ipv4Addr, peer: Ipv4Addr) -> Value {
    json!({"local":local.to_string(),"peer":peer.to_string()})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_profile_and_conflicts() {
        let ip = Ipv4Addr::new(192, 168, 10, 10);
        let p = announcement(ip);
        assert_eq!(p.len(), 54);
        assert_eq!(&p[32..38], &[1, 1, 0, 54, 192, 0]);
        assert_eq!(&p[48..], &[3, 0, 0, 0, 5, 32]);
        assert!(!collision(&p, ip));
        assert!(collision(&p, Ipv4Addr::new(192, 168, 10, 11)));
        assert!(!collision(&p[..36], ip));
        let mut other = p;
        other[36] = 1;
        other[52] = 1;
        assert!(!collision(&other, Ipv4Addr::LOCALHOST));
        for kind in [4, 8] {
            other[10] = kind;
            other[36] = 0xc0;
            assert!(collision(&other, ip));
        }
        other[10] = 2;
        other[46] = 0xc0;
        assert!(collision(&other, ip));
    }
    #[test]
    fn quiet_period_and_cadence() {
        let now = Instant::now();
        let mut probe = Probe {
            session: String::new(),
            started: now,
            next_send: now + Duration::from_secs(5),
        };
        assert!(!probe.due(now + Duration::from_millis(4999)));
        assert!(probe.due(now + Duration::from_secs(5)));
        assert!(!probe.due(now + Duration::from_secs(6)));
        assert!(probe.due(now + Duration::from_millis(6500)));
    }
}
