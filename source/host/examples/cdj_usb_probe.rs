//! Explicit hardware experiment. Default is discovery/catalog only.
#[path = "../src/cdj_usb_load.rs"]
mod cdj_usb_load;
use cdj_usb_load::{LoadUsbTrack, TargetStatus};
use prolink::consume::nfs::NfsClient;
use prolink::{Discovery, Interface, Slot, VirtualCdj, VirtualCdjConfig};
use prolink_proto::status::CdjStatus;
use std::{
    collections::BTreeMap,
    error::Error,
    time::{Duration, Instant},
};
use tokio::net::UdpSocket;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

async fn status(socket: &UdpSocket, ip: std::net::Ipv4Addr, number: u8) -> Result<CdjStatus> {
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut buf = [0; 2048];
    // Discard queued telemetry: preflight must reflect a newly received packet.
    while socket.try_recv_from(&mut buf).is_ok() {}
    while Instant::now() < deadline {
        let (n, from) =
            tokio::time::timeout(Duration::from_secs(1), socket.recv_from(&mut buf)).await??;
        if from.ip() == ip
            && let Ok(s) = CdjStatus::parse(&buf[..n])
            && s.sender().map(|d| d.get()) == Some(number)
        {
            return Ok(s);
        }
    }
    Err("no fresh target status".into())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let search = args.len() == 3 && args[1] == "--find";
    if args.len() != 1 && args.len() != 5 && !search {
        return Err(
            "usage: cdj_usb_probe INTERFACE [--find TEXT | SOURCE TARGET TRACK_ID --send]".into(),
        );
    }
    let send = args.len() == 5;
    if send && args[4] != "--send" {
        return Err("explicit --send required".into());
    }
    let interface = Interface::named(&args[0])?;
    // Exclusive bind: fail if another host/monitor owns the status port.
    let socket = UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 50002)).await?;
    let discovery = Discovery::start(interface.clone()).await?;
    tokio::time::sleep(Duration::from_secs(8)).await;
    let devices: Vec<_> = discovery
        .devices()
        .into_iter()
        .filter(|d| !d.offline && d.number.get() <= 6 && d.name.as_str().starts_with("CDJ"))
        .collect();
    if devices.is_empty() {
        return Err("no CDJ discovered on this interface".into());
    }
    let observer = VirtualCdj::observe(
        &discovery,
        VirtualCdjConfig {
            emit_status: false,
            ..Default::default()
        },
    )
    .await?;
    let mut libraries = BTreeMap::new();
    for d in &devices {
        let s = status(&socket, d.ip, d.number.get()).await?;
        println!(
            "PLAYER {} {} {} firmware={:?} state={:?} loaded={} usb={:?}",
            d.number,
            d.name,
            d.ip,
            s.firmware(),
            s.play_state(),
            s.track_id(),
            s.usb_state()
        );
        if !s.usb_state().has_media() {
            continue;
        }
        let mut client = NfsClient::connect(d.ip, Some(&interface)).await?;
        let mount = client.mount_slot(Slot::USB).await?;
        let file = client.open(&mount, "/PIONEER/rekordbox/export.pdb").await?;
        let bytes = client.read_file(&file).await?;
        let library = prolink_rekordbox::Library::parse(&bytes)?;
        println!("USB catalog: {} tracks", library.tracks.len());
        let mut tracks: Vec<_> = library.tracks.values().collect();
        tracks.sort_by_key(|t| t.id);
        for t in tracks
            .into_iter()
            .filter(|t| {
                if search {
                    format!("{} {}", t.artist, t.title)
                        .to_lowercase()
                        .contains(&args[2].to_lowercase())
                } else {
                    t.id != s.track_id()
                }
            })
            .take(if search { 50 } else { 8 })
        {
            println!(
                "  {} | {} | {} | {:?} {}Hz",
                t.id, t.artist, t.title, t.container, t.sample_rate
            );
        }
        libraries.insert(d.number.get(), library);
    }
    if !send {
        println!("READ ONLY: no load command sent");
        return Ok(());
    }
    let source: u8 = args[1].parse()?;
    let target: u8 = args[2].parse()?;
    let track_id: u32 = args[3].parse()?;
    let src = devices
        .iter()
        .find(|d| d.number.get() == source)
        .ok_or("source not discovered")?;
    let dst = devices
        .iter()
        .find(|d| d.number.get() == target)
        .ok_or("target not discovered")?;
    let track = libraries
        .get(&source)
        .and_then(|l| l.tracks.get(&track_id))
        .ok_or("track absent from source USB database")?;
    let source_status = status(&socket, src.ip, source).await?;
    if !source_status.usb_state().has_media() {
        return Err("source USB no longer mounted".into());
    }
    let before = status(&socket, dst.ip, target).await?;
    TargetStatus {
        age_ms: 0,
        playing: before.as_bytes().get(0x89).is_none_or(|v| v & 0x40 != 0),
        // Spun down with no loaded track is an empty deck, not paused audio.
        play_state: match before.play_state() {
            Some(14) if before.track_id() == 0 => 0,
            Some(state) => state,
            None => return Err("missing play state".into()),
        },
    }
    .check()?;
    if before.track_id() == track_id
        && before.source_player().map(|d| d.get()) == Some(source)
        && before.source_slot() == Slot::USB
    {
        return Err(
            "choose a different track so completion can be distinguished from old state".into(),
        );
    }
    let packet = LoadUsbTrack {
        sender: observer.number().get(),
        source,
        target,
        track_id,
    }
    .encode()?;
    println!(
        "SEND ONCE: {} | {} | ID {} from PLAYER {} USB to PLAYER {}",
        track.artist, track.title, track_id, source, target
    );
    socket.send_to(&packet, (dst.ip, 50002)).await?;
    let mut buf = [0; 2048];
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        let Ok(result) =
            tokio::time::timeout(Duration::from_secs(1), socket.recv_from(&mut buf)).await
        else {
            continue;
        };
        let (n, from) = result?;
        if from.ip() != dst.ip {
            continue;
        }
        if n >= 11 && &buf[..10] == b"Qspt1WmJOL" && buf[10] == 0x1a {
            println!("ACK received (not completion)");
        }
        if let Ok(s) = CdjStatus::parse(&buf[..n])
            && s.sender().map(|d| d.get()) == Some(target)
            && s.track_id() == track_id
            && s.source_player().map(|d| d.get()) == Some(source)
            && s.source_slot() == Slot::USB
            && s.track_type() == 1
            && matches!(s.play_state(), Some(3 | 5 | 6))
        {
            println!(
                "MATCHING target status (physical playback verification required): source={} USB id={} state={:?}; verify title on physical CDJ",
                source,
                track_id,
                s.play_state()
            );
            return Ok(());
        }
    }
    Err("load outcome unknown after 12 seconds; no retry sent".into())
}
