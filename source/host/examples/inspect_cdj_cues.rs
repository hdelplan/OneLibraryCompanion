//! Read-only native cue-reply audit. Never sends load, playback or USB writes.
//! Claims a free player number for database queries; stop the Mac host first.
use prolink::consume::{dbclient::DbClient, nfs::NfsClient};
use prolink::virtual_cdj::Numbering;
use prolink::{Discovery, Interface, Slot, VirtualCdj, VirtualCdjConfig};
use prolink_proto::dbserver::{
    Arguments, Descriptor, Field, MenuTarget, Message, MessageKind, TrackType,
};
use serde_json::json;
use std::{error::Error, time::Duration};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: inspect_cdj_cues INTERFACE SOURCE_PLAYER TITLE_SUBSTRING".into());
    }
    let source: u8 = args[1].parse()?;
    let interface = Interface::named(&args[0])?;
    // Avoid competing with another local host or monitor on the status port.
    let _exclusive = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 50002)).await?;
    let discovery = Discovery::start(interface.clone()).await?;
    tokio::time::sleep(Duration::from_secs(8)).await;
    let peer = discovery
        .devices()
        .into_iter()
        .find(|d| !d.offline && d.number.get() == source && d.name.as_str().starts_with("CDJ"))
        .ok_or("source CDJ not discovered")?;
    let virtual_cdj = VirtualCdj::observe(
        &discovery,
        VirtualCdjConfig {
            numbering: Numbering::Claim { preferred: None },
            emit_status: false,
            ..Default::default()
        },
    )
    .await?;
    let number = virtual_cdj
        .browsable_number()
        .ok_or("no free player number")?;
    eprintln!(
        "Read-only cue audit: source={source} {}, temporary requester={number}",
        peer.ip
    );
    let mut nfs = NfsClient::connect(peer.ip, Some(&interface)).await?;
    let mount = nfs.mount_slot(Slot::USB).await?;
    let file = nfs.open(&mount, "/PIONEER/rekordbox/export.pdb").await?;
    let catalog = prolink_rekordbox::Library::parse(&nfs.read_file(&file).await?)?;
    let mut tracks: Vec<_> = catalog
        .tracks
        .values()
        .filter(|t| t.title.to_lowercase().contains(&args[2].to_lowercase()))
        .collect();
    tracks.sort_by_key(|t| t.id);
    if tracks.is_empty() {
        return Err("no matching track in the source USB catalog".into());
    }
    let mut client = DbClient::connect(peer.ip, number).await?;
    for (index, track) in tracks.iter().enumerate() {
        let descriptor =
            Descriptor::new(number, Slot::USB, MenuTarget::BINARY, TrackType::REKORDBOX);
        let reply = client
            .request(Message::new(
                0x0380_0100 + u32::try_from(index)?,
                MessageKind::GET_CUE_POINTS,
                Arguments::from([Field::from(descriptor), Field::U32(track.id)]),
            ))
            .await?;
        let records: Vec<_> = reply
            .blob(3)
            .unwrap_or_default()
            .as_chunks::<36>()
            .0
            .iter()
            .map(|r| {
                json!({"hot":r[2],"flags":[r[0],r[1]],
                "frame":u32::from_le_bytes(r[12..16].try_into().unwrap()),
                "loopFrame":u32::from_le_bytes(r[16..20].try_into().unwrap()),"hex":hex(r)})
            })
            .collect();
        let mut files = Vec::new();
        for path in [Some(track.analyze_path.clone()), track.analyze_ext_path()]
            .into_iter()
            .flatten()
        {
            let file = nfs.open(&mount, &path).await?;
            let bytes = nfs.read_file(&file).await?;
            let anlz = prolink_rekordbox::AnlzFile::parse(&bytes)?;
            let basic: Vec<_> = anlz
                .cue_lists()
                .map(|list| {
                    json!({
                        "kind":list.list_type.0,"entries":list.cues.iter().map(|c| json!({
                            "hot":c.hot_cue,"type":c.cue_type.0,"timeMs":c.time,"loopMs":c.loop_time
                        })).collect::<Vec<_>>()
                    })
                })
                .collect();
            let extended: Vec<_> = anlz
                .extended_cue_lists()
                .map(|list| {
                    json!({
                        "kind":list.list_type.0,"entries":list.cues.iter().map(|c| json!({
                            "hot":c.hot_cue,"type":c.cue_type.0,"timeMs":c.time,"loopMs":c.loop_time
                        })).collect::<Vec<_>>()
                    })
                })
                .collect();
            files.push(json!({"path":path,"basic":basic,"extended":extended}));
        }
        println!(
            "{}",
            json!({"sourcePlayer":source,"sourceIp":peer.ip.to_string(),
            "requester":number.get(),"trackId":track.id,"title":track.title,
            "replyKind":reply.kind.0,"replyReencodedHex":hex(&reply.encode()),
            "hotCount":reply.number(5),"memoryCount":reply.number(6),
            "records":records,"timesHex":hex(reply.blob(8).unwrap_or_default()),"analysisFiles":files})
        );
    }
    client.close().await?;
    Ok(())
}
