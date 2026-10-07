//! Explicit single-WAV transport experiment. No OneLibrary parsing, transcoding,
//! load commands or playback commands. Shared by desktop and embedded hosts.
use prolink::serve::{
    Medium, ServedSlot,
    dbserver::{DbServer, DbServerConfig},
    nfs::{NfsConfig, NfsServer},
    vfs::Vfs,
};
use prolink_proto::{BrowsableDeviceNumber, DeviceName, DeviceNumber, Slot, djl, status};
use prolink_rekordbox::{Container, Library, Track};
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};
use tokio::{net::UdpSocket, sync::watch, time::Instant};

struct Control {
    file: Option<PathBuf>,
    stop: Option<watch::Sender<bool>>,
    report: Value,
}
static CONTROL: Mutex<Control> = Mutex::new(Control {
    file: None,
    stop: None,
    report: Value::Null,
});
fn update(key: &str, value: Value) {
    if let Ok(mut c) = CONTROL.lock() {
        c.report[key] = value;
    }
}
fn step(message: impl Into<String>) {
    if let Ok(mut c) = CONTROL.lock()
        && let Some(steps) = c.report["steps"].as_array_mut()
    {
        steps.push(json!(message.into()));
    }
}
pub fn report() -> Value {
    CONTROL
        .lock()
        .map(|c| {
            if c.report.is_null() {
                json!({"phase":"Choose a WAV file", "active":false})
            } else {
                c.report.clone()
            }
        })
        .unwrap_or_default()
}
pub fn active() -> bool {
    CONTROL.lock().map(|c| c.stop.is_some()).unwrap_or(true)
}
pub fn select_file(path: PathBuf) -> Result<(), String> {
    let track = wav_track(&path)?;
    let mut c = CONTROL.lock().map_err(|e| e.to_string())?;
    if c.stop.is_some() {
        return Err("Stop the current USB test before choosing another file".into());
    }
    c.report = json!({"phase":"WAV ready", "active":false, "file":track.title,"sampleRate":track.sample_rate,"sampleDepth":track.sample_depth,"duration":track.duration,"steps":[]});
    c.file = Some(path);
    Ok(())
}
pub fn stop() {
    if let Ok(c) = CONTROL.lock()
        && let Some(stop) = &c.stop
    {
        let _ = stop.send(true);
    }
}
// Called only after the embedded runtime is fully stopped, before native access is released.
pub fn reset() {
    if let Ok(mut c) = CONTROL.lock() {
        c.stop = None;
        c.file = None;
        c.report = Value::Null;
    }
}
pub fn start(ip: Ipv4Addr, number: u8, isolated: bool) -> Result<(), String> {
    if !crate::EXPERIMENTS {
        return Err("Experiments are unavailable in this build".into());
    }
    if !isolated {
        return Err("Confirm this is an isolated test with one physical CDJ and the selected source number is unused".into());
    }
    let device = BrowsableDeviceNumber::new(number).ok_or("Source number must be 1–4")?;
    let mut c = CONTROL.lock().map_err(|e| e.to_string())?;
    if c.stop.is_some() {
        return Err("USB test is already running".into());
    }
    let path = c.file.clone().ok_or("Choose a WAV file on the USB first")?;
    let (tx, rx) = watch::channel(false);
    c.stop = Some(tx);
    c.report["active"] = json!(true);
    c.report["phase"] = json!("Starting");
    c.report["steps"] = json!([]);
    c.report["error"] = Value::Null;
    // A restarted test must not display evidence from the previous session.
    for key in ["player", "lastStatusAgeMs"] {
        c.report[key] = Value::Null;
    }
    c.report["statusPackets"] = json!(0);
    c.report["mediaQueries"] = json!(0);
    c.report["mounts"] = json!([]);
    tokio::spawn(async move {
        let result = serve(path, ip, device, rx).await;
        if let Ok(mut c) = CONTROL.lock() {
            c.stop = None;
            c.report["active"] = json!(false);
            c.report["phase"] = json!(if result.is_ok() { "Stopped" } else { "Failed" });
            if let Err(error) = result {
                c.report["error"] = json!(error);
            }
        }
    });
    Ok(())
}

/// Read RIFF chunks without loading audio into memory. Restrict the first proof
/// to stereo integer PCM known to work on the 2000nexus.
fn wav_track(path: &Path) -> Result<Track, String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Cannot read selected file: {e}"))?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length > u32::MAX as u64 {
        return Err("The test file must be smaller than 4 GiB".into());
    }
    let mut header = [0; 12];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return Err("Choose a standard PCM WAV file; no audio conversion is performed".into());
    }
    let mut format = None;
    let mut data = None;
    let mut offset = 12u64;
    for _ in 0..4096 {
        if offset + 8 > length {
            break;
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut chunk = [0; 8];
        file.read_exact(&mut chunk).map_err(|e| e.to_string())?;
        let size = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as u64;
        if offset + 8 + size > length {
            return Err("WAV contains a truncated chunk".into());
        }
        if &chunk[..4] == b"fmt " && size >= 16 {
            let mut fmt = [0; 16];
            file.read_exact(&mut fmt).map_err(|e| e.to_string())?;
            let u16at = |i| u16::from_le_bytes([fmt[i], fmt[i + 1]]);
            let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
            let depth = u16at(14);
            if u16at(0) != 1
                || u16at(2) != 2
                || ![44100, 48000].contains(&rate)
                || ![16, 24].contains(&depth)
            {
                return Err("For this test choose stereo PCM WAV, 44.1/48 kHz, 16/24-bit. Float/extensible WAV is outside this first test".into());
            }
            let bytes_per_second = rate * 2 * u32::from(depth) / 8;
            if u16at(12) != 2 * depth / 8
                || u32::from_le_bytes(fmt[8..12].try_into().unwrap()) != bytes_per_second
            {
                return Err("WAV format header is inconsistent".into());
            }
            format = Some((rate, depth, bytes_per_second));
        } else if &chunk[..4] == b"data" {
            data = Some(size);
        }
        if format.is_some() && data.is_some() {
            break;
        }
        offset += 8 + size + size % 2;
    }
    let (rate, depth, bytes_per_second) = format.ok_or("Missing supported WAV format chunk")?;
    let bytes = data.filter(|n| *n > 0).ok_or("Missing WAV audio data")?;
    let seconds = bytes / u64::from(bytes_per_second);
    if seconds == 0 || seconds > u16::MAX as u64 {
        return Err("Choose a WAV between one second and 18 hours".into());
    }
    Ok(Track {
        id: 1,
        title: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        file_path: "/probe.wav".into(),
        filename: "probe.wav".into(),
        duration: seconds as u16,
        sample_rate: rate,
        sample_depth: depth,
        bitrate: bytes_per_second * 8 / 1000,
        file_size: length as u32,
        container: Container::WAV,
        ..Track::default()
    })
}

async fn serve(
    path: PathBuf,
    peer: Ipv4Addr,
    browsable: BrowsableDeviceNumber,
    mut stop: watch::Receiver<bool>,
) -> Result<(), String> {
    let track = wav_track(&path)?;
    let route = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    route
        .connect((peer, 50000))
        .await
        .map_err(|e| e.to_string())?;
    let std::net::IpAddr::V4(local) = route.local_addr().map_err(|e| e.to_string())?.ip() else {
        return Err("IPv4 route required".into());
    };
    drop(route);
    if local == peer {
        return Err("Enter the CDJ address, not this device's address".into());
    }
    step(format!(
        "Source address: {local}; target: {peer}; virtual player: {}",
        browsable.get()
    ));
    let mut vfs = Vfs::new();
    vfs.add_disk_file("/C/probe.wav", &path)
        .map_err(|e| e.to_string())?;
    let nfs = NfsServer::start(Arc::new(RwLock::new(vfs)), NfsConfig::default()).await
        .map_err(|e| format!("NFS startup failed: {e}. This is a transport feasibility failure; no player announcement was sent."))?;
    step(format!("NFS ports opened: {:?}", nfs.ports()));
    let discovery = UdpSocket::bind((local, 50000))
        .await
        .map_err(|e| format!("Discovery port unavailable; disconnect live mode: {e}"))?;
    let status_socket = UdpSocket::bind((local, 50002))
        .await
        .map_err(|e| format!("Status port unavailable; disconnect live mode: {e}"))?;
    let number = DeviceNumber::new(browsable.get()).ok_or("Invalid source number")?;
    let name = DeviceName::new("CDJ-2000nexus");
    let library = Library {
        tracks: [(1, track)].into(),
        ..Library::default()
    };
    let medium = Arc::new(Medium::synthetic(ServedSlot::USB, library, "OLC USB TEST"));
    let db = DbServer::start(
        DbServerConfig {
            device: browsable,
            address: local,
            ..DbServerConfig::default()
        },
        [medium],
    )
    .await
    .map_err(|e| format!("Metadata server failed: {e}"))?;
    step(format!(
        "Metadata server opened on {}. Only the selected WAV is exposed; audio remains on USB.",
        db.port()
    ));
    // Reuse the proven direct-IP registration packet shapes, with an explicit
    // browsable identity. This is deliberately confined to an isolated rig.
    let mut stages = super::direct_status::registration(local);
    stages.push(super::direct_status::announcement(local));
    let stages: Vec<Vec<u8>> = stages
        .into_iter()
        .map(|bytes| {
            let mut packet = djl::Packet::decode(&bytes).expect("locally encoded registration");
            packet.name = name;
            match &mut packet.body {
                djl::Body::ClaimIp { device_number, .. }
                | djl::Body::ClaimNumber { device_number, .. }
                | djl::Body::KeepAlive { device_number, .. } => *device_number = number.get(),
                _ => (),
            }
            packet.encode()
        })
        .collect();
    let mut sent = 0;
    let mut next_registration = Instant::now();
    let mut status_tick = tokio::time::interval(Duration::from_millis(200));
    let mut counter = 0u32;
    let mut queries = 0u32;
    let mut statuses = 0u32;
    let mut last_peer = None;
    let mut buffer = [0u8; 2048];
    let mut discovery_buffer = [0u8; 2048];
    update(
        "phase",
        json!("Serving — on the CDJ select LINK → OLC USB TEST → TRACK"),
    );
    step(
        "Direct-IP registration uses an experimental synthetic MAC. No load/play commands are sent.",
    );
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tokio::time::sleep_until(next_registration) => {
                if *stop.borrow() { break; }
                let payload = &stages[sent.min(stages.len()-1)];
                discovery.send_to(payload,(peer,50000)).await.map_err(|e|format!("Announcement failed: {e}"))?;
                sent = sent.saturating_add(1);
                next_registration = Instant::now() + if sent < stages.len() {Duration::from_millis(300)} else {Duration::from_millis(1500)};
            }
            _ = status_tick.tick() => {
                if sent < stages.len()-1 { continue; }
                counter = counter.wrapping_add(1);
                let packet = source_status(name,number,counter,status::MediaState::LOADED);
                status_socket.send_to(packet.as_bytes(),(peer,50002)).await.map_err(|e|e.to_string())?;
                update("statusPackets",json!(statuses));
                update("mediaQueries",json!(queries));
                update("mounts",json!(nfs.mounts().iter().map(|m|format!("{} {}",m.peer,m.export)).collect::<Vec<_>>()));
                update("lastStatusAgeMs",last_peer.map(|t:Instant|json!(t.elapsed().as_millis())).unwrap_or(Value::Null));
            }
            result = discovery.recv_from(&mut discovery_buffer) => {
                let (len,from) = result.map_err(|e|e.to_string())?;
                if from.ip() != peer { continue; }
                if let Ok(packet) = djl::Packet::decode(&discovery_buffer[..len])
                    && packet.body.device_number() == Some(number.get()) {
                    return Err("Player-number conflict; server stopped".into());
                }
            }
            result = status_socket.recv_from(&mut buffer) => {
                let (len,from) = result.map_err(|e|e.to_string())?;
                if from.ip() != peer { continue; }
                match status::decode(&buffer[..len]) {
                    Ok(status::Packet::CdjStatus(packet)) => {
                        if packet.sender() == Some(number) { return Err("Player-number conflict; server stopped".into()); }
                        statuses += 1; last_peer = Some(Instant::now());
                        update("player", json!({"number":packet.sender().map(|n|n.get()),"source":packet.source_player().map(|n|n.get()),"track":packet.track_id(),"playState":packet.play_state(),"selectedFromTest":packet.source_player()==Some(number) && packet.source_slot()==Slot::USB && packet.track_id()==1}));
                    }
                    Ok(status::Packet::MediaQuery(query)) if query.target == number && query.slot == Slot::USB => {
                        queries += 1;
                        let response = status::MediaResponse::builder().name(name).device_number(number).slot(Slot::USB).volume_name("OLC USB TEST").counts(1,0).build();
                        status_socket.send_to(response.as_bytes(),(peer,50002)).await.map_err(|e|e.to_string())?;
                    }
                    Ok(status::Packet::SettingsQuery(query)) => {
                        let response = status::SettingsResponse::build(name,number,query.requester,query.slot,&[]);
                        status_socket.send_to(response.as_bytes(),(peer,50002)).await.map_err(|e|e.to_string())?;
                    }
                    _ => (),
                }
            }
        }
    }
    // Graceful disappearance; the user stops playback before pressing Stop.
    let packet = source_status(
        name,
        number,
        counter.wrapping_add(1),
        status::MediaState::EMPTY,
    );
    let _ = status_socket
        .send_to(packet.as_bytes(), (peer, 50002))
        .await;
    step("Stopped serving the selected WAV");
    Ok(())
}
pub(crate) fn source_status(
    name: DeviceName,
    number: DeviceNumber,
    counter: u32,
    media: status::MediaState,
) -> status::CdjStatus {
    status::CdjStatus::builder()
        .name(name)
        .device_number(number)
        .slot_state(Slot::USB, media)
        .slot_state(Slot::SD, status::MediaState::EMPTY)
        .link_available(true)
        .packet_counter(counter)
        .playing(false)
        .tempo_master(false)
        .synced(false)
        .play_state(0)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "pc-usb-wav-{}-{}.wav",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&path, bytes).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn wav(rate: u32, depth: u16, encoding: u16, channels: u16) -> Vec<u8> {
        let block = channels * depth / 8;
        let byte_rate = rate * u32::from(block);
        let mut bytes = b"RIFF".to_vec();
        bytes.extend((36 + byte_rate).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(encoding.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend(rate.to_le_bytes());
        bytes.extend(byte_rate.to_le_bytes());
        bytes.extend(block.to_le_bytes());
        bytes.extend(depth.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(byte_rate.to_le_bytes());
        bytes.resize(44 + byte_rate as usize, 0x35);
        bytes
    }
    #[test]
    fn validates_supported_pcm_without_changing_audio() {
        for (rate, depth) in [(44100, 16), (44100, 24), (48000, 16), (48000, 24)] {
            let bytes = wav(rate, depth, 1, 2);
            let fixture = Fixture::new(&bytes);
            let track = wav_track(&fixture.0).unwrap();
            assert_eq!(
                (track.sample_rate, track.sample_depth, track.duration),
                (rate, depth, 1)
            );
            assert_eq!(track.file_size as usize, bytes.len());
            assert_eq!(track.container, Container::WAV);
            assert_eq!(std::fs::read(&fixture.0).unwrap(), bytes);
        }
    }
    #[test]
    fn rejects_float_high_resolution_mono_and_truncated_audio() {
        for bytes in [
            wav(44100, 32, 3, 2),
            wav(96000, 24, 1, 2),
            wav(44100, 16, 1, 1),
            b"not a WAV file".to_vec(),
        ] {
            assert!(wav_track(&Fixture::new(&bytes).0).is_err());
        }
        let mut bytes = wav(44100, 16, 1, 2);
        bytes.pop();
        assert!(
            wav_track(&Fixture::new(&bytes).0)
                .unwrap_err()
                .contains("truncated")
        );
    }
    #[test]
    fn handles_padded_metadata_chunks_and_rejects_inconsistent_headers() {
        let mut bytes = wav(44100, 16, 1, 2);
        let mut junk = b"JUNK".to_vec();
        junk.extend(3u32.to_le_bytes());
        junk.extend([1, 2, 3, 0]);
        bytes.splice(12..12, junk);
        let riff_length = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_length.to_le_bytes());
        assert_eq!(wav_track(&Fixture::new(&bytes).0).unwrap().duration, 1);
        let mut bytes = wav(44100, 16, 1, 2);
        bytes[28..32].copy_from_slice(&1u32.to_le_bytes());
        assert!(
            wav_track(&Fixture::new(&bytes).0)
                .unwrap_err()
                .contains("inconsistent")
        );
    }
    #[test]
    fn exposes_only_selected_file_and_serves_original_byte_ranges() {
        let bytes = wav(44100, 16, 1, 2);
        let fixture = Fixture::new(&bytes);
        let mut vfs = Vfs::new();
        vfs.add_disk_file("/C/probe.wav", &fixture.0).unwrap();
        let handle = Vfs::handle_for("/C/probe.wav");
        assert_eq!(vfs.read(handle, 0, 44).unwrap(), bytes[..44]);
        assert_eq!(vfs.read(handle, 8192, 1024).unwrap(), bytes[8192..9216]);
        assert!(vfs.resolve(Vfs::handle_for("/C/another.wav")).is_none());
        assert!(
            vfs.read(handle, bytes.len() as u64, 1024)
                .unwrap()
                .is_empty()
        );
        std::fs::remove_file(&fixture.0).unwrap();
        assert!(vfs.read(handle, 0, 1024).unwrap().is_empty());
    }
    #[test]
    fn source_advertises_media_without_playback_or_mastership() {
        let number = DeviceNumber::new(4).unwrap();
        let packet = source_status(
            DeviceName::new("CDJ-2000nexus"),
            number,
            42,
            status::MediaState::LOADED,
        );
        assert_eq!(packet.sender(), Some(number));
        assert_eq!(packet.usb_state(), status::MediaState::LOADED);
        assert_eq!(packet.sd_state(), status::MediaState::EMPTY);
        assert_eq!(packet.track_id(), 0);
        assert_eq!(packet.source_player(), None);
        assert_eq!(packet.play_state(), Some(0));
    }
}
