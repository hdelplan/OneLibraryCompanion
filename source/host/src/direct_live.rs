//! Two explicitly selected CDJs sharing one pair of unicast UDP sockets.
use super::*;
use prolink::monitor::TrackKind;
use prolink_proto::{djl, status};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot},
};

pub struct Control {
    number: u8,
    ip: Option<Ipv4Addr>,
    bridge: bool,
    handoff: bool,
    sync_tap: bool,
    reply: oneshot::Sender<Result<(), String>>,
}

/// Release the idle observer's sockets before an exclusive USB serving test.
/// Disconnecting individual peers leaves the observer alive for reconnection.
pub async fn release_idle_transport(shared: &Shared) -> Result<(), String> {
    let task = {
        let mut state = shared.lock().unwrap();
        if state.enabled || !state.direct_peers.is_empty() {
            return Err("Disconnect live mode before starting the USB playback test".into());
        }
        state.direct_controls = None;
        state.loader = None;
        state.direct_task.take()
    };
    if let Some(task) = task {
        task.abort();
        // Abort is asynchronous: await destruction before attempting to bind.
        let _ = task.await;
    }
    Ok(())
}

pub fn stop(shared: &Shared, catalogs: &crate::library::Shared) -> Result<(), String> {
    if crate::cue_window::occupied() {
        return Err(
            "Disable cue windows in TEST and wait for restoration before disconnecting".into(),
        );
    }
    let mut state = shared.lock().unwrap();
    if state
        .decks
        .iter()
        .any(|d| d["sourceLabel"] == "Local USB" && d["loadProtected"] != false)
    {
        return Err("Stop both CDJs playing from the local USB before disconnecting".into());
    }
    if state.enabled && state.direct_task.is_none() {
        return Err("Desktop live session is active".into());
    }
    crate::jog_trace::command(false);
    if let Some(task) = state.direct_task.take() {
        task.abort();
    }
    state.direct_controls = None;
    state.direct_peers.clear();
    state.enabled = false;
    state.loader = None;
    state.decks.clear();
    state.assets.clear();
    state.error = None;
    crate::library::invalidate_direct(catalogs);
    publish(&state);
    Ok(())
}

/// Setting an IP connects that numbered player; None disconnects only that player.
pub async fn configure(
    shared: &Shared,
    catalogs: &crate::library::Shared,
    number: u8,
    ip: Option<Ipv4Addr>,
) -> Result<(), String> {
    configure_at(shared, catalogs, number, ip, 50000).await
}
async fn configure_at(
    shared: &Shared,
    catalogs: &crate::library::Shared,
    number: u8,
    ip: Option<Ipv4Addr>,
    discovery_port: u16,
) -> Result<(), String> {
    if !(1..=2).contains(&number) {
        return Err("Choose CDJ 1 or CDJ 2; match its physical player number".into());
    }
    let sender = {
        let mut state = shared.lock().unwrap();
        if state.enabled && state.direct_task.is_none() {
            return Err("Desktop live session is active".into());
        }
        if state.direct_controls.as_ref().is_none_or(|s| s.is_closed()) {
            let (controls, receiver) = mpsc::channel(8);
            let (loader, commands) = crate::loading::channel();
            state.direct_controls = Some(controls);
            state.loader = Some(loader);
            let shared = shared.clone();
            let catalogs = catalogs.clone();
            state.direct_task = Some(tokio::spawn(async move {
                let result = observe_direct(
                    discovery_port,
                    shared.clone(),
                    catalogs.clone(),
                    commands,
                    receiver,
                )
                .await;
                crate::cue_window::connection_closed();
                crate::jog_trace::end_handoff(
                    "Direct connection closed. Restore CDJ1 MASTER manually.",
                );
                crate::library::invalidate_direct(&catalogs);
                let mut state = shared.lock().unwrap();
                state.loader = None;
                state.direct_controls = None;
                state.decks.clear();
                state.assets.clear();
                state.error = result.err();
                let error = state.error.clone();
                for peer in &mut state.direct_peers {
                    peer["state"] = json!("error");
                    peer["error"] = json!(error);
                }
                publish(&state);
            }));
        }
        state.direct_controls.as_ref().unwrap().clone()
    };
    let (reply, result) = oneshot::channel();
    sender
        .try_send(Control {
            number,
            ip,
            bridge: false,
            handoff: false,
            sync_tap: false,
            reply,
        })
        .map_err(|_| "Connection controls are busy".to_string())?;
    result.await.map_err(|_| {
        shared
            .lock()
            .unwrap()
            .error
            .clone()
            .unwrap_or("Direct connection closed".into())
    })?
}

pub async fn start_bridge(shared: &Shared) -> Result<(), String> {
    if !crate::EXPERIMENTS {
        return Err("Experiments are unavailable in this build".into());
    }
    let sender = shared
        .lock()
        .unwrap()
        .direct_controls
        .clone()
        .ok_or("Connect both CDJs using direct IP before starting the bridge test")?;
    let (reply, result) = oneshot::channel();
    sender
        .try_send(Control {
            number: 0,
            ip: None,
            bridge: true,
            handoff: false,
            sync_tap: false,
            reply,
        })
        .map_err(|_| "Connection controls are busy")?;
    result.await.map_err(|_| "Direct connection closed")?
}

pub async fn start_handoff(shared: &Shared) -> Result<(), String> {
    start_guided(shared, false).await
}
pub async fn start_sync_tap(shared: &Shared) -> Result<(), String> {
    start_guided(shared, true).await
}
async fn start_guided(shared: &Shared, sync_tap: bool) -> Result<(), String> {
    if !crate::EXPERIMENTS {
        return Err("Experiments are unavailable in this build".into());
    }
    let sender = shared
        .lock()
        .unwrap()
        .direct_controls
        .clone()
        .ok_or("Connect both CDJs using direct IP before starting the guided capture")?;
    let (reply, result) = oneshot::channel();
    sender
        .try_send(Control {
            number: 0,
            ip: None,
            bridge: false,
            handoff: !sync_tap,
            sync_tap,
            reply,
        })
        .map_err(|_| "Connection controls are busy")?;
    result.await.map_err(|_| "Direct connection closed")?
}
fn handoff_players(peers: &BTreeMap<u8, Peer>) -> [Option<&status::CdjStatus>; 2] {
    [1, 2].map(|number| peers.get(&number).and_then(Peer::fresh))
}

struct Pending {
    source: String,
    source_number: u8,
    source_ip: Ipv4Addr,
    generation: u64,
    track: u32,
    sent: Instant,
    matched: Option<Instant>,
    reply: oneshot::Sender<Value>,
}
struct Peer {
    ip: Ipv4Addr,
    started: Instant,
    epoch: u64,
    local: Ipv4Addr,
    sent: usize,
    next_send: Instant,
    observed: Option<(status::CdjStatus, Instant)>,
    order: prolink::monitor::StatusPacketOrder,
    usb: bool,
    error: Option<String>,
    pending: Option<Pending>,
    bar: crate::bar_position::BarPosition,
    beat_tracker: crate::beat_position::BeatPosition,
    pulse: Option<(prolink_proto::beat::Beat, Instant)>,
    loops: crate::loop_region::LoopRegion,
    loop_at: Option<Instant>,
    loop_pulse_at: Option<Instant>,
    current_loop: Option<Value>,
}
impl Peer {
    fn observe(
        &mut self,
        number: u8,
        s: status::CdjStatus,
        catalogs: &crate::library::Shared,
        at: Instant,
    ) {
        if !self.order.accept(s.packet_counter(), at) {
            return;
        }
        if s.sender().map(|n| n.get()) != Some(number) {
            self.fail(catalogs,format!("Expected CDJ {number}, but this IP reports CDJ {}. Match the physical player number and reconnect.",s.sender().map_or(0,|n|n.get())));
        } else {
            let usb = s.usb_state() == status::MediaState::LOADED;
            if usb != self.usb {
                self.epoch += 1;
                if usb {
                    let _ = crate::library::connect_direct_peer(catalogs, self.ip, number);
                } else {
                    crate::library::invalidate_direct_peer(catalogs, self.ip);
                }
                self.usb = usb;
            }
            self.observed = Some((s, at));
        }
    }
    fn fresh(&self) -> Option<&status::CdjStatus> {
        self.observed
            .as_ref()
            .filter(|(_, at)| self.error.is_none() && at.elapsed() < Duration::from_secs(1))
            .map(|(s, _)| s)
    }
    fn fail(&mut self, catalogs: &crate::library::Shared, error: String) {
        crate::library::invalidate_direct_peer(catalogs, self.ip);
        self.error = Some(error);
        self.observed = None;
        self.usb = false;
        self.epoch += 1;
        if let Some(p) = self.pending.take() {
            unknown(p);
        }
    }
}
fn unknown(p: Pending) {
    let _ = p.reply.send(json!({"outcome":"unknown","message":"Load outcome unknown. Check the CDJ; no automatic retry was sent."}));
}
fn validate_load(s: &status::CdjStatus, age: Duration) -> Result<u8, String> {
    let number = s
        .sender()
        .filter(|n| n.is_player())
        .ok_or("Unknown player identity")?
        .get();
    crate::cdj_usb_load::TargetStatus {
        age_ms: age.as_millis() as u64,
        playing: prolink::monitor::PlayerStatus::from_packet(s).is_playing,
        play_state: s.play_state().ok_or("Missing play state")?,
    }
    .check()
    .map_err(str::to_owned)?;
    s.flags().ok_or("Missing play flags")?;
    Ok(number)
}
fn load_selection(
    body: &Value,
    peers: &BTreeMap<u8, Peer>,
    catalogs: &crate::library::Shared,
) -> Result<(u8, u8, Ipv4Addr, u32), String> {
    let target = body["target"]
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .ok_or("Invalid target")?;
    let peer = peers.get(&target).ok_or("Target is not connected")?;
    if peer.pending.is_some() {
        return Err("A load is already pending on this CDJ".into());
    }
    let s = peer.fresh().ok_or("Waiting for fresh target status")?;
    validate_load(s, peer.observed.as_ref().unwrap().1.elapsed())?;
    let generation = body["generation"].as_u64().ok_or("Missing generation")?;
    let id = body["trackId"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or("Invalid track")?;
    let source = body["source"].as_str().ok_or("Missing source")?;
    let query = crate::library::Query::from([("generation".into(), generation.to_string())]);
    let (_, catalog) = crate::library::catalog(catalogs, source, &query)?;
    let (location, _) =
        crate::library::artwork_location(catalogs, source, generation).ok_or("Source changed")?;
    let crate::library::Location::Direct(source_ip) = location else {
        return Err("Select a connected CDJ USB".into());
    };
    let (&source_number, _) = peers
        .iter()
        .find(|(_, p)| p.ip == source_ip && p.error.is_none() && p.usb && p.observed.is_some())
        .ok_or("Source CDJ or USB is unavailable")?;
    if !catalog.library.tracks.contains_key(&id) {
        return Err("Track is not in this USB catalog".into());
    }
    if s.track_id() == id
        && s.source_player().map(|n| n.get()) == Some(source_number)
        && s.source_slot() == Slot::USB
    {
        return Err("Track already loaded; no command sent".into());
    }
    Ok((target, source_number, source_ip, id))
}
async fn diagnostic_socket(port: u16) -> (Option<UdpSocket>, Value) {
    match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).await {
        Ok(socket) => (Some(socket), json!({"bound":true})),
        Err(error) => (None, json!({"bound":false,"error":error.to_string()})),
    }
}
async fn diagnostic_receive(
    socket: &Option<UdpSocket>,
    buffer: &mut [u8],
) -> std::io::Result<(usize, std::net::SocketAddr)> {
    match socket {
        Some(socket) => socket.recv_from(buffer).await,
        None => std::future::pending().await,
    }
}
fn record_diagnostic(
    result: std::io::Result<(usize, std::net::SocketAddr)>,
    port: u16,
    buffer: &[u8],
    peers: &BTreeMap<u8, Peer>,
    socket: &mut Option<UdpSocket>,
    state: &mut Value,
) {
    match result {
        Ok((n, from))
            if peers
                .values()
                .any(|p| p.error.is_none() && from.ip() == std::net::IpAddr::V4(p.ip)) =>
        {
            crate::jog_trace::packet(port, from, &buffer[..n])
        }
        Ok(_) => {}
        Err(error) => {
            *socket = None;
            *state = json!({"bound":false,"error":error.to_string()});
        }
    }
}

async fn observe_direct(
    discovery_port: u16,
    shared: Shared,
    catalogs: crate::library::Shared,
    mut commands: crate::loading::Receiver,
    mut controls: mpsc::Receiver<Control>,
) -> Result<(), String> {
    let discovery = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 50000))
        .await
        .map_err(|e| format!("Cannot open discovery port: {e}"))?;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 50002))
        .await
        .map_err(|e| format!("Cannot open status port: {e}"))?;
    // Diagnostic receivers only: failure must not break the working unicast
    // connection. Binding does not establish iOS broadcast permission/delivery.
    let (mut beat_socket, mut beat_state) = diagnostic_socket(50001).await;
    let (mut extra_socket, mut extra_state) = diagnostic_socket(50004).await;
    let mut beat_buffer = [0u8; 4096];
    let mut extra_buffer = [0u8; 4096];
    let mut peers = BTreeMap::<u8, Peer>::new();
    let mut bridge: Option<crate::bridge_probe::Probe> = None;
    let mut epoch = 0u64;
    let session = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    let mut status_buffer = [0u8; 4096];
    let mut discovery_buffer = [0u8; 4096];
    let mut assets: tokio::task::JoinSet<(String, Result<Asset, String>)> =
        tokio::task::JoinSet::new();
    let mut requested = std::collections::BTreeSet::new();
    let mut asset_tasks = BTreeMap::new();
    let mut local_server: Option<crate::local_serving::Server> = None;
    let mut local_prepares: tokio::task::JoinSet<(
        u8,
        crate::loading::Command,
        Result<crate::local_serving::Prepared, String>,
    )> = tokio::task::JoinSet::new();
    let mut local_busy = std::collections::BTreeSet::new();
    let mut local_queue: BTreeMap<
        u8,
        (
            crate::loading::Command,
            crate::local_serving::Key,
            u32,
            Ipv4Addr,
            Instant,
        ),
    > = BTreeMap::new();
    loop {
        let mut wake_ms = None;
        tokio::select! {
            Some(control) = controls.recv() => {
                let result = async {
                    if crate::cue_window::occupied() {return Err("Disable cue windows in TEST before changing connections or captures".into());}
                    if local_server.is_some() && peers.values().any(|p|p.observed.as_ref().is_some_and(|(s,at)|s.source_player().map(|n|n.get())==Some(crate::local_serving::NUMBER) && validate_load(s,at.elapsed()).is_err())) {return Err("Stop playback from the local USB before changing connections or starting diagnostics".into());}
                    if !local_busy.is_empty() {return Err("Wait for the pending local USB load before changing connections".into());}
                    if control.reply.is_closed() {return Err("Request cancelled".into());}
                    if bridge.is_some() { return Err("Stop the bridge capture before changing connections or starting another test".into()); }
                    if crate::jog_trace::guided_active() {return Err("Stop the guided capture before changing connections or starting another test".into());}
                    if (control.bridge || control.handoff || control.sync_tap) && crate::jog_trace::state()["active"]==true {return Err("Stop the current capture first".into());}
                    if control.handoff || control.sync_tap {
                        if peers.values().any(|p|p.pending.is_some()) {return Err("Wait for pending track loads to finish".into());}
                        if beat_socket.is_none() || extra_socket.is_none() {return Err("All telemetry receivers must be available for the guided capture".into());}
                        let players=handoff_players(&peers);
                        if control.sync_tap {
                            crate::handoff_capture::validate_sync_tap(players,true)?;
                            crate::jog_trace::start_sync_tap(players.map(|p|crate::handoff_capture::evidence(p.unwrap())));
                        } else {
                            crate::handoff_capture::validate(players,true)?;
                            crate::jog_trace::start_handoff(players.map(|p|crate::handoff_capture::evidence(p.unwrap())));
                        }
                        return Ok(());
                    }
                    if control.bridge {
                        if peers.len()!=2 || peers.values().any(|p|p.fresh().is_none() || p.pending.is_some()) {
                            return Err("Connect both CDJs and wait for fresh status and pending loads to finish".into());
                        }
                        if beat_socket.is_none() || extra_socket.is_none() { return Err("All telemetry receivers must be available for the bridge test".into()); }
                        if crate::library::sources(&catalogs)["sources"].as_array().is_some_and(|sources|sources.iter().any(|s|s["direct"]==true && s["state"]=="loading")) {
                            return Err("Wait for USB library reading to finish before starting the bridge test".into());
                        }
                        if peers.values().map(|p|p.local).collect::<std::collections::BTreeSet<_>>().len()!=1 {
                            return Err("Both CDJs must use the same local network interface".into());
                        }
                        assets.abort_all();
                        asset_tasks.clear(); requested.clear();
                        crate::library::invalidate_direct(&catalogs);
                        bridge = Some(crate::bridge_probe::Probe::start());
                        for p in peers.values() { crate::jog_trace::event(crate::bridge_probe::details(p.local,p.ip)); }
                        let mut state = shared.lock().unwrap();
                        state.decks.clear(); state.assets.clear(); publish(&state);
                        return Ok(());
                    }
                    if let Some(ip) = control.ip {
                        if peers.iter().any(|(n,p)| *n != control.number && p.ip == ip) {return Err("This IP is already assigned to the other CDJ".into());}
                        if peers.get(&control.number).is_some_and(|p|p.ip == ip && p.error.is_none()) {return Ok(());}
                        let route = UdpSocket::bind((Ipv4Addr::UNSPECIFIED,0)).await.map_err(|e|e.to_string())?;
                        route.connect((ip,discovery_port)).await.map_err(|e|e.to_string())?;
                        let std::net::IpAddr::V4(local) = route.local_addr().map_err(|e|e.to_string())?.ip() else {return Err("No IPv4 route".into());};
                        if let Some(mut old) = peers.remove(&control.number) {old.fail(&catalogs,"Reconnecting".into());}
                        crate::library::invalidate_direct_peer(&catalogs,ip);
                        epoch += 1;
                        peers.insert(control.number, Peer {ip,started:Instant::now(),epoch:epoch<<32,local,sent:0,next_send:Instant::now(),observed:None,order:Default::default(),usb:false,error:None,pending:None,bar:Default::default(),beat_tracker:Default::default(),pulse:None,loops:Default::default(),loop_at:None,loop_pulse_at:None,current_loop:None});
                    } else if let Some(mut old) = peers.remove(&control.number) {old.fail(&catalogs,"Disconnected".into());}
                    Ok(())
                }.await;
                let _ = control.reply.send(result);
            }
            result = discovery.recv_from(&mut discovery_buffer) => {
                let (n,from) = result.map_err(|e|e.to_string())?;
                if local_server.as_ref().is_some_and(|server|server.collision(&discovery_buffer[..n],from.ip())) {
                    return Err("Local USB source number 4 conflicts with another player; stop playback and choose a free player number before reconnecting".into());
                }
                if peers.values().any(|p|from.ip() == std::net::IpAddr::V4(p.ip)) {
                    crate::jog_trace::packet(50000,from,&discovery_buffer[..n]);
                }
                if let Some(probe) = &bridge
                    && peers.values().any(|p|crate::bridge_probe::collision(&discovery_buffer[..n],p.local)) {
                    probe.finish("Bridge identity conflict detected; registration stopped");
                }
                if bridge.is_none() && peers.values().any(|p|from.ip() == std::net::IpAddr::V4(p.ip)) && let Ok(p) = djl::Packet::decode(&discovery_buffer[..n])
                    && matches!(p.body,djl::Body::NumberInUse {device_number:15,..} | djl::Body::KeepAlive {device_number:15,..}) {return Err("Observer 15 is in use; disconnect other observer apps".into());}
            }
            result = socket.recv_from(&mut status_buffer) => {
                let received_at = Instant::now();
                let (n,from) = result.map_err(|e|e.to_string())?;
                if let Some(server) = &mut local_server {
                    if let Ok(status::Packet::CdjStatus(packet)) = status::decode(&status_buffer[..n])
                        && packet.sender().map(|n| n.get()) == Some(crate::local_serving::NUMBER)
                        && from.ip() != server.local {
                        return Err("Local USB source number 4 is already in use".into());
                    }
                    if let Some(peer) = peers.values().find(|p|from.ip()==std::net::IpAddr::V4(p.ip)) {
                        server.respond(&status_buffer[..n],peer.ip,&socket);
                    }
                }
                // Hardware replies may use an ephemeral source port; dispatch by IP only.
                if let Some((&number,peer)) = peers.iter_mut().find(|(_,p)| from.ip() == std::net::IpAddr::V4(p.ip) && p.error.is_none()) {
                    crate::jog_trace::packet(50002,from,&status_buffer[..n]);
                    if bridge.is_none() && let Ok(status::Packet::CdjStatus(s)) = status::decode(&status_buffer[..n]) {peer.observe(number,s,&catalogs,received_at);}
                }
            }
            result = diagnostic_receive(&beat_socket,&mut beat_buffer) => {
                let received_at = Instant::now();
                record_diagnostic(result.as_ref().copied().map_err(|e|std::io::Error::other(e.to_string())),50001,&beat_buffer,&peers,&mut beat_socket,&mut beat_state);
                if bridge.is_none() && let Ok((n,from)) = result
                    && let Some((&number,peer)) = peers.iter_mut().find(|(_,p)| from.ip() == std::net::IpAddr::V4(p.ip) && p.error.is_none())
                    && let Ok(prolink_proto::beat::Packet::Beat(beat)) = prolink_proto::beat::decode(&beat_buffer[..n])
                    && beat.device.get() == number {
                    peer.pulse = Some((beat,received_at));
                }
            }
            result = diagnostic_receive(&extra_socket,&mut extra_buffer) => {
                record_diagnostic(result,50004,&extra_buffer,&peers,&mut extra_socket,&mut extra_state);
            }
            Some(Ok((target, command, result))) = local_prepares.join_next(), if !local_prepares.is_empty() => {
                let result = async {
                    let prepared = result?;
                    if command.reply.is_closed() {return Err("Load request cancelled".to_owned());}
                    let peer=peers.get(&target).ok_or("Target disconnected")?;
                    let ip=peer.ip;
                    if local_server.is_none() {local_server=Some(crate::local_serving::Server::start(peer.local).await?);}
                    let key=prepared.key.clone();
                    let id=local_server.as_mut().unwrap().add(prepared)?;
                    Ok((key,id,ip))
                }.await;
                match result {
                    Ok((key,id,ip))=>{local_queue.insert(target,(command,key,id,ip,Instant::now()));},
                    Err(message)=>{local_busy.remove(&target);let _=command.reply.send(json!({"outcome":"error","message":message}));}
                }
            }
            Some(command) = commands.recv() => {
                if command.body["source"].as_str().is_some_and(|s|s.starts_with("local-usb:")) {
                    let validation = (|| {
                        if bridge.is_some() || crate::jog_trace::guided_active() {return Err("Stop diagnostic captures before loading a local track".to_owned());}
                        let target=command.body["target"].as_u64().and_then(|v|u8::try_from(v).ok()).ok_or("Invalid target")?;
                        let peer=peers.get(&target).ok_or("Target is not connected")?;
                        if local_busy.contains(&target) || peer.pending.is_some() {return Err("A load is already pending on this CDJ".into());}
                        let observed=peer.observed.as_ref().ok_or("Waiting for target status")?;
                        validate_load(peer.fresh().ok_or("Waiting for fresh target status")?,observed.1.elapsed())?;
                        Ok((target,crate::local_serving::key(&command.body)?))
                    })();
                    match validation {
                        Ok((target,key))=>{
                            local_busy.insert(target);
                            let catalogs=catalogs.clone();
                            local_prepares.spawn(async move {
                                let result=tokio::time::timeout(Duration::from_secs(12), tokio::task::spawn_blocking(move ||crate::local_serving::prepare(&catalogs,key)))
                                    .await.map_err(|_| "Local USB preparation timed out; no load command was sent".to_owned())
                                    .and_then(|r|r.map_err(|e|e.to_string())).and_then(|r|r);
                                (target,command,result)
                            });
                        },
                        Err(message)=>{let _=command.reply.send(json!({"outcome":"error","message":message}));}
                    }
                    continue;
                }
                let result = (|| -> Result<(u8, u8, Ipv4Addr, u32), String> {
                    if bridge.is_some() || crate::jog_trace::guided_active() {return Err("Track loading is disabled during the diagnostic capture".into());}
                    if command.reply.is_closed() {return Err("Request cancelled".into());}
                    let (target,source_number,source_ip,track) = load_selection(&command.body,&peers,&catalogs)?;
                    if local_busy.contains(&target) {return Err("A local load is already pending on this CDJ".into());}
                    let packet = crate::cdj_usb_load::LoadUsbTrack {sender:15,source:source_number,target,track_id:track}.encode().map_err(str::to_owned)?;
                    socket.try_send_to(&packet,(peers[&target].ip,50002).into()).map_err(|e|e.to_string())?;
                    Ok((target,source_number,source_ip,track))
                })();
                match result {
                    Ok((target,source_number,source_ip,track)) => peers.get_mut(&target).unwrap().pending = Some(Pending {source:command.body["source"].as_str().unwrap_or("").into(),source_number,source_ip,generation:command.body["generation"].as_u64().unwrap_or(0),track,sent:Instant::now(),matched:None,reply:command.reply}),
                    Err(message) => {let _ = command.reply.send(json!({"outcome":"error","message":message}));}
                }
            }
            Some(result) = assets.join_next(), if !assets.is_empty() => {
                if let Ok((key,result)) = result {
                    let asset = result.unwrap_or_else(|error|Asset {artwork:None,analysis:json!({"detail":null,"preview":null,"track":null}),beats:vec![],warning:Some(error)});
                    asset_tasks.remove(&key);
                    shared.lock().unwrap().assets.insert(key,asset);
                }
            }
            scheduled = tick.tick() => { wake_ms = Some(scheduled.elapsed().as_secs_f64()*1000.0); }
        }
        let work_started = Instant::now();
        if let Some(server) = &mut local_server {
            let destinations: Vec<_> = peers
                .values()
                .filter(|p| p.error.is_none())
                .map(|p| (p.ip, p.local))
                .collect();
            server.tick(&destinations, &discovery, &socket)?;
            let ready: Vec<_> = local_queue
                .iter()
                .filter(|(_, (_, _, _, ip, at))| {
                    server.ready(*ip) || at.elapsed() > Duration::from_secs(8)
                })
                .map(|(target, _)| *target)
                .collect();
            for target in ready {
                let (command, key, track, ip, _) = local_queue.remove(&target).unwrap();
                local_busy.remove(&target);
                let result = (|| {
                    if command.reply.is_closed() {
                        return Err("Load request cancelled".to_owned());
                    }
                    let peer = peers
                        .get(&target)
                        .filter(|p| p.ip == ip)
                        .ok_or("Target changed or disconnected")?;
                    if !server.ready(ip) {
                        return Err(format!(
                            "CDJ {target} has not discovered the local USB yet. Press LINK on CDJ {target}, wait for OLC LOCAL USB to appear, then load again. No load command was sent."
                        ));
                    }
                    if !crate::local_serving::current(&catalogs, &key) {
                        return Err("USB changed or disconnected; no load sent".into());
                    }
                    let s = peer.fresh().ok_or("Target status is stale; no load sent")?;
                    validate_load(s, peer.observed.as_ref().unwrap().1.elapsed())?;
                    crate::local_serving::compatible(
                        server.track(track).ok_or("Served track disappeared")?,
                        &s.name().as_str(),
                    )?;
                    if s.track_id() == track
                        && s.source_player().map(|n| n.get()) == Some(crate::local_serving::NUMBER)
                    {
                        return Err("Track already loaded; no command sent".into());
                    }
                    let packet = crate::cdj_usb_load::LoadUsbTrack {
                        sender: 15,
                        source: crate::local_serving::NUMBER,
                        target,
                        track_id: track,
                    }
                    .encode()
                    .map_err(str::to_owned)?;
                    socket
                        .try_send_to(&packet, (ip, 50002).into())
                        .map_err(|e| e.to_string())?;
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        peers.get_mut(&target).unwrap().pending = Some(Pending {
                            source: key.source,
                            generation: key.generation,
                            source_number: crate::local_serving::NUMBER,
                            source_ip: server.local,
                            track,
                            sent: Instant::now(),
                            matched: None,
                            reply: command.reply,
                        })
                    }
                    Err(message) => {
                        let _ = command
                            .reply
                            .send(json!({"outcome":"error","message":message}));
                    }
                }
            }
        }
        let cue_decks = [1, 2].map(|n| {
            peers.get(&n).and_then(|p| {
                p.observed.as_ref().map(|(s, at)| crate::cue_window::Deck {
                    status: s.clone(),
                    age: if p.error.is_some() {
                        u64::MAX
                    } else {
                        at.elapsed().as_millis() as u64
                    },
                    pending: p.pending.is_some(),
                })
            })
        });
        for number in
            crate::cue_window::tick(cue_decks, beat_socket.is_some() && extra_socket.is_some())
        {
            let packet = crate::cue_window::master_packet();
            if let Some(peer) = peers.get(&number) {
                let destination = (peer.ip, 50001).into();
                match socket.try_send_to(&packet, destination) {
                    Ok(_) => {
                        crate::jog_trace::transmitted(50001, destination, &packet);
                        crate::jog_trace::event(
                            json!({"phase":"master-command-sent","target":number}),
                        );
                    }
                    Err(e) => crate::cue_window::send_failed(e.to_string()),
                }
            }
        }

        if crate::jog_trace::guided_active() {
            let players = handoff_players(&peers);
            let result = if crate::jog_trace::sync_tap_active() {
                crate::handoff_capture::validate_sync_tap(players, false)
            } else {
                crate::handoff_capture::validate(players, false)
            };
            crate::jog_trace::update_handoff(
                players.map(|p| {
                    p.map(crate::handoff_capture::evidence)
                        .unwrap_or(Value::Null)
                }),
                result,
            );
        } else {
            crate::jog_trace::finish_handoff_if_expired();
        }
        crate::jog_trace::receivers(
            json!({"mode":"direct","50000":{"bound":true},"50002":{"bound":true},"50001":beat_state,"50004":extra_state}),
        );
        if let Some(probe) = &mut bridge {
            if probe.active() && (beat_socket.is_none() || extra_socket.is_none()) {
                probe.finish("Telemetry receiver failed; registration stopped");
            }
            if probe.active() {
                if probe.due(Instant::now()) {
                    for peer in peers.values() {
                        let packet = crate::bridge_probe::announcement(peer.local);
                        match discovery.send_to(&packet, (peer.ip, discovery_port)).await {
                            Ok(_) => crate::jog_trace::transmitted(
                                50000,
                                (peer.ip, discovery_port).into(),
                                &packet,
                            ),
                            Err(error) => {
                                probe.finish(&format!("Bridge send failed: {error}"));
                                break;
                            }
                        }
                    }
                }
                continue;
            }
            probe.finish("Capture ended; restoring normal observer connection");
            bridge = None;
            for peer in peers.values_mut() {
                peer.started = Instant::now();
                peer.next_send = Instant::now();
                peer.sent = 0;
                peer.observed = None;
                peer.error = None;
                peer.usb = false;
                peer.epoch += 1;
                peer.order = Default::default();
                peer.bar = Default::default();
                peer.beat_tracker = Default::default();
                peer.pulse = None;
                peer.loops = Default::default();
                peer.loop_at = None;
                peer.loop_pulse_at = None;
                peer.current_loop = None;
            }
        }
        for peer in peers.values_mut().filter(|p| p.error.is_none()) {
            if peer
                .observed
                .as_ref()
                .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(2))
                || (peer.observed.is_none() && peer.started.elapsed() > Duration::from_secs(12))
            {
                peer.fail(
                    &catalogs,
                    "Direct status lost. Reconnect this CDJ; loads are disabled.".into(),
                );
                continue;
            }
            if Instant::now() >= peer.next_send {
                let stages = crate::direct_status::registration(peer.local);
                let keepalive = crate::direct_status::announcement(peer.local);
                if let Err(e) = discovery
                    .send_to(
                        stages.get(peer.sent).unwrap_or(&keepalive),
                        (peer.ip, discovery_port),
                    )
                    .await
                {
                    peer.fail(&catalogs, format!("Could not contact CDJ: {e}"));
                    continue;
                }
                peer.sent += 1;
                peer.next_send = Instant::now()
                    + Duration::from_millis(if peer.sent <= stages.len() { 300 } else { 1500 });
            }
        }
        let fresh_sources: std::collections::BTreeSet<_> = peers
            .iter()
            .filter(|(_, p)| p.fresh().is_some())
            .map(|(n, _)| *n)
            .collect();
        let sources: BTreeMap<_, _> = peers
            .iter()
            .filter(|(_, p)| p.error.is_none() && p.usb && p.observed.is_some())
            .map(|(n, p)| (*n, (p.ip, p.epoch)))
            .collect();
        let mut decks = vec![];
        let mut keys = std::collections::BTreeSet::new();
        for (&number, peer) in &mut peers {
            if let Some(p) = &mut peer.pending {
                let source_valid = if p.source.starts_with("local-usb:") {
                    local_server.is_some()
                        && matches!(
                            crate::library::artwork_location(&catalogs, &p.source, p.generation),
                            Some((crate::library::Location::Local(_), _))
                        )
                } else {
                    fresh_sources.contains(&p.source_number)
                        && sources
                            .get(&p.source_number)
                            .is_some_and(|(ip, _)| *ip == p.source_ip)
                        && crate::library::artwork_location(&catalogs, &p.source, p.generation)
                            .is_some_and(|(l, _)| {
                                l == crate::library::Location::Direct(p.source_ip)
                            })
                };
                let matches = source_valid
                    && peer.observed.as_ref().is_some_and(|(s, at)| {
                        at.elapsed() < Duration::from_secs(1)
                            && s.track_id() == p.track
                            && s.source_player().map(|n| n.get()) == Some(p.source_number)
                            && s.source_slot() == Slot::USB
                            && s.track_type() == 1
                            && matches!(s.play_state(), Some(3..=6))
                    });
                if matches {
                    p.matched.get_or_insert(Instant::now());
                } else {
                    p.matched = None;
                }
                let confirmed = p
                    .matched
                    .is_some_and(|t| t.elapsed() > Duration::from_millis(500));
                if confirmed || !source_valid || p.sent.elapsed() > Duration::from_secs(12) {
                    let p = peer.pending.take().unwrap();
                    if confirmed {
                        let _ = p.reply.send(json!({"outcome":"confirmed","message":"CDJ status confirms the selected track is loaded"}));
                    } else {
                        unknown(p);
                    }
                }
            }
            let Some((s, _at)) = &peer.observed else {
                continue;
            };
            let source = s.source_player().and_then(|n| sources.get(&n.get()));
            let supported = s.track_id() > 0 && s.source_slot() == Slot::USB && s.track_type() == 1;
            let mut key = source
                .filter(|_| supported)
                .map(|(ip, epoch)| format!("direct:{session}:{ip}:{epoch}:{}", s.track_id()));
            if supported
                && s.source_player().map(|n| n.get()) == Some(crate::local_serving::NUMBER)
                && let Some(server) = &local_server
            {
                let local_key = format!("local:{session}:{}", s.track_id());
                if !shared.lock().unwrap().assets.contains_key(&local_key)
                    && let Some(asset) = server.asset(s.track_id())
                {
                    shared
                        .lock()
                        .unwrap()
                        .assets
                        .insert(local_key.clone(), asset);
                }
                key = Some(local_key);
            }
            if let Some(key) = &key {
                keys.insert(key.clone());
                if let Some((ip, _)) = source
                    && requested.insert(key.clone())
                {
                    let key = key.clone();
                    let catalogs = catalogs.clone();
                    let ip = *ip;
                    let track = LoadedTrack {
                        id: s.track_id(),
                        source_player: s.source_player().unwrap(),
                        slot: Slot::USB,
                        kind: TrackKind(1),
                    };
                    let request_key = key.clone();
                    let task = assets.spawn(async move {
                        let result = tokio::time::timeout(
                            Duration::from_secs(35),
                            fetch_asset(ip, track, None, &catalogs),
                        )
                        .await
                        .map_err(|_| "Analysis timed out".to_string())
                        .and_then(|r| r);
                        (key, result)
                    });
                    asset_tasks.insert(request_key, task);
                }
            }
            let state = shared.lock().unwrap();
            decks.push(render_peer(number, peer, &key, &state, session));
        }
        let mut state = shared.lock().unwrap();
        state.assets.retain(|k, _| keys.contains(k));
        // Do not retain old USB/track keys indefinitely. Running fetches may finish
        // later, but results with obsolete keys are pruned before publication.
        asset_tasks.retain(|k, task| {
            if keys.contains(k) {
                true
            } else {
                task.abort();
                false
            }
        });
        requested.retain(|k| keys.contains(k));
        state.decks = decks;
        state.enabled = !peers.is_empty();
        state.error = None;
        state.direct_peers = peers.iter().map(|(n,p)|json!({"number":n,"ip":p.ip.to_string(),"state":if p.error.is_some(){"error"}else if p.observed.is_some(){"connected"}else{"connecting"},"error":p.error,"usb":p.usb})).collect();
        state.last_update = Instant::now();
        publish(&state);
        drop(state);
        crate::jog_trace::loop_health(work_started.elapsed().as_secs_f64() * 1000.0, wake_ms);
    }
}

fn render_peer(
    number: u8,
    peer: &mut Peer,
    key: &Option<String>,
    state: &LiveState,
    session: u128,
) -> Value {
    let (s, at) = peer.observed.as_ref().unwrap();
    let asset = key.as_ref().and_then(|k| state.assets.get(k));
    let decoded = prolink::monitor::PlayerStatus::from_packet(s);
    let fine_position = key.as_ref().and_then(|k| {
        asset.and_then(|a| peer.bar.position(k, decoded, &a.beats, *at, Instant::now()))
    });
    if peer.loop_at != Some(*at) || peer.loop_pulse_at != peer.pulse.map(|(_, t)| t) {
        // Observe each real packet once; polling must not create phase samples.
        let mut loop_status = decoded;
        if fine_position.is_none_or(|p| p.held) {
            loop_status.bar_position = None;
        }
        peer.current_loop = peer.loops.observe(
            key.as_deref().unwrap_or(""),
            loop_status,
            peer.pulse
                .map(|(beat, received_at)| prolink::monitor::BeatObservation {
                    beat,
                    received_at,
                    age: received_at.elapsed(),
                }),
            asset.map_or(&[], |a| a.beats.as_slice()),
            *at,
        );
        peer.loop_at = Some(*at);
        peer.loop_pulse_at = peer.pulse.map(|(_, t)| t);
    }
    let tracked = key.as_ref().and_then(|k| {
        asset.and_then(|a| {
            let region = (decoded.play_state.0 == 4)
                .then_some(peer.current_loop.as_ref())
                .flatten()
                .and_then(|r| Some((r["start"].as_f64()?, r["end"].as_f64()?)));
            peer.beat_tracker.configure_loop(k, region, &a.beats);
            peer.beat_tracker
                .observe(k, decoded, *at, peer.pulse, &a.beats, Instant::now())
        })
    });
    let saved_cue = asset.and_then(|a| {
        crate::cues::saved_position(
            decoded,
            &a.beats,
            a.analysis["cues"].as_array().map_or(&[], |c| c.as_slice()),
        )
    });
    let position = tracked
        .map(|p| p.seconds)
        .or_else(|| fine_position.map(|p| p.seconds))
        .or(saved_cue)
        .or_else(|| {
            asset.and_then(|a| {
                s.beat_number().and_then(|n| {
                    if n == 0 {
                        Some(0.0)
                    } else {
                        a.beats.get((n - 1) as usize).copied()
                    }
                })
            })
        });
    let playing = decoded.is_playing;
    let play_state = play_state_label(decoded);
    json!({"number":number,"name":s.name().as_str(),"ip":peer.ip.to_string(),"connection":if at.elapsed()<Duration::from_secs(1) {"connected"} else {"stale"},"statusAgeMs":at.elapsed().as_secs_f64()*1000.0,"observationId":format!("{session}:{}:{}:{}",peer.epoch,at.duration_since(peer.started).as_nanos(),tracked.map_or(0,|p|p.at.duration_since(peer.started).as_nanos())),"positionAgeMs":tracked.map(|p|p.at).or_else(||fine_position.and_then(|_|peer.bar.observed_at())).map(|at|at.elapsed().as_secs_f64()*1000.0),"motionRate":tracked.map(|p|p.rate),"beatAnchorNumber":tracked.map(|p|p.beat_number),"beatCorrectionMs":tracked.map(|p|p.correction_seconds*1000.0),"beatArrivalResidualMs":tracked.map(|p|p.arrival_residual_seconds*1000.0),"observationTimeMs":at.duration_since(peer.started).as_secs_f64()*1000.0,"packetCounter":s.packet_counter(),"playState":play_state,"playing":playing,"loadProtected":validate_load(s,at.elapsed()).is_err(),"bpm":s.effective_bpm(),"pitch":s.pitch().map(|p|(p.multiplier()-1.0)*100.0),"master":s.flags().map(|f|f.is_tempo_master()),"sync":s.flags().map(|f|f.is_synced()),"trackId":s.track_id(),"trackKey":key,"assetReady":asset.is_some(),"beatNumber":s.beat_number(),"loop":peer.current_loop,"currentCue":if decoded.play_state.0 == 6 {fine_position.map(|p|p.seconds).or(saved_cue)} else {None},"position":position,"positionSource":if tracked.is_some() {"beat-motion"} else if fine_position.is_some() {"bar-phase"} else if saved_cue.is_some() {"saved-cue-estimate"} else {"status-beat-estimate"},"positionQuality":if tracked.is_some() {"beat"} else {fine_position.map_or(if position.is_some() {"coarse"} else {"unavailable"},|p|p.quality())},"manualMotion":tracked.is_none() && decoded.position_requires_direct_updates(),"reverse":decoded.reverse,"sourceLabel":if s.source_player().map(|n|n.get())==Some(crate::local_serving::NUMBER) {"Local USB"} else {"Direct IP · USB"},"qualifyingPlayback":at.elapsed() < Duration::from_secs(2) && qualifies_for_set(decoded),"warning":asset.and_then(|a|a.warning.clone()).unwrap_or_else(||if tracked.is_some() {"Direct IP: beat and motion-speed tracking; loop boundaries estimated".into()} else {"Direct IP: status-based position; waiting for fresh forward beat events".into()})})
}

#[cfg(test)]
mod set_history_tests {
    use super::*;

    #[test]
    fn direct_status_feeds_set_recorder_only_during_fresh_playback() {
        let now = Instant::now();
        let mut peer = Peer {
            ip: Ipv4Addr::LOCALHOST,
            local: Ipv4Addr::LOCALHOST,
            started: now - Duration::from_secs(10),
            epoch: 1,
            sent: 0,
            next_send: now,
            observed: None,
            order: Default::default(),
            usb: true,
            error: None,
            pending: None,
            bar: Default::default(),
            beat_tracker: Default::default(),
            pulse: None,
            loops: Default::default(),
            loop_at: None,
            loop_pulse_at: None,
            current_loop: None,
        };
        let shared = Arc::new(Mutex::new(LiveState::default()));
        for (raw, playing, age, expected) in [
            (3, true, 0, true),
            (4, true, 0, true),
            (4, false, 0, false),
            (5, false, 0, false),
            (6, true, 0, false),
            (7, true, 0, false),
            (3, true, 3, false),
        ] {
            peer.observed = Some((
                status::CdjStatus::builder()
                    .play_state(raw)
                    .playing(playing)
                    .build(),
                now - Duration::from_secs(age),
            ));
            {
                let mut state = shared.lock().unwrap();
                state.decks = vec![render_peer(
                    1,
                    &mut peer,
                    &Some("direct:test:42".into()),
                    &state,
                    1,
                )];
            }
            let samples = set_samples(&shared);
            assert_eq!(samples.len(), 1);
            assert_eq!(
                samples[0].playing, expected,
                "state {raw}, flag {playing}, age {age}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn optional_diagnostics_receive_ephemeral_source_and_disable_on_error() {
        let (mut receiver, mut state) = diagnostic_socket(0).await;
        let address = receiver.as_ref().unwrap().local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(&[0x28, 1, 2], (Ipv4Addr::LOCALHOST, address.port()))
            .await
            .unwrap();
        let mut bytes = [0; 20];
        let received = tokio::time::timeout(
            Duration::from_secs(1),
            diagnostic_receive(&receiver, &mut bytes),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(received.1.port(), sender.local_addr().unwrap().port());
        assert_eq!(&bytes[..received.0], &[0x28, 1, 2]);
        // A diagnostic socket failure never invalidates or changes a CDJ peer.
        let peers = BTreeMap::from([(1, peer(Ipv4Addr::LOCALHOST, 1, true))]);
        record_diagnostic(
            Err(std::io::Error::other("test failure")),
            50001,
            &bytes,
            &peers,
            &mut receiver,
            &mut state,
        );
        assert!(receiver.is_none());
        assert_eq!(state["bound"], false);
        assert!(peers[&1].observed.is_some());
        assert!(
            tokio::time::timeout(
                Duration::from_millis(5),
                diagnostic_receive(&receiver, &mut bytes)
            )
            .await
            .is_err()
        );
    }
    #[test]
    fn direct_whole_beat_positions_are_labeled_for_clock_calibration() {
        for number in [1, 2] {
            let mut peer = peer(Ipv4Addr::LOCALHOST, number, true);
            let mut raw = status(3, true, status::MediaState::LOADED).into_bytes();
            raw[0xa0..0xa4].copy_from_slice(&3u32.to_be_bytes());
            peer.observed = Some((status::CdjStatus::parse(&raw).unwrap(), Instant::now()));
            let mut state = LiveState::default();
            state.assets.insert(
                "test".into(),
                Asset {
                    artwork: None,
                    analysis: json!({}),
                    beats: vec![0.0, 0.5, 1.0, 1.5],
                    warning: None,
                },
            );
            let result = render_peer(number, &mut peer, &Some("test".into()), &state, 1);
            assert_eq!(result["position"], 1.0);
            assert_eq!(result["positionSource"], "status-beat-estimate");
            assert_eq!(result["positionQuality"], "coarse");
        }
    }
    #[test]
    fn beat_publication_preserves_load_protection_and_uses_each_players_motion_rate() {
        for master in [false, true] {
            let mut peer = peer(Ipv4Addr::LOCALHOST, 1, true);
            let t = Instant::now() - Duration::from_millis(300);
            peer.started = t;
            let mut state = LiveState::default();
            state.assets.insert(
                "track".into(),
                Asset {
                    artwork: None,
                    analysis: json!({}),
                    beats: (0..20).map(|i| i as f64 * 0.5).collect(),
                    warning: None,
                },
            );
            for ms in [0, 100, 200] {
                let mut raw = status::CdjStatus::builder()
                    .play_state(3)
                    .playing(true)
                    .build()
                    .into_bytes();
                raw[0x9e] = u8::from(master);
                raw[0x8b] = 0xfa;
                raw[0x9d] = 9;
                raw[0x98..0x9c].copy_from_slice(&983_564u32.to_be_bytes());
                raw[0xa0..0xa4].copy_from_slice(&10u32.to_be_bytes());
                raw[0xa6] = 2;
                let at = t + Duration::from_millis(ms);
                peer.observed = Some((status::CdjStatus::parse(&raw).unwrap(), at));
                if ms == 200 {
                    peer.pulse = Some((
                        prolink_proto::beat::Beat {
                            name: prolink_proto::DeviceName::new("CDJ-2000nexus"),
                            device: prolink_proto::DeviceNumber::ONE,
                            timings: Default::default(),
                            pitch: prolink_proto::beat::Pitch::UNITY,
                            bpm_centi: 12000,
                            beat_in_bar: prolink_proto::beat::BeatInBar::new(3),
                            scratching: false,
                        },
                        at,
                    ));
                }
                let p = render_peer(1, &mut peer, &Some("track".into()), &state, 1);
                assert_eq!(p["loadProtected"], true);
                if ms == 200 {
                    assert_eq!(p["positionSource"], "beat-motion");
                    assert_eq!(p["positionQuality"], "beat");
                    assert_eq!(p["position"], 5.);
                    assert_eq!(p["beatAnchorNumber"], 11);
                    assert_eq!(p["manualMotion"], false);
                    assert!(
                        (p["motionRate"].as_f64().unwrap() - 983_564. / 1_048_576.).abs() < 1e-9
                    );
                    assert!(p["positionAgeMs"].as_f64().unwrap() >= 100.);
                }
            }
        }
    }
    fn status(raw: u8, playing: bool, usb: status::MediaState) -> status::CdjStatus {
        status::CdjStatus::builder()
            .device_number(prolink_proto::DeviceNumber::ONE)
            .play_state(raw)
            .playing(playing)
            .slot_state(Slot::USB, usb)
            .build()
    }
    fn peer(ip: Ipv4Addr, number: u8, usb: bool) -> Peer {
        let s = status::CdjStatus::builder()
            .device_number(prolink_proto::DeviceNumber::new(number).unwrap())
            .play_state(5)
            .playing(false)
            .slot_state(
                Slot::USB,
                if usb {
                    status::MediaState::LOADED
                } else {
                    status::MediaState::EMPTY
                },
            )
            .build();
        Peer {
            ip,
            started: Instant::now(),
            epoch: 1,
            local: Ipv4Addr::LOCALHOST,
            sent: 0,
            next_send: Instant::now(),
            observed: Some((s, Instant::now())),
            order: Default::default(),
            usb,
            error: None,
            pending: None,
            bar: Default::default(),
            beat_tracker: Default::default(),
            pulse: None,
            loops: Default::default(),
            loop_at: None,
            loop_pulse_at: None,
            current_loop: None,
        }
    }
    #[test]
    fn dual_loads_resolve_source_usb_and_target_independently() {
        let catalogs = crate::library::with_local_path(None, None);
        let a = Ipv4Addr::new(192, 0, 2, 1);
        let b = Ipv4Addr::new(192, 0, 2, 2);
        let (id, generation) = crate::library::install_test_direct_catalog(&catalogs, a, 42);
        let mut peers = BTreeMap::from([(1, peer(a, 1, true)), (2, peer(b, 2, false))]);
        let body = json!({"source":id,"generation":generation,"trackId":42,"target":2});
        assert_eq!(
            load_selection(&body, &peers, &catalogs).unwrap(),
            (2, 1, a, 42)
        );
        let (second, other_generation) =
            crate::library::install_test_direct_catalog(&catalogs, b, 42);
        peers.insert(2, peer(b, 2, true));
        let reverse =
            json!({"source":second,"generation":other_generation,"trackId":42,"target":1});
        assert_eq!(
            load_selection(&reverse, &peers, &catalogs).unwrap(),
            (1, 2, b, 42)
        );
        let (reply, _rx) = oneshot::channel();
        peers.get_mut(&1).unwrap().pending = Some(Pending {
            source: id.clone(),
            source_number: 1,
            source_ip: a,
            generation,
            track: 42,
            sent: Instant::now(),
            matched: None,
            reply,
        });
        // A pending load on CDJ 1 must not block the other target.
        assert!(load_selection(&body, &peers, &catalogs).is_ok());
        peers.get_mut(&2).unwrap().observed.as_mut().unwrap().1 =
            Instant::now() - Duration::from_secs(2);
        assert!(load_selection(&body, &peers, &catalogs).is_err());
        peers.insert(2, peer(b, 2, false));
        peers
            .get_mut(&1)
            .unwrap()
            .fail(&catalogs, "Disconnected".into());
        assert!(load_selection(&body, &peers, &catalogs).is_err());
        assert!(peers[&2].fresh().is_some());
    }
    #[test]
    fn identity_mismatch_and_usb_removal_only_invalidate_that_source() {
        let catalogs = crate::library::with_local_path(None, None);
        let a = Ipv4Addr::new(192, 0, 2, 1);
        let b = Ipv4Addr::new(192, 0, 2, 2);
        let (first, generation) = crate::library::install_test_direct_catalog(&catalogs, a, 42);
        let (second, other) = crate::library::install_test_direct_catalog(&catalogs, b, 42);
        let mut first_peer = peer(a, 1, true);
        let mut second_peer = peer(b, 2, true);
        first_peer.observe(
            1,
            status(5, false, status::MediaState::EMPTY),
            &catalogs,
            Instant::now(),
        );
        assert!(first_peer.error.is_none());
        assert!(!first_peer.usb);
        assert!(crate::library::artwork_location(&catalogs, &first, generation).is_none());
        assert!(crate::library::artwork_location(&catalogs, &second, other).is_some());
        second_peer.observe(
            2,
            status(5, false, status::MediaState::LOADED),
            &catalogs,
            Instant::now(),
        );
        assert!(
            second_peer
                .error
                .as_ref()
                .unwrap()
                .contains("Expected CDJ 2")
        );
        assert!(first_peer.fresh().is_some());
    }
    #[test]
    fn shared_state_names_include_cue_audition_and_paused_loops() {
        for (raw, playing, expected) in [
            (7, false, "cue play"),
            (6, false, "cued"),
            (4, false, "paused"),
            (4, true, "looping"),
        ] {
            let decoded = prolink::monitor::PlayerStatus::from_packet(&status(
                raw,
                playing,
                status::MediaState::LOADED,
            ));
            assert_eq!(play_state_label(decoded), expected);
            if raw == 7 {
                assert!(decoded.is_playing);
            }
        }
    }
    #[test]
    fn packet_order_handles_duplicates_reordering_wrap_and_fixed_counters() {
        use prolink::monitor::StatusPacketOrder;
        let mut order = StatusPacketOrder::default();
        let now = Instant::now();
        assert!(order.accept(Some(u32::MAX - 1), now));
        assert!(order.accept(Some(u32::MAX), now));
        assert!(!order.accept(Some(u32::MAX), now));
        assert!(order.accept(Some(0), now));
        assert!(!order.accept(Some(u32::MAX - 1), now));
        assert!(order.accept(Some(1), now));
        assert!(order.accept(Some(0), now + Duration::from_secs(2)));
        assert!(order.accept(Some(0), now + Duration::from_millis(2100)));
        assert!(order.accept(None, now + Duration::from_millis(2200)));
    }
    #[test]
    fn loading_requires_fresh_idle_target_but_not_a_local_usb() {
        for raw in [0, 5, 6] {
            assert!(
                validate_load(
                    &status(raw, false, status::MediaState::LOADED),
                    Duration::from_millis(999)
                )
                .is_ok()
            );
        }
        for raw in [2, 3, 4, 7, 8, 9, 18, 255] {
            assert!(
                validate_load(
                    &status(raw, false, status::MediaState::LOADED),
                    Duration::ZERO
                )
                .is_err()
            );
        }
        assert!(
            validate_load(&status(5, true, status::MediaState::LOADED), Duration::ZERO).is_err()
        );
        assert!(
            validate_load(
                &status(5, false, status::MediaState::LOADED),
                Duration::from_secs(1)
            )
            .is_err()
        );
        assert!(
            validate_load(&status(5, false, status::MediaState::EMPTY), Duration::ZERO).is_ok()
        );
    }
    #[tokio::test]
    async fn unicast_accepts_ephemeral_source_ports_and_disables_load_after_loss() {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let replies = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let shared = Arc::new(Mutex::new(LiveState::default()));
        let catalogs = crate::library::with_local_path(None, None);
        configure_at(
            &shared,
            &catalogs,
            1,
            Some(Ipv4Addr::LOCALHOST),
            peer.local_addr().unwrap().port(),
        )
        .await
        .unwrap();
        assert!(start_bridge(&shared).await.is_err()); // Requires both fresh CDJs.
        let mut buffer = [0; 2048];
        let (_, from) = tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        let packet = status(3, true, status::MediaState::LOADED);
        replies
            .send_to(packet.as_bytes(), (from.ip(), 50002))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !shared.lock().unwrap().decks.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(shared.lock().unwrap().decks[0]["playing"], true);
        let first_id = shared.lock().unwrap().decks[0]["observationId"].clone();
        replies
            .send_to(
                status(7, false, status::MediaState::LOADED).as_bytes(),
                (from.ip(), 50002),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let changed = {
                    let state = shared.lock().unwrap();
                    state.decks[0]["playState"] == "cue play"
                        && state.decks[0]["observationId"] != first_id
                };
                if changed {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(shared.lock().unwrap().decks[0]["playing"], true);
        // Both controls share the existing socket pair; a duplicate address is rejected.
        assert!(
            configure_at(
                &shared,
                &catalogs,
                2,
                Some(Ipv4Addr::LOCALHOST),
                peer.local_addr().unwrap().port()
            )
            .await
            .is_err()
        );
        configure_at(
            &shared,
            &catalogs,
            2,
            Some(Ipv4Addr::new(127, 0, 0, 2)),
            peer.local_addr().unwrap().port(),
        )
        .await
        .unwrap();
        let result = super::super::load(&shared, json!({"target":1})).await;
        assert_eq!(result["outcome"], "error");
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if shared.lock().unwrap().direct_peers[0]["state"] == "error" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(shared.lock().unwrap().decks.is_empty());
        assert_eq!(
            shared.lock().unwrap().direct_peers[1]["state"],
            "connecting"
        );
        configure_at(
            &shared,
            &catalogs,
            1,
            None,
            peer.local_addr().unwrap().port(),
        )
        .await
        .unwrap();
        assert!(shared.lock().unwrap().enabled);
        assert_eq!(shared.lock().unwrap().direct_peers[0]["number"], 2);
        configure_at(
            &shared,
            &catalogs,
            2,
            None,
            peer.local_addr().unwrap().port(),
        )
        .await
        .unwrap();
        assert!(!shared.lock().unwrap().enabled);
        stop(&shared, &catalogs).unwrap();
    }
    #[tokio::test]
    async fn usb_handover_releases_idle_socket_and_preserves_active_session() {
        let shared = Arc::new(Mutex::new(LiveState::default()));
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        shared.lock().unwrap().direct_task = Some(tokio::spawn(async move {
            let _socket = socket;
            std::future::pending::<()>().await;
        }));
        shared.lock().unwrap().enabled = true;
        assert!(release_idle_transport(&shared).await.is_err());
        assert!(UdpSocket::bind(address).await.is_err());
        shared.lock().unwrap().enabled = false;
        release_idle_transport(&shared).await.unwrap();
        assert!(UdpSocket::bind(address).await.is_ok());
        assert!(shared.lock().unwrap().direct_task.is_none());
    }
    #[tokio::test]
    async fn disconnect_clears_live_state_and_assets() {
        let catalogs = crate::library::with_local_path(None, None);
        let shared = Arc::new(Mutex::new(LiveState::default()));
        {
            let mut s = shared.lock().unwrap();
            s.enabled = true;
            s.decks = vec![json!({"number":1})];
            s.direct_task = Some(tokio::spawn(std::future::pending()));
            s.loader = Some(crate::loading::channel().0);
        }
        stop(&shared, &catalogs).unwrap();
        let s = shared.lock().unwrap();
        assert!(!s.enabled);
        assert!(s.loader.is_none());
        assert!(s.decks.is_empty());
        assert!(s.direct_task.is_none());
    }
}
