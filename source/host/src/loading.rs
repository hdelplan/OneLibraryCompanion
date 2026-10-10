//! Explicit load requests handled inside the existing observation session.
use crate::{cdj_usb_load::LoadUsbTrack, library};
use prolink::{
    Discovery, Monitor, Slot,
    monitor::{LoadedTrack, PlayerStatus},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    net::Ipv4Addr,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
pub struct Command {
    pub body: Value,
    pub reply: oneshot::Sender<Value>,
}
pub type Sender = mpsc::Sender<Command>;
pub type Receiver = mpsc::Receiver<Command>;
pub fn channel() -> (Sender, Receiver) {
    mpsc::channel(8)
}
#[derive(Clone, PartialEq, Eq)]
struct Selection {
    source: String,
    generation: u64,
    track: u32,
    target: u8,
}
#[derive(Clone, PartialEq, Eq)]
struct TargetIdentity {
    mac: String,
    ip: Ipv4Addr,
    track: Option<LoadedTrack>,
}
struct Pending {
    selection: Selection,
    target: TargetIdentity,
    source_number: u8,
    source_ip: Ipv4Addr,
    sent: Instant,
    matched_since: Option<Instant>,
    reply: Option<oneshot::Sender<Value>>,
}
#[derive(Default)]
pub struct Controller {
    pending: BTreeMap<u8, Pending>,
}
/// Conservative status policy: ambiguous or stale status never sends a command.
pub fn check_load_allowed(status: PlayerStatus, age: Duration) -> Result<(), String> {
    if age >= Duration::from_secs(1) {
        return Err("Target CDJ status is stale; wait for a fresh connection".into());
    }
    let raw = status.play_state.0;
    if raw == 2 {
        return Err("Target CDJ is already loading a track".into());
    }
    if status.is_playing || matches!(raw, 3 | 4 | 7 | 8 | 9 | 18) {
        return Err(
            "Target CDJ is playing or looping. Stop playback before loading a track".into(),
        );
    }
    crate::cdj_usb_load::TargetStatus {
        age_ms: age.as_millis() as u64,
        playing: status.is_playing,
        play_state: if raw == 14 && status.track.is_none() {
            0
        } else {
            raw
        },
    }
    .check()
    .map_err(|reason| format!("{reason} (player state {raw})"))
}
fn selection(body: &Value) -> Result<Selection, String> {
    let source = body["source"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Select a USB source")?
        .to_owned();
    let generation = body["generation"]
        .as_u64()
        .ok_or("Missing catalog generation")?;
    let track = body["trackId"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or("Invalid track ID")?;
    let target = body["target"]
        .as_u64()
        .filter(|n| matches!(n, 1 | 2))
        .ok_or("Choose CDJ1 or CDJ2")? as u8;
    Ok(Selection {
        source,
        generation,
        track,
        target,
    })
}
impl Controller {
    pub fn busy(&self, target: u8) -> bool {
        self.pending.contains_key(&target)
    }
    pub async fn accept(
        &mut self,
        command: Command,
        discovery: &Discovery,
        monitor: &Monitor,
        catalogs: &library::Shared,
        sender: u8,
    ) {
        if command.reply.is_closed() {
            return;
        }
        let Some(socket) = monitor.status_sender() else {
            let _ = command.reply.send(
                json!({"outcome":"error","message":"Load transport unavailable in this session"}),
            );
            return;
        };
        if !matches!(
            tokio::time::timeout(Duration::from_millis(100), socket.writable()).await,
            Ok(Ok(()))
        ) {
            let _ = command.reply.send(
                json!({"outcome":"error","message":"Load transport is busy; no command was sent"}),
            );
            return;
        }
        let result = (|| -> Result<Pending, String> {
            let selected = selection(&command.body)?;
            if self.pending.contains_key(&selected.target) {
                return Err("A load to this CDJ is already pending".into());
            }
            let (source_ip, _) = library::load_source(
                catalogs,
                &selected.source,
                selected.generation,
                selected.track,
            )?;
            let devices = discovery.devices();
            let src = devices
                .iter()
                .find(|d| d.ip == source_ip && !d.offline && d.number.get() <= 6)
                .ok_or("Source CDJ disconnected")?;
            let dst = devices
                .iter()
                .find(|d| {
                    d.number.get() == selected.target
                        && !d.offline
                        && d.name.as_str().starts_with("CDJ")
                })
                .ok_or("Target CDJ is not connected")?;
            let source_status = monitor
                .player(src.number)
                .and_then(|p| p.status)
                .ok_or("Source status unavailable")?;
            if source_status.age >= Duration::from_secs(1) || !source_status.status.usb_present {
                return Err("Source USB is no longer available".into());
            }
            let observed = monitor
                .player(dst.number)
                .and_then(|p| p.status)
                .ok_or("Target CDJ status unavailable")?;
            check_load_allowed(observed.status, observed.age)?;
            let target = TargetIdentity {
                mac: dst.mac.to_string(),
                ip: dst.ip,
                track: observed.status.track,
            };
            let already = target.track.is_some_and(|t| {
                t.id == selected.track
                    && t.source_player == src.number
                    && t.slot == Slot::USB
                    && t.kind.0 == 1
            });
            if already {
                return Err(
                    "This track is already selected on the target CDJ; no reload was sent".into(),
                );
            }
            let packet = LoadUsbTrack {
                sender,
                source: src.number.get(),
                target: selected.target,
                track_id: selected.track,
            }
            .encode()
            .map_err(str::to_owned)?;
            if command.reply.is_closed() {
                return Err("Request cancelled before sending".into());
            }
            // No await between fresh checks and the single send. Never retry automatically.
            socket
                .try_send_to(&packet, (dst.ip, 50002).into())
                .map_err(|e| format!("Load send failed: {e}"))?;
            Ok(Pending {
                selection: selected,
                target,
                source_number: src.number.get(),
                source_ip,
                sent: Instant::now(),
                matched_since: None,
                reply: None,
            })
        })();
        match result {
            Err(message) => {
                let _ = command
                    .reply
                    .send(json!({"outcome":"error","message":message}));
            }
            Ok(mut pending) => {
                pending.reply = Some(command.reply);
                self.pending.insert(pending.selection.target, pending);
            }
        }
    }
    pub fn tick(&mut self, discovery: &Discovery, monitor: &Monitor, catalogs: &library::Shared) {
        let devices = discovery.devices();
        let mut done = Vec::new();
        for (&number, p) in &mut self.pending {
            let dst = devices.iter().find(|d| {
                d.number.get() == number
                    && d.ip == p.target.ip
                    && d.mac.to_string() == p.target.mac
                    && !d.offline
            });
            let source_valid = library::load_source(
                catalogs,
                &p.selection.source,
                p.selection.generation,
                p.selection.track,
            )
            .is_ok_and(|(ip, _)| ip == p.source_ip);
            if dst.is_none() || !source_valid {
                done.push((
                    number,
                    "unknown",
                    "Source or target disconnected. Load outcome is unknown; check the CDJ.",
                ));
                continue;
            }
            let observed = dst
                .and_then(|d| monitor.player(d.number))
                .and_then(|p| p.status);
            let matched = observed.is_some_and(|s| {
                s.age < Duration::from_secs(1)
                    && Instant::now()
                        .checked_sub(s.age)
                        .is_some_and(|at| at > p.sent)
                    && matches!(s.status.play_state.0, 3..=6)
                    && s.status.track.is_some_and(|t| {
                        t.id == p.selection.track
                            && t.source_player.get() == p.source_number
                            && t.slot == Slot::USB
                            && t.kind.0 == 1
                    })
            });
            if matched {
                let start = p.matched_since.get_or_insert_with(Instant::now);
                if start.elapsed() >= Duration::from_millis(500) {
                    done.push((
                        number,
                        "reported",
                        "CDJ reports the selected track. Check its display for ready/error state.",
                    ));
                }
            } else {
                p.matched_since = None;
            }
            if p.sent.elapsed() > Duration::from_secs(12)
                && !done.iter().any(|(n, _, _)| *n == number)
            {
                done.push((number,"unknown","No matching CDJ status within 12 seconds. Check the CDJ before trying again; no retry was sent."));
            }
        }
        for (number, outcome, message) in done {
            if let Some(p) = self.pending.remove(&number)
                && let Some(reply) = p.reply
            {
                let _ = reply.send(json!({"outcome":outcome,"target":number,"message":message}));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prolink::monitor::PlayState;
    fn status(raw: u8, playing: bool) -> PlayerStatus {
        PlayerStatus {
            usb_present: true,
            sd_present: false,
            jogging: false,
            bar_position: None,
            active_loop: None,
            beat_in_bar: None,
            reverse: false,
            beat_number: None,
            play_state: PlayState(raw),
            track: None,
            bpm_centi: None,
            pitch: None,
            motion_pitch: None,
            forward_transport: false,
            is_tempo_master: false,
            is_synced: false,
            is_playing: playing,
            yielding_to: None,
        }
    }
    #[test]
    fn stale_and_loading_targets_are_never_overridden() {
        assert!(check_load_allowed(status(5, false), Duration::from_secs(1)).is_err());
        assert!(check_load_allowed(status(2, true), Duration::ZERO).is_err());
        assert!(check_load_allowed(status(255, false), Duration::ZERO).is_err());
    }
    #[test]
    fn playing_looping_and_cue_audition_are_blocked_even_with_clear_flag() {
        for raw in [3, 4, 7, 8, 9, 18] {
            assert!(check_load_allowed(status(raw, false), Duration::ZERO).is_err());
        }
        assert!(check_load_allowed(status(5, true), Duration::ZERO).is_err());
        for raw in [0, 5, 6, 14] {
            assert!(check_load_allowed(status(raw, false), Duration::ZERO).is_ok());
        }
    }
    #[test]
    fn invalid_targets_and_track_ids_are_rejected() {
        for target in [0, 3, 7] {
            assert!(
                selection(&json!({"source":"USB","generation":1,"target":target,"trackId":1}))
                    .is_err()
            );
        }
        assert!(selection(&json!({"source":"USB","generation":1,"target":1,"trackId":0})).is_err());
    }
}
