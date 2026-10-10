//! Local USB serving inside the discovered session. No additional DJ Link sockets.
use crate::{library, loading, local_serving, transcoding};
use prolink::{Discovery, Monitor};
use serde_json::json;
use std::{
    collections::BTreeMap,
    net::Ipv4Addr,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Target {
    number: u8,
    ip: Ipv4Addr,
    mac: String,
}
struct Waiting {
    command: loading::Command,
    target: Target,
    key: local_serving::Key,
    track: u32,
    queued: Instant,
    sent: Option<Instant>,
    matched: Option<Instant>,
}
type Job = (
    loading::Command,
    Target,
    Result<local_serving::Prepared, String>,
);
pub struct Session {
    server: Option<local_serving::Server>,
    jobs: tokio::task::JoinSet<Job>,
    waiting: BTreeMap<u8, Waiting>,
    busy: std::collections::BTreeSet<u8>,
    retry_at: Instant,
    conflicted: bool,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            server: None,
            jobs: tokio::task::JoinSet::new(),
            waiting: BTreeMap::new(),
            busy: Default::default(),
            retry_at: Instant::now(),
            conflicted: false,
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if self.server.is_some() {
            local_serving::publish_status(json!({"active":false}));
        }
    }
}
fn fail(command: loading::Command, message: impl Into<String>) {
    let _ = command
        .reply
        .send(json!({"outcome":"error","message":message.into()}));
}
fn collision(discovery: &Discovery, local: Ipv4Addr) -> bool {
    discovery
        .online()
        .iter()
        .any(|d| d.number.get() == local_serving::NUMBER && d.ip != local)
}
fn source_conflict(number: Option<u8>, from: Ipv4Addr, local: Ipv4Addr) -> bool {
    number == Some(local_serving::NUMBER) && from != local
}

fn target(
    discovery: &Discovery,
    monitor: &Monitor,
    number: u8,
) -> Result<(Target, String), String> {
    if !matches!(number, 1 | 2) {
        return Err("Choose CDJ1 or CDJ2".into());
    }
    let device = discovery
        .online()
        .into_iter()
        .find(|d| d.number.get() == number && d.name.as_str().starts_with("CDJ"))
        .ok_or("Target CDJ is not connected")?;
    let observed = monitor
        .player(device.number)
        .and_then(|p| p.status)
        .ok_or("Target status unavailable")?;
    loading::check_load_allowed(observed.status, observed.age)?;
    Ok((
        Target {
            number,
            ip: device.ip,
            mac: device.mac.to_string(),
        },
        device.name.as_str().to_owned(),
    ))
}
impl Session {
    pub fn announcement(
        &mut self,
        announcement: &prolink::discovery::Announcement,
        local: Ipv4Addr,
    ) {
        if source_conflict(
            announcement.packet.body.device_number(),
            announcement.from,
            local,
        ) {
            self.conflicted = true;
        }
    }
    pub fn busy(&self, target: u8) -> bool {
        self.busy.contains(&target)
    }
    pub fn accept(
        &mut self,
        command: loading::Command,
        discovery: &Discovery,
        monitor: &Monitor,
        catalogs: &library::Shared,
    ) {
        let result = (|| -> Result<(local_serving::Key, Target, String), String> {
            if self.conflicted || collision(discovery, discovery.interface().ip) {
                return Err("Local USB source number 4 is occupied".into());
            }
            let key = local_serving::key(&command.body)?;
            if let Some(server) = &self.server {
                server.validate_source(&key)?;
            }
            let number = command.body["target"]
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or("Choose CDJ1 or CDJ2")?;
            if self.busy.contains(&number) {
                return Err("A local load is already pending on this CDJ".into());
            }
            let (target, model) = target(discovery, monitor, number)?;
            Ok((key, target, model))
        })();
        let (key, target, model) = match result {
            Ok(value) => value,
            Err(message) => {
                fail(command, message);
                return;
            }
        };
        self.busy.insert(target.number);
        let catalogs = catalogs.clone();
        let job = transcoding::job_id(&command.body);
        self.jobs.spawn(async move {
            let result = match transcoding::register(&job) {
                Err(error) => Err(error),
                Ok(guard) => {
                    let cancel = guard.flag.clone();
                    tokio::task::spawn_blocking(move || {
                        let _guard = guard;
                        transcoding::prepare_load(&catalogs, key, &model, &job, cancel)
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
                }
            };
            (command, target, result)
        });
    }
    pub fn respond(
        &mut self,
        from: std::net::SocketAddr,
        bytes: &[u8],
        discovery: &Discovery,
        monitor: &Monitor,
    ) -> Result<(), String> {
        let Some(server) = &mut self.server else {
            return Ok(());
        };
        if let Ok(prolink_proto::status::Packet::CdjStatus(packet)) =
            prolink_proto::status::decode(bytes)
            && packet.sender().map(|n| n.get()) == Some(local_serving::NUMBER)
            && from.ip() != server.local
        {
            self.conflicted = true;
            return Ok(());
        }
        if let std::net::IpAddr::V4(ip) = from.ip()
            && discovery
                .online()
                .iter()
                .any(|d| d.ip == ip && matches!(d.number.get(), 1 | 2))
            && let Some(socket) = monitor.status_sender()
        {
            server.respond(bytes, ip, &socket);
        }
        Ok(())
    }
    pub fn asset(&self, track: u32) -> Option<crate::live::Asset> {
        self.server.as_ref()?.asset(track)
    }
    pub fn source_label(&self, track: u32) -> Option<&str> {
        self.server.as_ref()?.track_source_label(track)
    }
    pub async fn tick(
        &mut self,
        discovery: &Discovery,
        monitor: &Monitor,
        catalogs: &library::Shared,
        sender: u8,
    ) -> Result<(), String> {
        let local = discovery.interface().ip;
        if self.conflicted || collision(discovery, local) {
            self.server = None;
            local_serving::publish_status(
                json!({"active":false,"error":"Local USB source number 4 is occupied"}),
            );
            self.jobs.abort_all();
            self.busy.clear();
            for (_, waiting) in std::mem::take(&mut self.waiting) {
                let _ = waiting.command.reply.send(json!({"outcome":if waiting.sent.is_some(){"unknown"}else{"error"},"message":"Local USB source number 4 conflicts with another player. Check the CDJs and restart OLC once number 4 is free."}));
            }
            return Ok(());
        }
        if self.server.is_none()
            && library::has_local_usb(catalogs)
            && Instant::now() >= self.retry_at
            && crate::mac_networking::available()
        {
            // Ignore only our synthetic source MAC, never another physical player 4.
            for bytes in local_serving::stages(local) {
                if let Ok(packet) = prolink_proto::djl::Packet::decode(&bytes)
                    && let prolink_proto::djl::Body::KeepAlive { mac, .. } = packet.body
                {
                    discovery.ignore(mac);
                }
            }
            self.retry_at = Instant::now() + Duration::from_secs(30);
            match local_serving::Server::start(local).await {
                Ok(server) => self.server = Some(server),
                Err(error) => local_serving::publish_status(json!({"active":false,"error":error})),
            }
        }
        while let Some(result) = self.jobs.try_join_next() {
            let (command, target, prepared) = result.map_err(|e| e.to_string())?;
            let result = (|| {
                let prepared = prepared?;
                if command.reply.is_closed() {
                    return Err("Load request cancelled".to_owned());
                }
                if !local_serving::current(catalogs, &prepared.key) {
                    return Err("Local USB changed".into());
                }
                let key = prepared.key.clone();
                let track = self
                    .server
                    .as_mut()
                    .ok_or("Local USB server unavailable")?
                    .add(prepared)?;
                Ok((key, track))
            })();
            match result {
                Ok((key, track)) => {
                    self.waiting.insert(
                        target.number,
                        Waiting {
                            command,
                            target,
                            key,
                            track,
                            queued: Instant::now(),
                            sent: None,
                            matched: None,
                        },
                    );
                }
                Err(message) => {
                    self.busy.remove(&target.number);
                    fail(command, message);
                }
            }
        }
        let Some(server) = &mut self.server else {
            return Ok(());
        };
        let socket = monitor
            .status_sender()
            .ok_or("Local USB transport unavailable")?;
        let peers: Vec<_> = discovery
            .online()
            .iter()
            .filter(|d| matches!(d.number.get(), 1 | 2))
            .filter(|d| {
                monitor
                    .player(d.number)
                    .and_then(|p| p.status)
                    .is_some_and(|s| s.age < Duration::from_secs(1))
            })
            .map(|d| (d.ip, local))
            .collect();
        server.tick(&peers, &discovery.socket(), &socket)?;
        local_serving::publish_status(server.diagnostics());
        let mut finished = Vec::new();
        for (&number, w) in &mut self.waiting {
            let result = (|| -> Result<Option<&str>, String> {
                let device = discovery
                    .online()
                    .into_iter()
                    .find(|d| {
                        d.number.get() == number
                            && d.ip == w.target.ip
                            && d.mac.to_string() == w.target.mac
                    })
                    .ok_or("Target changed or disconnected; check the CDJ")?;
                if !local_serving::current(catalogs, &w.key) {
                    return Err("USB changed or disconnected; check the CDJ".into());
                }
                let observed = monitor
                    .player(device.number)
                    .and_then(|p| p.status)
                    .ok_or("Target status unavailable")?;
                if let Some(sent) = w.sent {
                    let matched = observed.age < Duration::from_secs(1)
                        && Instant::now()
                            .checked_sub(observed.age)
                            .is_some_and(|at| at > sent)
                        && matches!(observed.status.play_state.0, 3..=6)
                        && observed.status.track.is_some_and(|t| {
                            t.id == w.track
                                && t.source_player.get() == local_serving::NUMBER
                                && t.slot == prolink::Slot::USB
                                && t.kind.0 == 1
                        });
                    if matched {
                        if w.matched.get_or_insert_with(Instant::now).elapsed()
                            >= Duration::from_millis(500)
                        {
                            return Ok(Some(
                                "CDJ reports the selected track. Check its display for ready/error state.",
                            ));
                        }
                    } else {
                        w.matched = None;
                    }
                    if sent.elapsed() > Duration::from_secs(12) {
                        return Err("No matching status within 12 seconds. Check the CDJ; no retry was sent".into());
                    }
                } else {
                    loading::check_load_allowed(observed.status, observed.age)?;
                    local_serving::compatible(
                        server.track(w.track).ok_or("Served track disappeared")?,
                        &device.name.as_str(),
                    )?;
                    if !server.ready(device.ip) {
                        if w.queued.elapsed() > Duration::from_secs(8) {
                            return Err("CDJ has not discovered the local USB. Press LINK, wait for the source, then load again. No command was sent".into());
                        }
                        return Ok(None);
                    }
                    if w.command.reply.is_closed() {
                        return Err("Request cancelled; no command sent".into());
                    }
                    if observed.status.track.is_some_and(|t| {
                        t.id == w.track && t.source_player.get() == local_serving::NUMBER
                    }) {
                        return Err("Track already selected; no reload sent".into());
                    }
                    let packet = crate::cdj_usb_load::LoadUsbTrack {
                        sender,
                        source: local_serving::NUMBER,
                        target: number,
                        track_id: w.track,
                    }
                    .encode()
                    .map_err(str::to_owned)?;
                    socket
                        .try_send_to(&packet, (device.ip, 50002).into())
                        .map_err(|e| e.to_string())?;
                    w.sent = Some(Instant::now());
                }
                Ok(None)
            })();
            match result {
                Ok(Some(message)) => finished.push((number, "reported", message.to_owned())),
                Err(message) => finished.push((
                    number,
                    if w.sent.is_some() { "unknown" } else { "error" },
                    message,
                )),
                _ => (),
            }
        }
        for (number, outcome, message) in finished {
            if let Some(w) = self.waiting.remove(&number) {
                self.busy.remove(&number);
                let _ = w
                    .command
                    .reply
                    .send(json!({"outcome":outcome,"message":message}));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_claim_and_keepalive_conflicts_but_not_our_own_source() {
        let local = Ipv4Addr::new(192, 168, 10, 99);
        let peer = Ipv4Addr::new(192, 168, 10, 145);
        let packets: Vec<_> = local_serving::stages(local)
            .into_iter()
            .map(|raw| prolink_proto::djl::Packet::decode(&raw).unwrap())
            .collect();
        assert!(
            packets
                .iter()
                .any(|p| source_conflict(p.body.device_number(), peer, local))
        );
        for packet in packets {
            assert!(!source_conflict(packet.body.device_number(), local, local));
        }
        assert!(!source_conflict(Some(1), peer, local));
        assert!(!source_conflict(Some(15), peer, local));
    }
}
