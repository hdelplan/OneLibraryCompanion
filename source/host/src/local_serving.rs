//! Original-file serving for explicit local-library load requests. The existing
//! live session owns discovery/status sockets; this module owns only NFS/dbserver.
use crate::library::{self, Location};
use prolink::serve::{
    MediaSet, Medium, ServedSlot,
    dbserver::{DbServer, DbServerConfig},
    medium::Analysis,
    nfs::{NfsConfig, NfsServer},
    vfs::Vfs,
};
use prolink::virtual_cdj::LoadedTracks;
use prolink_proto::{BrowsableDeviceNumber, DeviceName, DeviceNumber, Slot, djl, status};
use prolink_rekordbox::{AnlzFile, Library, Track};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::{net::UdpSocket, time::Instant};

pub const NUMBER: u8 = 4;
const NAME: &str = "CDJ-2000nexus";
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub source: String,
    pub generation: u64,
    pub track: u32,
}
pub struct Prepared {
    pub key: Key,
    pub track: Track,
    pub audio: PathBuf,
    pub analysis: Arc<Analysis>,
    pub artwork: Vec<u8>,
    pub ui: crate::live::Asset,
}
pub fn key(body: &Value) -> Result<Key, String> {
    Ok(Key {
        source: body["source"]
            .as_str()
            .filter(|s| s.starts_with("local-usb:"))
            .ok_or("Select a local USB")?
            .into(),
        generation: body["generation"]
            .as_u64()
            .ok_or("Missing library generation")?,
        track: body["trackId"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or("Invalid track")?,
    })
}
pub fn current(catalogs: &library::Shared, key: &Key) -> bool {
    matches!(
        library::artwork_location(catalogs, &key.source, key.generation),
        Some((Location::Local(_), _))
    )
}
fn confined(root: &Path, path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("Missing exported file path".into());
    }
    let path = root
        .join(path.trim_start_matches('/'))
        .canonicalize()
        .map_err(|e| format!("USB file unavailable: {e}"))?;
    if !path.starts_with(root) || !path.is_file() {
        return Err("Exported file is outside the selected USB".into());
    }
    Ok(path)
}
fn small_file(root: &Path, path: &str, max: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let path = confined(root, path).ok()?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(max + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
}
pub fn prepare(catalogs: &library::Shared, key: Key) -> Result<Prepared, String> {
    let query = library::Query::from([("generation".into(), key.generation.to_string())]);
    let (_, catalog) = library::catalog(catalogs, &key.source, &query)?;
    let Some((Location::Local(database), _)) =
        library::artwork_location(catalogs, &key.source, key.generation)
    else {
        return Err("Local USB changed or disconnected".into());
    };
    let root = database
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("Invalid USB database path")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let mut track = catalog
        .library
        .tracks
        .get(&key.track)
        .cloned()
        .ok_or("Track is no longer in this USB")?;
    if track.container.name().is_none() {
        return Err("Unknown audio format; cannot safely describe this track to a CDJ".into());
    }
    let audio = confined(&root, &track.file_path)?;
    track.file_size = u32::try_from(audio.metadata().map_err(|e| e.to_string())?.len())
        .map_err(|_| "Audio files larger than 4 GiB cannot be served")?;
    let parse = |path: &str| {
        small_file(&root, path, 32 * 1024 * 1024).and_then(|b| AnlzFile::parse(&b).ok())
    };
    let analysis = Arc::new(Analysis {
        dat: parse(&track.analyze_path),
        ext: track.analyze_ext_path().as_deref().and_then(parse),
    });
    let artwork = small_file(&root, &track.artwork_path, 8 * 1024 * 1024).unwrap_or_default();
    let native = Path::new(&track.analyze_path)
        .with_extension("2EX")
        .to_string_lossy()
        .into_owned();
    let mut ui = small_file(&root, &native, 32 * 1024 * 1024)
        .and_then(|b| pioneer_companion_core::decode(&b).ok())
        .and_then(|w| serde_json::to_value(w).ok())
        .unwrap_or_else(|| serde_json::json!({"detail":null,"preview":null}));
    let mut beats = vec![];
    let mut marks = vec![];
    let mut cues = vec![];
    if let Some(dat) = &analysis.dat {
        crate::cues::collect(dat, &mut cues);
        if let Some(grid) = dat.beat_grid() {
            for b in &grid.beats {
                let time = f64::from(b.time) / 1000.0;
                beats.push(time);
                marks.push(serde_json::json!({"time":time,"beatInBar":b.beat_number}));
            }
        }
    }
    if let Some(ext) = &analysis.ext {
        crate::cues::collect(ext, &mut cues);
    }
    ui["track"] = crate::library::metadata(&track);
    ui["track"]["databaseFormat"] = serde_json::json!("OneLibrary · exportLibrary.db");
    ui["track"]["audioHeader"] = crate::audio_header::inspect(&audio).unwrap_or(Value::Null);
    ui["beats"] = serde_json::json!(marks);
    ui["cues"] = serde_json::json!(cues);
    if let Some(structure) = analysis.ext.as_ref().and_then(|e| e.song_structure()) {
        ui["phrases"] = serde_json::json!(crate::phrases::segments(
            structure,
            &beats,
            f64::from(track.duration)
        ));
        ui["phraseMood"] = serde_json::json!(crate::phrases::mood_label(structure.mood));
    }
    let image = if artwork.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image/jpeg", artwork.clone()))
    } else if artwork.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", artwork.clone()))
    } else {
        None
    };
    let ui = crate::live::Asset {
        artwork: image,
        analysis: ui,
        beats,
        warning: Some("Local USB — keep OLC running and the USB connected during playback".into()),
    };
    if !current(catalogs, &key) {
        return Err("USB changed while preparing track".into());
    }
    Ok(Prepared {
        key,
        track,
        audio,
        analysis,
        artwork,
        ui,
    })
}
struct Registration {
    // Announcements alone do not mean a deck has discovered our USB.
    media_queried: bool,
    sent: usize,
    next: Instant,
    status_at: Instant,
    counter: u32,
}
pub struct Server {
    _nfs: NfsServer,
    _db: DbServer,
    vfs: Arc<RwLock<Vfs>>,
    media: Arc<MediaSet>,
    library: Library,
    entries: BTreeMap<Key, u32>,
    analysis: BTreeMap<u32, Arc<Analysis>>,
    artwork: BTreeMap<u32, Vec<u8>>,
    registrations: BTreeMap<Ipv4Addr, Registration>,
    ui: BTreeMap<u32, crate::live::Asset>,
    pub local: Ipv4Addr,
    session: u128,
}
impl Server {
    pub async fn start(local: Ipv4Addr) -> Result<Self, String> {
        Self::start_with(
            local,
            NfsConfig::default(),
            DbServerConfig {
                device: BrowsableDeviceNumber::new(NUMBER).unwrap(),
                address: local,
                ..Default::default()
            },
        )
        .await
    }
    async fn start_with(
        local: Ipv4Addr,
        nfs_config: NfsConfig,
        db_config: DbServerConfig,
    ) -> Result<Self, String> {
        let vfs = Arc::new(RwLock::new(Vfs::new()));
        #[cfg(target_os = "macos")]
        let portmapper = if nfs_config.portmap_port == 111 {
            let socket = tokio::task::spawn_blocking(crate::mac_networking::acquire)
                .await
                .map_err(|e| e.to_string())??;
            Some(UdpSocket::from_std(socket).map_err(|e| e.to_string())?)
        } else {
            None
        };
        #[cfg(not(target_os = "macos"))]
        let portmapper = None;
        let nfs = NfsServer::start_with_portmapper(vfs.clone(), nfs_config, portmapper)
            .await
            .map_err(|e| format!("Local USB NFS unavailable: {e}"))?;
        let media = Arc::new(MediaSet::new([]));
        let db =
            DbServer::start_watching(db_config, media.clone(), Arc::new(LoadedTracks::default()))
                .await
                .map_err(|e| format!("Local USB metadata server unavailable: {e}"))?;
        if db.query_port().is_none() {
            return Err("Local USB metadata query port is in use".into());
        }
        Ok(Self {
            _nfs: nfs,
            _db: db,
            vfs,
            media,
            library: Library::default(),
            entries: BTreeMap::new(),
            analysis: BTreeMap::new(),
            artwork: BTreeMap::new(),
            registrations: BTreeMap::new(),
            ui: BTreeMap::new(),
            local,
            session: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        })
    }
    pub fn add(&mut self, prepared: Prepared) -> Result<u32, String> {
        if let Some(id) = self.entries.get(&prepared.key) {
            return Ok(*id);
        }
        if self.entries.len() >= 128 {
            return Err(
                "Local serving session is full; reconnect after stopping both players".into(),
            );
        }
        // Never reuse an earlier session's track ID or NFS path for new audio.
        // A CDJ may retain decoder/cue information beyond our server's lifetime.
        let identity = format!("{}:{:?}", self.session, prepared.key);
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        identity.hash(&mut hasher);
        let mut id = (hasher.finish() as u32 & 0x7fff_ffff).max(1);
        while self.library.tracks.contains_key(&id) {
            id = (id + 1) & 0x7fff_ffff;
            id = id.max(1);
        }

        let mut track = prepared.track;
        let extension = prepared
            .audio
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("audio");
        track.id = id;
        track.file_path = format!("/OLC/{:x}/{id}/audio.{extension}", self.session);
        track.filename = format!("audio.{extension}");
        self.vfs
            .write()
            .unwrap()
            .add_disk_file(&format!("/C{}", track.file_path), &prepared.audio)
            .map_err(|e| e.to_string())?;
        // IDs in different OneLibrary databases are unrelated. Remap every
        // name-table reference as well as the audio path and track identity.
        for (table, field, name) in [
            (
                &mut self.library.artists,
                &mut track.artist_id,
                &track.artist,
            ),
            (&mut self.library.albums, &mut track.album_id, &track.album),
            (&mut self.library.genres, &mut track.genre_id, &track.genre),
            (&mut self.library.keys, &mut track.key_id, &track.key),
            (&mut self.library.labels, &mut track.label_id, &track.label),
        ] {
            *field = id;
            table.insert(id, name.clone());
        }
        track.composer_id = 0;
        track.original_artist_id = 0;
        track.remixer_id = 0;
        track.artwork_id = id;
        self.ui.insert(id, prepared.ui);
        self.analysis.insert(id, prepared.analysis);
        self.artwork.insert(id, prepared.artwork);
        self.library.tracks.insert(id, track);
        let medium = Medium::synthetic(ServedSlot::USB, self.library.clone(), "OLC LOCAL USB");
        medium.seed_assets(self.analysis.clone(), self.artwork.clone());
        self.media.insert(Arc::new(medium));
        self.entries.insert(prepared.key, id);
        Ok(id)
    }
    pub fn asset(&self, id: u32) -> Option<crate::live::Asset> {
        self.ui.get(&id).cloned()
    }
    pub fn track(&self, id: u32) -> Option<&Track> {
        self.library.tracks.get(&id)
    }
    pub fn ready(&self, ip: Ipv4Addr) -> bool {
        self.registrations
            .get(&ip)
            .is_some_and(|r| r.media_queried && r.sent >= stages(self.local).len())
    }
    pub fn collision(&self, bytes: &[u8], from: std::net::IpAddr) -> bool {
        if from == self.local {
            return false;
        }
        djl::Packet::decode(bytes).is_ok_and(|p| p.body.device_number() == Some(NUMBER))
    }
    pub fn respond(&mut self, bytes: &[u8], peer: Ipv4Addr, socket: &UdpSocket) {
        let number = DeviceNumber::new(NUMBER).unwrap();
        let name = DeviceName::new(NAME);
        let packet = status::decode(bytes);
        let media_query = matches!(&packet, Ok(status::Packet::MediaQuery(q))
            if q.target == number && q.slot == Slot::USB);
        let response = match packet {
            Ok(status::Packet::MediaQuery(q)) if q.target == number && q.slot == Slot::USB => Some(
                status::MediaResponse::builder()
                    .name(name)
                    .device_number(number)
                    .slot(Slot::USB)
                    .volume_name("OLC LOCAL USB")
                    .counts(self.library.tracks.len() as u32, 0)
                    .build()
                    .as_bytes()
                    .to_vec(),
            ),
            Ok(status::Packet::SettingsQuery(q)) => Some(
                status::SettingsResponse::build(name, number, q.requester, q.slot, &[])
                    .as_bytes()
                    .to_vec(),
            ),
            _ => None,
        };
        if let Some(reply) = response
            && socket.try_send_to(&reply, (peer, 50002).into()).is_ok()
            && media_query
            && let Some(registration) = self.registrations.get_mut(&peer)
        {
            registration.media_queried = true;
        }
    }
    pub fn tick(
        &mut self,
        peers: &[(Ipv4Addr, Ipv4Addr)],
        discovery: &UdpSocket,
        socket: &UdpSocket,
    ) -> Result<(), String> {
        let stages = stages(self.local);
        self.registrations
            .retain(|ip, _| peers.iter().any(|(p, _)| p == ip));
        for &(ip, local) in peers {
            if local != self.local {
                return Err(
                    "Local USB serving requires both CDJs on the same network interface".into(),
                );
            }
            let r = self.registrations.entry(ip).or_insert(Registration {
                media_queried: false,
                sent: 0,
                next: Instant::now(),
                status_at: Instant::now(),
                counter: 0,
            });
            if Instant::now() >= r.next {
                discovery
                    .try_send_to(&stages[r.sent.min(stages.len() - 1)], (ip, 50000).into())
                    .map_err(|e| e.to_string())?;
                r.sent += 1;
                r.next = Instant::now()
                    + Duration::from_millis(if r.sent < stages.len() { 300 } else { 1500 });
            }
            if r.sent >= stages.len() && Instant::now() >= r.status_at {
                r.counter = r.counter.wrapping_add(1);
                let packet = crate::local_usb_probe::source_status(
                    DeviceName::new(NAME),
                    DeviceNumber::new(NUMBER).unwrap(),
                    r.counter,
                    status::MediaState::LOADED,
                );
                socket
                    .try_send_to(packet.as_bytes(), (ip, 50002).into())
                    .map_err(|e| e.to_string())?;
                r.status_at = Instant::now() + Duration::from_millis(200);
            }
        }
        Ok(())
    }
}
fn stages(local: Ipv4Addr) -> Vec<Vec<u8>> {
    let mut stages = crate::direct_status::registration(local);
    stages.push(crate::direct_status::announcement(local));
    stages
        .into_iter()
        .map(|bytes| {
            let mut p = djl::Packet::decode(&bytes).unwrap();
            p.name = DeviceName::new(NAME);
            match &mut p.body {
                djl::Body::ClaimMac { mac, .. } => mac.0[1] = 81,
                djl::Body::ClaimIp {
                    device_number, mac, ..
                }
                | djl::Body::KeepAlive {
                    device_number, mac, ..
                } => {
                    *device_number = NUMBER;
                    mac.0[1] = 81;
                }
                djl::Body::ClaimNumber { device_number, .. } => *device_number = NUMBER,
                _ => (),
            }
            p.encode()
        })
        .collect()
}

/// Mirror the UI's known hardware limits at the actual send boundary.
pub fn compatible(track: &Track, model: &str) -> Result<(), String> {
    let model: String = model
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_uppercase)
        .collect();
    let nexus = matches!(model.as_str(), "CDJ2000NEXUS" | "CDJ2000NXS");
    let newer = matches!(model.as_str(), "CDJ2000NXS2" | "CDJ2000NEXUS2" | "CDJ3000");
    if !nexus && !newer {
        return Ok(());
    }
    let format = track.container.name().unwrap_or("").to_ascii_lowercase();
    let lossless = matches!(format.as_str(), "wav" | "aiff" | "flac" | "alac");
    if nexus && matches!(format.as_str(), "flac" | "alac") {
        return Err(
            "UNSUPPORTED FORMAT: this CDJ cannot decode this audio; no transcoding is performed"
                .into(),
        );
    }
    let allowed: &[u32] = if lossless {
        if nexus {
            &[44100, 48000]
        } else {
            &[44100, 48000, 88200, 96000]
        }
    } else if model == "CDJ3000" {
        &[44100, 48000]
    } else if format == "mp3" {
        &[32000, 44100, 48000]
    } else {
        &[16000, 22050, 24000, 32000, 44100, 48000]
    };
    if track.sample_rate > 0 && !allowed.contains(&track.sample_rate)
        || lossless && track.sample_depth > 0 && ![16, 24].contains(&track.sample_depth)
    {
        return Err(
            "UNSUPPORTED FORMAT: sample rate or bit depth is not supported by this CDJ".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prolink_rekordbox::Container;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "olc-serving-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn register(&self, catalogs: &library::Shared, index: u8) -> (Key, Vec<u8>) {
            let root = self.0.join(index.to_string());
            std::fs::create_dir_all(root.join("PIONEER/rekordbox")).unwrap();
            let audio: Vec<u8> = (0..120_007)
                .map(|i| ((i * 17 + usize::from(index) * 31) % 256) as u8)
                .collect();
            std::fs::write(root.join("audio.wav"), &audio).unwrap();
            let track = Track {
                id: 1,
                title: format!("Track from USB {index}"),
                file_path: "/audio.wav".into(),
                container: Container::WAV,
                sample_rate: 44100,
                sample_depth: 24,
                ..Default::default()
            };
            let catalog = library::local_catalog(
                Library {
                    tracks: [(1, track)].into(),
                    ..Default::default()
                },
                None,
                index.to_string(),
            );
            // Preserve prior catalogs as scanner polls do.
            let mut ready: Vec<_> = library::sources(catalogs)["sources"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| {
                    (
                        s["id"].as_str().unwrap().to_owned(),
                        s["label"].as_str().unwrap().to_owned(),
                        self.0
                            .join(s["id"].as_str().unwrap().trim_start_matches("local-usb:"))
                            .join("PIONEER/rekordbox/exportLibrary.db"),
                        None,
                    )
                })
                .collect();
            let source = format!("local-usb:{index}");
            ready.push((
                source.clone(),
                format!("USB {index}"),
                root.join("PIONEER/rekordbox/exportLibrary.db"),
                Some(catalog),
            ));
            library::sync_local(catalogs, ready);
            let sources = library::sources(catalogs);
            let generation = sources["sources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["id"] == source)
                .unwrap()["generation"]
                .as_u64()
                .unwrap();
            (
                Key {
                    source,
                    generation,
                    track: 1,
                },
                audio,
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    async fn server() -> Server {
        Server::start_with(
            Ipv4Addr::LOCALHOST,
            NfsConfig {
                interface: None,
                portmap_port: 0,
                mount_port: 0,
                nfs_port: 0,
            },
            DbServerConfig {
                device: BrowsableDeviceNumber::new(NUMBER).unwrap(),
                address: Ipv4Addr::LOCALHOST,
                port: 0,
                query_port: Some(0),
            },
        )
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn three_usb_ids_remain_distinct_and_nfs_delivers_original_bytes() {
        use prolink::consume::nfs::{NfsClient, NfsConfig as ClientConfig};
        let fixture = Fixture::new();
        let catalogs = library::with_local_path(None, None);
        let mut server = server().await;
        let mut expected = vec![];
        for i in 1..=3 {
            let (key, bytes) = fixture.register(&catalogs, i);
            let wire = server
                .add(prepare(&catalogs, key.clone()).unwrap())
                .unwrap();
            assert!(wire > 0);
            assert!(!expected.iter().any(|(previous, _)| *previous == wire));
            assert_eq!(server.add(prepare(&catalogs, key).unwrap()).unwrap(), wire);
            expected.push((wire, bytes));
        }
        let mut client = NfsClient::connect_with(
            Ipv4Addr::LOCALHOST,
            None,
            ClientConfig {
                portmap_port: server._nfs.ports().portmap,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let mount = client.mount_slot(Slot::USB).await.unwrap();
        for (wire, bytes) in expected {
            let track = server.track(wire).unwrap();
            let file = client.open(&mount, &track.file_path).await.unwrap();
            assert_eq!(client.read_file(&file).await.unwrap(), bytes);
            // Out-of-order, non-frame-aligned ranges model seeking and two decks.
            for offset in [8193, 37, 100_001, 0, bytes.len() - 17] {
                let got = client.read_at(&file, offset as u64, 8192).await.unwrap();
                assert_eq!(got, bytes[offset..(offset + 8192).min(bytes.len())]);
            }
        }
        assert_eq!(
            server.media.get(Slot::USB).unwrap().library().tracks.len(),
            3
        );
        assert!(
            library::sources(&catalogs)["sources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["loadable"] == crate::mac_networking::available())
        );
        if !crate::mac_networking::available() {
            assert!(
                library::sources(&catalogs)["sources"][0]["loadUnavailableReason"]
                    .as_str()
                    .unwrap()
                    .contains("Local USB Support")
            );
        }
    }
    #[tokio::test]
    async fn a_new_serving_session_does_not_reuse_track_or_file_identity() {
        let fixture = Fixture::new();
        let catalogs = library::with_local_path(None, None);
        let (key, _) = fixture.register(&catalogs, 1);
        let mut first = server().await;
        first.session = 123456789012345678;
        let first_id = first.add(prepare(&catalogs, key.clone()).unwrap()).unwrap();
        let first_path = first.track(first_id).unwrap().file_path.clone();
        let mut second = server().await;
        second.session = 123456789012345679;
        let second_id = second.add(prepare(&catalogs, key).unwrap()).unwrap();
        let second_path = &second.track(second_id).unwrap().file_path;
        assert_ne!(first_id, second_id);
        assert_ne!(&first_path, second_path);
        assert_ne!(Vfs::handle_for(&first_path), Vfs::handle_for(second_path));
    }

    #[test]
    fn stale_selection_missing_file_and_escaped_path_are_rejected() {
        let fixture = Fixture::new();
        let catalogs = library::with_local_path(None, None);
        let (mut key, _) = fixture.register(&catalogs, 1);
        key.generation += 1;
        assert!(prepare(&catalogs, key).is_err());
        assert!(confined(&fixture.0, "../outside.wav").is_err());
        std::fs::write(fixture.0.join("outside.wav"), b"private").unwrap();
        std::os::unix::fs::symlink(
            fixture.0.join("outside.wav"),
            fixture.0.join("1/escape.wav"),
        )
        .unwrap();
        assert!(confined(&fixture.0.join("1"), "escape.wav").is_err());
        let source = library::sources(&catalogs)["sources"][0].clone();
        let key = Key {
            source: "local-usb:1".into(),
            generation: source["generation"].as_u64().unwrap(),
            track: 1,
        };
        std::fs::remove_file(fixture.0.join("1/audio.wav")).unwrap();
        assert!(prepare(&catalogs, key).is_err());
    }
    #[test]
    fn original_formats_are_checked_without_transcoding() {
        let mut track = Track {
            container: Container::WAV,
            sample_rate: 44100,
            sample_depth: 24,
            ..Default::default()
        };
        assert!(compatible(&track, "CDJ-2000nexus").is_ok());
        track.container = Container::FLAC;
        assert!(compatible(&track, "CDJ-2000nexus").is_err());
        assert!(compatible(&track, "CDJ-2000NXS2").is_ok());
        track.container = Container::WAV;
        track.sample_rate = 96000;
        assert!(compatible(&track, "CDJ-2000nexus").is_err());
        assert!(compatible(&track, "CDJ-3000").is_ok());
    }
    #[tokio::test]
    async fn each_deck_must_query_local_usb_before_first_load() {
        let mut server = server().await;
        let first = Ipv4Addr::LOCALHOST;
        let second = Ipv4Addr::new(127, 0, 0, 2);
        for ip in [first, second] {
            server.registrations.insert(
                ip,
                Registration {
                    media_queried: false,
                    sent: stages(server.local).len(),
                    next: Instant::now(),
                    status_at: Instant::now(),
                    counter: 1,
                },
            );
            assert!(!server.ready(ip), "sending announcements is not readiness");
        }
        for (target, slot, expected) in [
            (1, Slot::USB, false),
            (NUMBER, Slot::SD, false),
            (NUMBER, Slot::USB, true),
        ] {
            let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            socket.writable().await.unwrap();
            let query = status::MediaQuery {
                requester: DeviceNumber::new(1).unwrap(),
                requester_ip: first,
                target: DeviceNumber::new(target).unwrap(),
                slot,
            }
            .encode(DeviceName::new("CDJ-2000nexus"));
            server.respond(&query, first, &socket);
            assert_eq!(server.ready(first), expected);
            assert!(
                !server.ready(second),
                "LINK on one deck cannot ready another"
            );
        }
        // Removing a peer also removes its readiness; reconnection must discover again.
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        server.tick(&[], &socket, &socket).unwrap();
        assert!(!server.ready(first));
    }
    #[test]
    fn source_registration_has_distinct_identity_from_observer() {
        let ip = Ipv4Addr::new(192, 0, 2, 10);
        for bytes in stages(ip) {
            let packet = djl::Packet::decode(&bytes).unwrap();
            if let djl::Body::KeepAlive {
                device_number, mac, ..
            } = packet.body
            {
                assert_eq!(device_number, NUMBER);
                assert_eq!(mac.0[1], 81);
            }
        }
    }
}
