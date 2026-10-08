//! Explicit read-only CDJ cue capture over unicast, shared by all hosts.
use prolink::consume::{dbclient::DbClient, nfs::NfsClient};
use prolink_proto::{
    BrowsableDeviceNumber, Slot,
    dbserver::{Arguments, Descriptor, Field, MenuTarget, Message, MessageKind, TrackType},
    djl, status,
};
use serde_json::{Value, json};
use std::{net::Ipv4Addr, sync::Mutex, time::Duration};
use tokio::{net::UdpSocket, sync::watch, time::Instant};

struct State {
    stop: Option<watch::Sender<bool>>,
    report: Value,
}
static STATE: Mutex<State> = Mutex::new(State {
    stop: None,
    report: Value::Null,
});
const REQUESTER: u8 = crate::local_serving::NUMBER;
fn update(key: &str, value: Value) {
    STATE.lock().unwrap().report[key] = value;
}
pub fn active() -> bool {
    STATE.lock().unwrap().stop.is_some()
}
pub fn report() -> Value {
    let state = STATE.lock().unwrap();
    let mut value = state.report.clone();
    if value.is_null() {
        value = json!({"phase":"Ready"});
    }
    value["active"] = json!(state.stop.is_some());
    value
}
pub fn stop() {
    if let Some(tx) = &STATE.lock().unwrap().stop {
        let _ = tx.send(true);
    }
}
// The embedded runtime has already stopped before this is called.
pub fn reset() {
    let mut state = STATE.lock().unwrap();
    state.stop = None;
    if !state.report.is_null()
        && state
            .report
            .get("phase")
            .is_some_and(|v| v != "Complete" && v != "Incomplete")
    {
        state.report["phase"] = json!("Incomplete");
        state.report["error"] = json!("App stopped during capture; partial results retained");
    }
}
pub fn start(ip: Ipv4Addr, number: u8) -> Result<(), String> {
    if !(1..=2).contains(&number) {
        return Err("Choose CDJ 1 or CDJ 2".into());
    }
    let mut state = STATE.lock().unwrap();
    if state.stop.is_some() {
        return Err("A cue capture is already running".into());
    }
    let (tx, mut rx) = watch::channel(false);
    state.stop = Some(tx);
    state.report = json!({"schema":"olc-native-cues-v1","build":52,"phase":"Registering for metadata queries",
        "sourceIp":ip.to_string(),"sourcePlayer":number,"requester":REQUESTER,
        "readOnly":true,"startedUnixMs":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()});
    tokio::spawn(async move {
        let result = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(90), capture(ip, number)) =>
                result.unwrap_or_else(|_| Err("Capture timed out after 90 seconds; partial results retained".into())),
            _ = rx.changed() => Err("Capture stopped; partial results retained".into()),
        };
        let mut state = STATE.lock().unwrap();
        state.stop = None;
        state.report["phase"] = json!(if result.is_ok() {
            "Complete"
        } else {
            "Incomplete"
        });
        if let Err(error) = result {
            state.report["error"] = json!(error);
        }
    });
    Ok(())
}
fn validate(s: &status::CdjStatus, number: u8) -> Result<u32, String> {
    if s.sender().map(|n| n.get()) != Some(number) {
        return Err("CDJ player number does not match the selected player".into());
    }
    if !s.usb_state().has_media()
        || s.source_slot() != Slot::USB
        || s.source_player().map(|n| n.get()) != Some(number)
        || s.track_type() != 1
        || s.track_id() == 0
    {
        return Err(
            "Load a rekordbox track directly from the USB plugged into this CDJ first".into(),
        );
    }
    Ok(s.track_id())
}
async fn capture(ip: Ipv4Addr, number: u8) -> Result<(), String> {
    let route = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    route
        .connect((ip, 50000))
        .await
        .map_err(|e| e.to_string())?;
    let std::net::IpAddr::V4(local) = route.local_addr().map_err(|e| e.to_string())?.ip() else {
        return Err("IPv4 route required".into());
    };
    if local == ip {
        return Err("Enter the CDJ address, not this device's address".into());
    }
    update("localIp", json!(local.to_string()));
    let discovery = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 50000))
        .await
        .map_err(|e| format!("Disconnect OLC live connections first: {e}"))?;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 50002))
        .await
        .map_err(|e| format!("Status port unavailable: {e}"))?;
    let stages = crate::local_serving::stages(local);
    let mut sent = 0usize;
    let mut next = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut discovery_bytes = [0; 4096];
    let mut status_bytes = [0; 4096];
    let mut observed: Option<(status::CdjStatus, Instant)> = None;
    let registered_at = Instant::now();
    let track = loop {
        tokio::select! {
            _ = tick.tick() => {
                if Instant::now() >= next {
                    discovery.send_to(&stages[sent.min(stages.len()-1)],(ip,50000)).await.map_err(|e|e.to_string())?;
                    sent += 1;
                    next = Instant::now()+Duration::from_millis(if sent < stages.len(){300}else{1500});
                }
                if sent >= stages.len() && let Some((s,at)) = &observed && at.elapsed()<Duration::from_secs(1) {
                    let id = validate(s,number)?;
                    update("status",json!({"trackId":id,"model":s.name().as_str(),"firmware":s.firmware(),"rawHex":hex(s.as_bytes())}));
                    break id;
                }
                if registered_at.elapsed()>Duration::from_secs(15) { return Err("No fresh CDJ status received after registration".into()); }
            }
            r = discovery.recv_from(&mut discovery_bytes) => {
                let (n,from)=r.map_err(|e|e.to_string())?;
                reject_collision(&discovery_bytes[..n],from.ip(),local)?;
            }
            r = socket.recv_from(&mut status_bytes) => {
                let (n,from)=r.map_err(|e|e.to_string())?;
                if from.ip()==ip && let Ok(s)=status::CdjStatus::parse(&status_bytes[..n]) { observed=Some((s,Instant::now())); }
            }
        }
    };
    let work = read_cues(ip, track);
    tokio::pin!(work);
    let mut keepalive = tokio::time::interval(Duration::from_millis(1500));
    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = keepalive.tick() => {
                if observed.as_ref().is_none_or(|(_,at)|at.elapsed()>Duration::from_secs(3)) { return Err("CDJ status lost during capture; partial results retained".into()); }
                discovery.send_to(stages.last().unwrap(),(ip,50000)).await.map_err(|e|e.to_string())?;
            }
            r = discovery.recv_from(&mut discovery_bytes) => {
                let (n,from)=r.map_err(|e|e.to_string())?;
                reject_collision(&discovery_bytes[..n],from.ip(),local)?;
            }
            r = socket.recv_from(&mut status_bytes) => {
                let (n,from)=r.map_err(|e|e.to_string())?;
                if from.ip()==ip && let Ok(s)=status::CdjStatus::parse(&status_bytes[..n]) {
                    if validate(&s,number)?!=track { return Err("Track changed during capture; partial results retained".into()); }
                    observed=Some((s,Instant::now()));
                }
            }
        }
    }
}
fn reject_collision(bytes: &[u8], from: std::net::IpAddr, local: Ipv4Addr) -> Result<(), String> {
    if from != local
        && djl::Packet::decode(bytes).is_ok_and(|p| p.body.device_number() == Some(REQUESTER))
    {
        return Err("Player 4 is in use; cue capture stopped".into());
    }
    Ok(())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn describe(reply: &Message) -> Value {
    let bytes = reply.blob(3).unwrap_or_default();
    let records:Vec<_>=bytes.as_chunks::<36>().0.iter().map(|r|json!({"hot":r[2],"flags":[r[0],r[1]],
        "frame":u32::from_le_bytes(r[12..16].try_into().unwrap()),"loopFrame":u32::from_le_bytes(r[16..20].try_into().unwrap()),"hex":hex(r)})).collect();
    json!({"kind":reply.kind.0,"transaction":reply.transaction_id,"hotCount":reply.number(5),"memoryCount":reply.number(6),
        "entrySize":reply.number(4),"recordLength":bytes.len(),"recordRemainder":bytes.len()%36,
        "recordsHex":hex(bytes),"records":records,"timesHex":hex(reply.blob(8).unwrap_or_default()),"reencodedMessageHex":hex(&reply.encode())})
}
async fn read_cues(ip: Ipv4Addr, track: u32) -> Result<(), String> {
    update("phase", json!("Reading native cue reply"));
    let device = BrowsableDeviceNumber::new(REQUESTER).unwrap();
    let mut client = DbClient::connect(ip, device)
        .await
        .map_err(|e| e.to_string())?;
    let reply = client
        .request(Message::new(
            0x03800100,
            MessageKind::GET_CUE_POINTS,
            Arguments::from([
                Field::from(Descriptor::new(
                    device,
                    Slot::USB,
                    MenuTarget::BINARY,
                    TrackType::REKORDBOX,
                )),
                Field::U32(track),
            ]),
        ))
        .await
        .map_err(|e| e.to_string())?;
    update("nativeReply", describe(&reply));
    if reply.kind != MessageKind::CUE_POINTS {
        return Err("CDJ did not return a legacy cue reply; response saved".into());
    }
    // Retain the native reply even if subsequent USB reads fail.
    client.close().await.map_err(|e| e.to_string())?;
    update("phase", json!("Reading USB cue definitions"));
    let mut nfs = NfsClient::connect(ip, None)
        .await
        .map_err(|e| e.to_string())?;
    let mount = nfs.mount_slot(Slot::USB).await.map_err(|e| e.to_string())?;
    let file = nfs
        .open(&mount, "/PIONEER/rekordbox/export.pdb")
        .await
        .map_err(|e| e.to_string())?;
    if file.size() > 32 * 1024 * 1024 {
        return Err("USB catalog exceeds capture limit; native reply saved".into());
    }
    let bytes = nfs.read_file(&file).await.map_err(|e| e.to_string())?;
    let catalog = prolink_rekordbox::Library::parse(&bytes).map_err(|e| e.to_string())?;
    let row = catalog
        .tracks
        .get(&track)
        .ok_or("Loaded track not found in USB export.pdb; native reply saved")?;
    update(
        "track",
        json!({"id":track,"title":row.title,"artist":row.artist,"audioPath":row.file_path,"analysisPath":row.analyze_path}),
    );
    let mut parsed = prolink::serve::Analysis::default();
    let mut files = Vec::new();
    for (index, path) in [Some(row.analyze_path.clone()), row.analyze_ext_path()]
        .into_iter()
        .enumerate()
    {
        let Some(path) = path else { continue };
        let result=async {
            let file=nfs.open(&mount,&path).await.map_err(|e|e.to_string())?;
            if file.size()>4*1024*1024 {return Err("Analysis file exceeds capture limit".to_owned());}
            let bytes=nfs.read_file(&file).await.map_err(|e|e.to_string())?;
            let anlz=prolink_rekordbox::AnlzFile::parse(&bytes).map_err(|e|e.to_string())?;
            let basic:Vec<_>=anlz.cue_lists().map(|l|json!({"kind":l.list_type.0,"cues":l.cues.iter().map(|c|json!({"hot":c.hot_cue,"type":c.cue_type.0,"timeMs":c.time,"loopMs":c.loop_time})).collect::<Vec<_>>()})).collect();
            let extended:Vec<_>=anlz.extended_cue_lists().map(|l|json!({"kind":l.list_type.0,"cues":l.cues.iter().map(|c|json!({"hot":c.hot_cue,"type":c.cue_type.0,"timeMs":c.time,"loopMs":c.loop_time})).collect::<Vec<_>>()})).collect();
            let tags=cue_tags(&bytes)?;
            Ok((anlz,json!({"path":path,"basic":basic,"extended":extended,"rawCueTags":tags})))
        }.await;
        match result {
            Ok((anlz, description)) => {
                if index == 0 {
                    parsed.dat = Some(anlz)
                } else {
                    parsed.ext = Some(anlz)
                };
                files.push(description);
            }
            Err(error) => files.push(json!({"path":path,"error":error})),
        }
        update("analysisFiles", json!(files));
    }
    if files.iter().any(|f| f.get("error").is_some()) {
        return Err(
            "Could not read all analysis files; native reply and partial USB data saved".into(),
        );
    }
    let olc = prolink::serve::dbserver::diagnostic_cue_reply(&parsed);
    update("olcReplyOffline", describe(&olc));
    update(
        "comparison",
        json!({"recordsMatch":reply.blob(3)==olc.blob(3),"timesMatch":reply.blob(8)==olc.blob(8),
        "hotCountMatch":reply.number(5)==olc.number(5),"memoryCountMatch":reply.number(6)==olc.number(6)}),
    );
    Ok(())
}
fn cue_tags(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let word = |at: usize| -> Result<usize, String> {
        let b = bytes.get(at..at + 4).ok_or("Truncated analysis header")?;
        Ok(u32::from_be_bytes(b.try_into().unwrap()) as usize)
    };
    let mut at = word(4)?;
    let mut tags = Vec::new();
    while at < bytes.len() {
        let size = word(at + 8)?;
        if size < 12 || size > bytes.len() - at {
            return Err("Invalid analysis tag length".into());
        }
        let tag = &bytes[at..at + size];
        if &tag[..4] == b"PCOB" || &tag[..4] == b"PCO2" {
            tags.push(json!({"tag":String::from_utf8_lossy(&tag[..4]),"hex":hex(tag)}));
        }
        at += size;
    }
    Ok(tags)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_report_keeps_overlapping_memory_and_hot_records_and_both_blobs() {
        let mut records = vec![0u8; 72];
        records[1] = 1;
        records[2] = 2;
        records[12..16].copy_from_slice(&13530u32.to_le_bytes());
        records[37] = 1;
        records[48..52].copy_from_slice(&13530u32.to_le_bytes());
        let times = vec![
            1, 2, 3, 4, 255, 255, 255, 255, 1, 2, 3, 4, 255, 255, 255, 255,
        ];
        let reply = Message::new(
            1,
            MessageKind::CUE_POINTS,
            Arguments::from([
                Field::U32(0x2104),
                Field::U32(0),
                Field::U32(72),
                Field::Blob(records.clone()),
                Field::U32(36),
                Field::U32(1),
                Field::U32(1),
                Field::U32(16),
                Field::Blob(times.clone()),
            ]),
        );
        let d = describe(&reply);
        assert_eq!(d["records"].as_array().unwrap().len(), 2);
        assert_eq!(d["records"][0]["hot"], 2);
        assert_eq!(d["records"][1]["hot"], 0);
        assert_eq!(d["recordsHex"], hex(&records));
        assert_eq!(d["timesHex"], hex(&times));
        assert_eq!(d["records"][0]["frame"], d["records"][1]["frame"]);
    }
    #[test]
    fn usb_track_precondition_and_raw_tag_bounds() {
        let packet = status::CdjStatus::builder().build();
        assert!(validate(&packet, 2).is_err());
        let player = prolink_proto::DeviceNumber::new(2).unwrap();
        let packet = status::CdjStatus::builder()
            .device_number(player)
            .slot_state(Slot::USB, status::MediaState::LOADED)
            .loaded_track(Some(status::LoadedTrack {
                source_player: player,
                slot: Slot::USB,
                id: 2040,
            }))
            .build();
        assert_eq!(validate(&packet, 2).unwrap(), 2040);
        assert!(validate(&packet, 1).is_err());
        let remote = status::CdjStatus::builder()
            .device_number(player)
            .slot_state(Slot::USB, status::MediaState::LOADED)
            .loaded_track(Some(status::LoadedTrack {
                source_player: prolink_proto::DeviceNumber::new(4).unwrap(),
                slot: Slot::USB,
                id: 2040,
            }))
            .build();
        assert!(validate(&remote, 2).is_err());
        assert!(cue_tags(&[]).is_err());
        let mut bytes = b"PMAI".to_vec();
        bytes.extend(12u32.to_be_bytes());
        bytes.extend(24u32.to_be_bytes());
        bytes.extend(b"PCOB");
        bytes.extend(12u32.to_be_bytes());
        bytes.extend(12u32.to_be_bytes());
        assert_eq!(cue_tags(&bytes).unwrap()[0]["hex"], hex(&bytes[12..]));
        bytes[20..24].copy_from_slice(&0u32.to_be_bytes());
        assert!(cue_tags(&bytes).is_err());
    }
}
