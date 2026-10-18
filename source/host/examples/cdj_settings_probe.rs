//! Isolated CDJ-2000NXS settings feasibility worker. JSON-lines on stdin/stdout.
//! Run through scripts/cdj-settings-test.py so OLC is restored and pcap is saved.
use prolink::serve::{MediaSet, Medium, ServedSlot, Vfs};
use prolink::serve::{
    dbserver::{DbServer, DbServerConfig},
    nfs::{NfsConfig, NfsServer},
};
use prolink::virtual_cdj::Numbering;
use prolink::{Discovery, Interface, Slot, VirtualCdj, VirtualCdjConfig};
use prolink_proto::{DeviceName, DeviceNumber, status};
use prolink_rekordbox::SettingsFile;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, BufRead},
    net::Ipv4Addr,
    path::Path,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tokio::{net::UdpSocket, sync::mpsc};

#[path = "../src/cdj_settings_command.rs"]
mod cdj_settings_command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PushMethod {
    LoadCommand,
    LegacyResponse,
}
impl PushMethod {
    fn parse(value: &Value) -> Result<Self, &'static str> {
        match value.as_str() {
            Some("load-command") => Ok(Self::LoadCommand),
            Some("legacy-response") => Ok(Self::LegacyResponse),
            _ => Err("Reload the test page and select a send method"),
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::LoadCommand => "load-command",
            Self::LegacyResponse => "legacy-response",
        }
    }
    fn kind(self) -> u8 {
        match self {
            Self::LoadCommand => 0x34,
            Self::LegacyResponse => 0x36,
        }
    }
    fn encode(
        self,
        name: DeviceName,
        sender: DeviceNumber,
        target: DeviceNumber,
        settings: &[u8],
    ) -> Result<Vec<u8>, &'static str> {
        match self {
            Self::LoadCommand => {
                cdj_settings_command::encode(&name.0, sender.get(), target.get(), settings)
                    .map(|p| p.to_vec())
            }
            Self::LegacyResponse => {
                Ok(
                    status::SettingsResponse::build(name, sender, target, Slot::USB, settings)
                        .as_bytes()
                        .to_vec(),
                )
            }
        }
    }
}

fn event(value: Value) {
    println!("{value}");
}
fn packet(direction: &str, peer: Ipv4Addr, data: &[u8], reason: &str) {
    event(
        json!({"event":"packet","direction":direction,"peer":peer,"reason":reason,
        "kind":data.get(10),"hex":data.iter().map(|b|format!("{b:02x}")).collect::<String>()}),
    );
}
fn profile(original: &[u8], brightness: u8) -> Result<Vec<u8>, String> {
    if original.len() != 32 || !(0x81..=0x85).contains(&original[1]) {
        return Err("Unsupported MYSETTING.DAT brightness layout".into());
    }
    let mut bytes = original.to_vec();
    if brightness != 0 {
        if !(1..=5).contains(&brightness) {
            return Err("Brightness must be 1–5 or 0 (original)".into());
        }
        // MYSETTING payload byte 9, after its 8-byte prefix. See rekordcrate setting.rs.
        bytes[1] = 0x80 + brightness;
    }
    Ok(bytes)
}
#[derive(Clone)]
struct Peer {
    number: DeviceNumber,
    seen: Instant,
    stopped: bool,
}
struct Arm {
    method: PushMethod,
    ip: Ipv4Addr,
    mac: prolink_proto::MacAddress,
    number: DeviceNumber,
    deadline: Instant,
    absent: bool,
    returned: Option<Instant>,
}
fn eligible(peer: &Peer) -> bool {
    peer.seen.elapsed() < Duration::from_secs(2) && peer.stopped
}
fn source_status(name: DeviceName, number: DeviceNumber, counter: u32) -> status::CdjStatus {
    status::CdjStatus::builder()
        .name(name)
        .device_number(number)
        .slot_state(Slot::USB, status::MediaState::LOADED)
        .slot_state(Slot::SD, status::MediaState::EMPTY)
        .link_available(true)
        .packet_counter(counter)
        .playing(false)
        .tempo_master(false)
        .synced(false)
        .play_state(0)
        .build()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let result = run().await;
    if let Err(error) = &result {
        event(json!({"event":"fatal","message":error.to_string()}));
    }
    result
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "--check-command" {
        // Deployment verification: encode from the real export, without opening
        // sockets, announcing a source, stopping OLC, or sending to a player.
        let file = SettingsFile::parse(&std::fs::read(
            Path::new(&args[1]).join(Medium::SETTINGS_PATH),
        )?)?;
        let settings = profile(file.wire_settings(), 1)?;
        let raw = PushMethod::LoadCommand.encode(
            DeviceName::new("OLC SETTINGS TEST"),
            DeviceNumber::new(4).unwrap(),
            DeviceNumber::ONE,
            &settings,
        )?;
        event(
            json!({"event":"codec-check","workerVersion":2,"commandKind":raw[10],"bytes":raw.len(),"brightness":raw[0x2d]-0x80,"hex":raw.iter().map(|b|format!("{b:02x}")).collect::<String>(),"networkOpened":false}),
        );
        return Ok(());
    }
    if args.len() != 2 {
        return Err(
            "usage: cdj_settings_probe INTERFACE EXPORT_ROOT | --check-command EXPORT_ROOT".into(),
        );
    }
    let interface = Interface::named(&args[0])?;
    let root = Path::new(&args[1]);
    let raw = std::fs::read(root.join(Medium::SETTINGS_PATH))?;
    let file = SettingsFile::parse(&raw)?;
    if file.payload.len() != 40 || !file.has_magic() {
        return Err("Expected first-generation MYSETTING.DAT".into());
    }
    let original = profile(file.wire_settings(), 0)?;
    let mut selected = original.clone();
    let source = Arc::new(Medium::from_volume(root, ServedSlot::USB)?);
    if source.library().tracks.is_empty() {
        return Err("Baseline needs a nonempty traditional export.pdb".into());
    }
    // Exclusive port ownership; never use SO_REUSEPORT against the production host.
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 50002)).await?;
    let mut tree = Vfs::new();
    tree.mount(ServedSlot::USB.vfs_prefix(), root)?;
    let nfs = NfsServer::start(
        Arc::new(RwLock::new(tree)),
        NfsConfig {
            interface: Some(interface.clone()),
            ..Default::default()
        },
    )
    .await?;
    let discovery = Discovery::start(interface.clone()).await?;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let name = DeviceName::new("OLC SETTINGS TEST");
    let media = Arc::new(MediaSet::new([source.clone()]));
    // Claim a free 1–4 slot using the existing conflict-aware claim chain.
    // Custom status receiver below records source addresses and every settings exchange.
    let cdj = VirtualCdj::start(
        &discovery,
        VirtualCdjConfig {
            name,
            numbering: Numbering::Claim { preferred: None },
            emit_status: false,
            ..Default::default()
        },
        media,
    )
    .await?;
    let db = DbServer::start(
        DbServerConfig {
            device: cdj.browsable_number().ok_or("No free player number")?,
            address: interface.ip,
            ..Default::default()
        },
        [source.clone()],
    )
    .await?;
    let (tx, mut commands) = mpsc::channel::<String>(16);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.blocking_send(line).is_err() {
                return;
            }
        }
    });
    event(
        json!({"event":"ready","workerVersion":2,"sourceNumber":cdj.number().get(),"sourceIp":interface.ip,"sourceName":"OLC SETTINGS TEST",
        "originalBrightness":original[1]-0x80,"checksumMatches":file.checksum_matches(),"tracks":source.library().tracks.len(),"durationSeconds":900}),
    );
    let mut peers: HashMap<Ipv4Addr, Peer> = HashMap::new();
    let mut replies: HashMap<Ipv4Addr, (Instant, Vec<u8>)> = HashMap::new();
    let mut baselines: HashMap<Ipv4Addr, prolink_proto::MacAddress> = HashMap::new();
    let mut arm: Option<Arm> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    let deadline = Instant::now() + Duration::from_secs(900);
    let mut last_list = Instant::now() - Duration::from_secs(10);
    let mut buffer = [0; 4096];
    let mut counter = 0u32;
    loop {
        tokio::select! {
            line = commands.recv() => {
                let Some(line) = line else { break };
                let result: Result<(), String> = async {
                    let command: Value = serde_json::from_str(&line).map_err(|e|e.to_string())?;
                    let action = command["action"].as_str().unwrap_or("");
                    if action == "stop" { return Ok(()); }
                    if action == "disarm" { arm = None; event(json!({"event":"disarmed"})); return Ok(()); }
                    if action == "profile" {
                        if arm.is_some() { return Err("Disarm reconnect before changing brightness".into()); }
                        let b = command["brightness"].as_u64().and_then(|b|u8::try_from(b).ok()).ok_or("Invalid brightness")?;
                        selected = profile(&original,b)?;
                        event(json!({"event":"profile","brightness":selected[1]-0x80}));
                        return Ok(());
                    }
                    let ip: Ipv4Addr = command["ip"].as_str().ok_or("Select a player")?.parse().map_err(|_|"Invalid IPv4")?;
                    let device = discovery.online().into_iter().find(|d|d.ip==ip && d.name.as_str()=="CDJ-2000nexus" && d.number.get()<=4).ok_or("No discovered CDJ-2000nexus at that address")?;
                    let peer = peers.get(&ip).filter(|p|eligible(p) && p.number==device.number).ok_or("Player must have fresh stopped status")?;
                    match action {
                        "baseline" => {
                            let (at, bytes) = replies.get(&ip).filter(|(at,_)|at.elapsed()<Duration::from_secs(90)).ok_or("First load My Settings from OLC SETTINGS TEST on this CDJ")?;
                            let _ = at;
                            if bytes != &selected { return Err("Profile changed since manual load; repeat baseline".into()); }
                            baselines.insert(ip,device.mac);
                            event(json!({"event":"baseline-confirmed","ip":ip,"brightness":selected[1]-0x80,"evidence":"operator-observed"}));
                        }
                        "push" | "arm" => {
                            let method=PushMethod::parse(&command["method"])?;
                            if baselines.get(&ip)!=Some(&device.mac) { return Err("Confirm a working manual baseline for this player first".into()); }
                            if replies.get(&ip).is_some_and(|(at,_)|at.elapsed()<Duration::from_secs(10)) {
                                return Err("Wait 10 seconds after manual Load Settings so this attempt is clearly unsolicited".into());
                            }
                            if action=="arm" {
                                arm=Some(Arm {method,ip,mac:device.mac,number:peer.number,deadline:Instant::now()+Duration::from_secs(180),absent:false,returned:None});
                                event(json!({"event":"armed","method":method.id(),"commandKind":method.kind(),"ip":ip,"brightness":selected[1]-0x80,"message":"Disconnect player network for at least 12 seconds, then reconnect. One send only; expires in 3 minutes."}));
                            } else {
                                if arm.is_some() { return Err("Disarm reconnect before manual push".into()); }
                                let response=method.encode(name,cdj.number(),peer.number,&selected)?;
                                socket.send_to(&response,(ip,50002)).await.map_err(|e|e.to_string())?;
                                packet("tx",ip,&response,method.id());
                                event(json!({"event":"push-sent","method":method.id(),"commandKind":method.kind(),"bytes":response.len(),"ip":ip,"brightness":selected[1]-0x80,"applied":null,"evidence":"send-only"}));
                            }
                        }
                        _ => return Err("Unknown action".into()),
                    }
                    Ok(())
                }.await;
                if let Err(message)=result { event(json!({"event":"error","message":message})); }
                if serde_json::from_str::<Value>(&line).ok().is_some_and(|v|v["action"]=="stop") { break; }
            }
            _ = tick.tick() => {
                if Instant::now()>=deadline { event(json!({"event":"timeout"})); break; }
                // Fail closed if a peer takes our player number after startup.
                if discovery.online().iter().any(|d|d.number==cdj.number()) { return Err("Player number conflict; stopping test".into()); }
                counter=counter.wrapping_add(1);
                for device in discovery.online() {
                    if device.ip==interface.ip { continue; }
                    let response=source_status(name,cdj.number(),counter);
                    socket.send_to(response.as_bytes(),(device.ip,50002)).await?;
                }
                if let Some(a)=arm.as_mut() {
                    if Instant::now()>=a.deadline { event(json!({"event":"arm-expired"})); arm=None; }
                    else {
                        let live=discovery.online().into_iter().find(|d|d.ip==a.ip && d.mac==a.mac && d.number==a.number);
                        let fresh=peers.get(&a.ip).is_some_and(|p|p.seen.elapsed()<Duration::from_secs(2));
                        if live.is_none() && peers.get(&a.ip).is_none_or(|p|p.seen.elapsed()>Duration::from_secs(10)) { a.absent=true; }
                        if a.absent && live.is_some() && fresh {
                            let returned=a.returned.get_or_insert(Instant::now());
                            if returned.elapsed()>Duration::from_secs(2) {
                                if peers.get(&a.ip).is_some_and(|p|eligible(p) && p.number==a.number) {
                                    let response=a.method.encode(name,cdj.number(),a.number,&selected)?;
                                    socket.send_to(&response,(a.ip,50002)).await?;
                                    packet("tx",a.ip,&response,"reconnect-push");
                                    event(json!({"event":"reconnect-push-sent","method":a.method.id(),"commandKind":a.method.kind(),"bytes":response.len(),"ip":a.ip,"brightness":selected[1]-0x80,"applied":null,"evidence":"send-only"}));
                                } else { event(json!({"event":"reconnect-skipped","message":"Player is playing or has stale status"})); }
                                arm=None;
                            }
                        } else { a.returned=None; }
                    }
                }
                if last_list.elapsed()>Duration::from_secs(1) {
                    event(json!({"event":"players","players":discovery.online().into_iter().filter(|d|d.name.as_str()=="CDJ-2000nexus").map(|d|json!({"ip":d.ip,"number":d.number.get(),"name":d.name.as_str(),"stopped":peers.get(&d.ip).is_some_and(eligible),"baseline":baselines.get(&d.ip)==Some(&d.mac)})).collect::<Vec<_>>() }));
                    last_list=Instant::now();
                }
            }
            received = socket.recv_from(&mut buffer) => {
                let (len,from)=received?;
                let std::net::IpAddr::V4(ip)=from.ip() else {continue};
                let bytes=&buffer[..len];
                packet("rx",ip,bytes,"");
                let device=discovery.online().into_iter().find(|d|d.ip==ip && d.name.as_str()=="CDJ-2000nexus" && d.number.get()<=4);
                let Some(device)=device else {continue};
                match status::decode(bytes) {
                    Ok(status::Packet::CdjStatus(s)) if s.sender()==Some(device.number) => {
                        // Unknown flag/play states do not authorize transmission.
                        let stopped=s.flags().is_some_and(|f|!f.is_playing()) && matches!(s.play_state(),Some(0|4|5|6));
                        peers.insert(ip,Peer {number:device.number,seen:Instant::now(),stopped});
                    }
                    Ok(status::Packet::MediaQuery(q)) if q.target==cdj.number() && q.slot==Slot::USB && q.requester==device.number => {
                        let reply=status::MediaResponse::builder().name(name).device_number(cdj.number()).slot(Slot::USB)
                            .volume_name("OLC SETTINGS TEST").counts(source.library().tracks.len() as u32,source.library().playlists.len() as u32).has_settings(true).build();
                        socket.send_to(reply.as_bytes(),(ip,50002)).await?;
                        packet("tx",ip,reply.as_bytes(),"media-response");
                    }
                    Ok(status::Packet::SettingsQuery(q)) if q.requester==device.number && q.sender==device.number && q.slot==Slot::USB => {
                        if !peers.get(&ip).is_some_and(eligible) { event(json!({"event":"error","message":"Settings request ignored: fresh stopped status required"})); continue; }
                        let response=status::SettingsResponse::build(name,cdj.number(),q.requester,q.slot,&selected);
                        socket.send_to(response.as_bytes(),(ip,50002)).await?;
                        packet("tx",ip,response.as_bytes(),"requested-settings-response");
                        replies.insert(ip,(Instant::now(),selected.clone()));
                        event(json!({"event":"manual-response-sent","ip":ip,"brightness":selected[1]-0x80,"applied":null}));
                    }
                    _ => (),
                }
            }
        }
    }
    arm.take();
    drop(db);
    drop(nfs);
    drop(cdj);
    drop(discovery);
    event(json!({"event":"stopped"}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_method_selects_distinct_wire_formats() {
        let name = DeviceName::new("OLC SETTINGS TEST");
        let sender = DeviceNumber::new(4).unwrap();
        let target = DeviceNumber::ONE;
        let settings = vec![0x85; 32];
        let command = PushMethod::LoadCommand
            .encode(name, sender, target, &settings)
            .unwrap();
        let response = PushMethod::LegacyResponse
            .encode(name, sender, target, &settings)
            .unwrap();
        assert_eq!(
            (command.len(), command[10], command[0x1f], command[0x2d]),
            (116, 0x34, 2, 0x85)
        );
        assert_eq!(
            (response.len(), response[10], response[0x1f], response[0x31]),
            (80, 0x36, 1, 0x85)
        );
        assert!(PushMethod::parse(&Value::Null).is_err());
        assert_eq!(
            PushMethod::parse(&json!("load-command")).unwrap(),
            PushMethod::LoadCommand
        );
    }
    #[test]
    fn source_advertises_media_without_playback_and_advances_counter() {
        let s = source_status(DeviceName::new("OLC SETTINGS TEST"), DeviceNumber::ONE, 42);
        assert_eq!(s.packet_counter(), Some(42));
        assert_eq!(s.usb_state(), status::MediaState::LOADED);
        assert_eq!(s.sd_state(), status::MediaState::EMPTY);
        assert!(!s.flags().unwrap().is_playing());
        assert_eq!(s.track_id(), 0);
    }
    #[test]
    fn profile_changes_only_lcd_and_preserves_original() {
        let original = vec![0x83; 32];
        for b in 1..=5 {
            let p = profile(&original, b).unwrap();
            assert_eq!(p[1], 0x80 + b);
            assert!(
                p.iter()
                    .zip(&original)
                    .enumerate()
                    .all(|(i, (a, b))| i == 1 || a == b)
            );
        }
        assert_eq!(profile(&original, 0).unwrap(), original);
        assert!(profile(&original, 6).is_err());
        assert!(profile(&[], 1).is_err());
    }
    #[test]
    fn push_requires_fresh_stopped_telemetry() {
        let mut p = Peer {
            number: DeviceNumber::ONE,
            seen: Instant::now(),
            stopped: true,
        };
        assert!(eligible(&p));
        p.stopped = false;
        assert!(!eligible(&p));
        p.stopped = true;
        p.seen = Instant::now() - Duration::from_secs(3);
        assert!(!eligible(&p));
    }
}
