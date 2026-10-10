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

static STATUS: std::sync::Mutex<Value> = std::sync::Mutex::new(Value::Null);
pub fn status() -> Value {
    STATUS.lock().unwrap().clone()
}
pub fn publish_status(value: Value) {
    *STATUS.lock().unwrap() = value;
}
pub const NUMBER: u8 = 4;
const NAME: &str = "CDJ-2000nexus";
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub source: String,
    pub generation: u64,
    pub track: u32,
    pub variant: String,
}
#[derive(Clone, Default)]
pub struct UsbDetails {
    label: String,
    settings: Vec<u8>,
    capacity: Option<(u64, u64)>,
    stats: Option<prolink_proto::rpc::nfs2::FsStat>,
}
impl UsbDetails {
    fn read(root: &Path, label: String) -> Self {
        let settings = small_file(root, Medium::SETTINGS_PATH, 4096)
            .and_then(|raw| prolink_rekordbox::SettingsFile::parse(&raw).ok())
            .filter(|file| file.checksum_matches() == Some(true))
            .map(|file| file.wire_settings().to_vec())
            .filter(|bytes| bytes.len() == 32)
            .unwrap_or_default();
        let stats = filesystem_stats(root);
        Self {
            label,
            settings,
            capacity: stats.map(|s| {
                (
                    u64::from(s.blocks) * u64::from(s.bsize),
                    u64::from(s.bfree) * u64::from(s.bsize),
                )
            }),
            stats,
        }
    }
}

#[cfg(unix)]
#[allow(clippy::useless_conversion)] // libc block-count widths differ on Apple and Linux.
fn filesystem_stats(root: &Path) -> Option<prolink_proto::rpc::nfs2::FsStat> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(root.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is NUL-terminated and stat points to writable storage. Only
    // read it after statvfs reports success. This call never modifies the USB.
    let stat = unsafe {
        if libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) != 0 {
            return None;
        }
        stat.assume_init()
    };
    Some(prolink_proto::rpc::nfs2::FsStat {
        tsize: 8192,
        bsize: u32::try_from(stat.f_frsize).ok().filter(|v| *v > 0)?,
        blocks: u32::try_from(stat.f_blocks).ok()?,
        bfree: u32::try_from(stat.f_bfree).ok()?,
        bavail: u32::try_from(stat.f_bavail).ok()?,
    })
}
#[cfg(not(unix))]
fn filesystem_stats(_: &Path) -> Option<prolink_proto::rpc::nfs2::FsStat> {
    None
}

pub struct Prepared {
    pub key: Key,
    pub track: Track,
    pub audio: PathBuf,
    pub converted: Option<Arc<crate::transcoding::Artifact>>,
    pub analysis: Arc<Analysis>,
    pub artwork: Vec<u8>,
    pub ui: crate::live::Asset,
    pub usb: UsbDetails,
}
pub fn key(body: &Value) -> Result<Key, String> {
    Ok(Key {
        variant: String::new(),
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
    for (name, file) in [
        ("DAT", analysis.dat.as_ref()),
        ("EXT", analysis.ext.as_ref()),
    ] {
        let lists: Vec<_> = file
            .into_iter()
            .flat_map(|f| f.cue_lists())
            .map(|l| format!("PCOB:{}:{}", l.list_type.0, l.cues.len()))
            .chain(
                file.into_iter()
                    .flat_map(|f| f.extended_cue_lists())
                    .map(|l| format!("PCO2:{}:{}", l.list_type.0, l.cues.len())),
            )
            .collect();
        prolink::serve::diagnostics::record(format!(
            "analysis_source track={} file={name} parsed={} lists={lists:?}",
            key.track,
            file.is_some()
        ));
    }
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
    let label = library::sources(catalogs)["sources"]
        .as_array()
        .and_then(|sources| {
            sources
                .iter()
                .find(|s| s["id"] == key.source && s["generation"] == key.generation)
        })
        .and_then(|source| source["label"].as_str())
        .map(|label| {
            label
                .strip_prefix("LOCAL USB · ")
                .unwrap_or(label)
                .to_owned()
        })
        .unwrap_or_else(|| {
            root.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let usb = UsbDetails::read(&root, label);
    Ok(Prepared {
        usb,
        key,
        track,
        audio,
        converted: None,
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
    converted: Vec<Arc<crate::transcoding::Artifact>>,
    pub local: Ipv4Addr,
    source: Option<(String, u64)>,
    audio_paths: BTreeMap<String, PathBuf>,
    usb: UsbDetails,
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
        let mut tree = Vfs::new();
        tree.add_directory("/C");
        let vfs = Arc::new(RwLock::new(tree));
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
        let media = Arc::new(MediaSet::new([Arc::new(Medium::synthetic(
            ServedSlot::USB,
            Library::default(),
            "OLC LOCAL USB",
        ))]));
        let db =
            DbServer::start_watching(db_config, media.clone(), Arc::new(LoadedTracks::default()))
                .await
                .map_err(|e| format!("Local USB metadata server unavailable: {e}"))?;
        if db.query_port().is_none() {
            return Err("Local USB metadata query port is in use".into());
        }
        prolink::serve::diagnostics::record(format!(
            "source_started local={local} nfs={:?} db={}",
            nfs.ports(),
            db.port()
        ));
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
            converted: Vec::new(),
            source: None,
            audio_paths: BTreeMap::new(),
            usb: UsbDetails {
                label: "OLC LOCAL USB".into(),
                ..Default::default()
            },
        })
    }
    pub fn validate_source(&self, key: &Key) -> Result<(), String> {
        if self.source.as_ref().is_some_and(|(source, generation)| {
            source != &key.source || *generation != key.generation
        }) {
            return Err("OLC serves one local USB library per connection session. Stop both CDJs, then restart OLC or reconnect both manual connections to use another or refreshed USB library.".into());
        }
        Ok(())
    }
    pub fn add(&mut self, prepared: Prepared) -> Result<u32, String> {
        self.validate_source(&prepared.key)?;
        if let Some(id) = self.entries.get(&prepared.key) {
            return Ok(*id);
        }
        if self.entries.len() >= 128 {
            return Err(
                "Local serving session is full; reconnect after stopping both players".into(),
            );
        }
        let id = prepared.key.track;
        if id == 0 || prepared.track.id != id {
            return Err("Invalid original USB track ID; no track published".into());
        }
        if self.library.tracks.contains_key(&id) {
            return Err("This track is already served with different audio settings. Stop both CDJs, disconnect both in OLC, then reconnect before changing its audio format.".into());
        }
        let mut track = prepared.track;
        // Only explicitly unsupported audio needs a cache path. Compatible
        // tracks keep the USB database's ID, filename and path verbatim.
        if prepared.converted.is_some() {
            let extension = prepared
                .audio
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or("audio");
            track.file_path = format!(
                "/OLC-converted/{id}/{}/audio.{extension}",
                prepared.key.variant
            );
            track.filename = format!("audio.{extension}");
        }
        let path = format!("/C/{}", track.file_path.trim_start_matches('/'));
        if self
            .audio_paths
            .get(&path)
            .is_some_and(|audio| audio != &prepared.audio)
        {
            return Err("USB audio path already refers to another file; no track published".into());
        }
        self.vfs
            .write()
            .unwrap()
            .add_disk_file(&path, &prepared.audio)
            .map_err(|e| e.to_string())?;
        if self.source.is_none() {
            self.usb = prepared.usb;
            self.vfs.write().unwrap().set_filesystem_stats(
                "/C",
                self.usb.stats.unwrap_or(prolink_proto::rpc::nfs2::FsStat {
                    tsize: 8192,
                    bsize: 512,
                    blocks: 0,
                    bfree: 0,
                    bavail: 0,
                }),
            );
        }
        self.audio_paths.insert(path, prepared.audio);
        self.source = Some((prepared.key.source.clone(), prepared.key.generation));
        if let Some(artifact) = prepared.converted {
            self.converted.push(artifact);
        }
        // A connection serves one USB: retain its reference IDs, including
        // shared artist/album/artwork rows and the absent-reference value zero.
        for (table, reference, name) in [
            (&mut self.library.albums, track.album_id, &track.album),
            (&mut self.library.genres, track.genre_id, &track.genre),
            (&mut self.library.keys, track.key_id, &track.key),
            (&mut self.library.labels, track.label_id, &track.label),
            (
                &mut self.library.artwork,
                track.artwork_id,
                &track.artwork_path,
            ),
        ] {
            if reference != 0 {
                table.insert(reference, name.clone());
            }
        }
        for (reference, name) in [
            (track.artist_id, &track.artist),
            (track.composer_id, &track.composer),
            (track.original_artist_id, &track.original_artist),
            (track.remixer_id, &track.remixer),
        ] {
            if reference != 0 {
                self.library.artists.insert(reference, name.clone());
            }
        }
        self.ui.insert(id, prepared.ui);
        self.analysis.insert(id, prepared.analysis);
        if track.artwork_id != 0 && !prepared.artwork.is_empty() {
            self.artwork.insert(track.artwork_id, prepared.artwork);
        }
        self.library.tracks.insert(id, track);
        let medium = Medium::synthetic(ServedSlot::USB, self.library.clone(), &self.usb.label)
            .with_usb_details(self.usb.settings.clone(), self.usb.capacity);
        medium.seed_assets(self.analysis.clone(), self.artwork.clone());
        self.media.insert(Arc::new(medium));
        prolink::serve::diagnostics::record(format!(
            "track_published source={} generation={} original={} served={id}",
            prepared.key.source, prepared.key.generation, prepared.key.track
        ));
        self.entries.insert(prepared.key, id);
        Ok(id)
    }
    pub fn asset(&self, id: u32) -> Option<crate::live::Asset> {
        self.ui.get(&id).cloned()
    }
    pub fn track(&self, id: u32) -> Option<&Track> {
        self.library.tracks.get(&id)
    }
    pub fn track_source_label(&self, id: u32) -> Option<&str> {
        self.track(id).map(|_| self.usb.label.as_str())
    }
    pub fn forget_peer(&mut self, ip: Ipv4Addr) {
        self.registrations.remove(&ip);
    }
    pub fn diagnostics(&self) -> Value {
        serde_json::json!({"active":true,"identityPolicy":"original-usb","source":self.source.as_ref().map(|s|&s.0),"generation":self.source.as_ref().map(|s|s.1),"local":self.local.to_string(),"publishedTracks":self.entries.len(),"peers":self.registrations.iter().map(|(ip,r)|serde_json::json!({"ip":ip.to_string(),"announced":r.sent>=stages(self.local).len(),"mediaQueried":r.media_queried})).collect::<Vec<_>>(),"mounts":self._nfs.mounts().iter().map(|m|format!("{} {}",m.peer,m.export)).collect::<Vec<_>>()})
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
                    .volume_name(&self.usb.label)
                    .created("")
                    .has_settings(!self.usb.settings.is_empty())
                    .size(
                        self.usb.capacity.map(|s| s.0).unwrap_or(0),
                        self.usb.capacity.map(|s| s.1).unwrap_or(0),
                    )
                    .counts(self.library.tracks.len() as u32, 0)
                    .build()
                    .as_bytes()
                    .to_vec(),
            ),
            Ok(status::Packet::SettingsQuery(q)) => Some(
                status::SettingsResponse::build(
                    name,
                    number,
                    q.requester,
                    q.slot,
                    if q.slot == Slot::USB {
                        &self.usb.settings
                    } else {
                        &[]
                    },
                )
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
            if !registration.media_queried {
                prolink::serve::diagnostics::record(format!("media_query_answered peer={peer}"));
            }
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
                if r.sent == 0 {
                    prolink::serve::diagnostics::record(format!("registration_started peer={ip}"));
                }
                discovery
                    .try_send_to(&stages[r.sent.min(stages.len() - 1)], (ip, 50000).into())
                    .map_err(|e| e.to_string())?;
                r.sent += 1;
                if r.sent == stages.len() {
                    prolink::serve::diagnostics::record(format!(
                        "registration_sent peer={ip} media_queried={}",
                        r.media_queried
                    ));
                }
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
impl Drop for Server {
    fn drop(&mut self) {
        publish_status(serde_json::json!({"active":false}));
        prolink::serve::diagnostics::record("source_stopped");
    }
}
pub(crate) fn stages(local: Ipv4Addr) -> Vec<Vec<u8>> {
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
    compatible_audio(
        track.container.name().unwrap_or(""),
        track.sample_rate,
        u32::from(track.sample_depth),
        model,
    )
}

pub fn compatible_audio(
    format: &str,
    sample_rate: u32,
    sample_depth: u32,
    model: &str,
) -> Result<(), String> {
    let model: String = model
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_uppercase)
        .collect();
    let nexus = matches!(model.as_str(), "CDJ2000" | "CDJ2000NEXUS" | "CDJ2000NXS");
    let newer = matches!(model.as_str(), "CDJ2000NXS2" | "CDJ2000NEXUS2" | "CDJ3000");
    if !nexus && !newer {
        return Ok(());
    }
    let format = format.to_ascii_lowercase();
    let lossless = matches!(format.as_str(), "wav" | "aiff" | "flac" | "alac");
    // Match the UI: unknown formats are inconclusive, never auto-transcode.
    if !lossless && !matches!(format.as_str(), "mp3" | "aac") {
        return Ok(());
    }
    if nexus && matches!(format.as_str(), "flac" | "alac") {
        return Err("UNSUPPORTED FORMAT: this CDJ cannot decode the original audio".into());
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
    if sample_rate > 0 && !allowed.contains(&sample_rate)
        || lossless && sample_depth > 0 && ![16, 24].contains(&sample_depth)
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
    const AUDIO_PATH: &str = "/Contents/Amnésie/Nhyx - Amne\u{301}sie.wav";
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
            let audio_file = root.join(AUDIO_PATH.trim_start_matches('/'));
            std::fs::create_dir_all(audio_file.parent().unwrap()).unwrap();
            std::fs::write(audio_file, &audio).unwrap();
            let track = Track {
                id: 1,
                title: format!("Track from USB {index}"),
                file_path: AUDIO_PATH.into(),
                filename: "Nhyx - Amne\u{301}sie.wav".into(),
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
                    variant: String::new(),
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
    #[test]
    fn only_valid_usb_saved_settings_are_published_without_modifying_the_file() {
        use prolink_rekordbox::settings::{
            HEADER_LEN, OFS_PAYLOAD, OFS_PAYLOAD_LEN, PAYLOAD_MAGIC,
        };
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.0.join("PIONEER")).unwrap();
        let root = fixture.0.canonicalize().unwrap();
        let path = root.join(Medium::SETTINGS_PATH);
        let settings: Vec<u8> = (0x80..0xa0).collect();
        let mut raw = vec![0; OFS_PAYLOAD];
        raw[..4].copy_from_slice(&HEADER_LEN.to_le_bytes());
        raw[OFS_PAYLOAD_LEN..OFS_PAYLOAD].copy_from_slice(&40u32.to_le_bytes());
        raw.extend(PAYLOAD_MAGIC.to_le_bytes());
        raw.extend(1u32.to_le_bytes());
        raw.extend(&settings);
        let crc = prolink_rekordbox::SettingsFile::parse(&raw)
            .unwrap()
            .computed_checksum();
        raw.extend(crc.to_le_bytes());
        raw.extend([0, 0]);
        std::fs::write(&path, &raw).unwrap();
        assert_eq!(UsbDetails::read(&root, "USB".into()).settings, settings);
        assert_eq!(std::fs::read(&path).unwrap(), raw);
        raw[OFS_PAYLOAD + 9] ^= 1;
        std::fs::write(&path, &raw).unwrap();
        assert!(UsbDetails::read(&root, "USB".into()).settings.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), raw);
        std::fs::remove_file(&path).unwrap();
        assert!(UsbDetails::read(&root, "USB".into()).settings.is_empty());
    }

    #[tokio::test]
    async fn published_tracks_keep_shared_usb_metadata_references_and_file_dates() {
        let fixture = Fixture::new();
        let catalogs = library::with_local_path(None, None);
        let (key, _) = fixture.register(&catalogs, 1);
        let mut server = server().await;
        for id in [1, 2] {
            let mut prepared = prepare(&catalogs, key.clone()).unwrap();
            prepared.key.track = id;
            prepared.track.id = id;
            prepared.track.artist_id = 31;
            prepared.track.artist = "Shared artist".into();
            prepared.track.album_id = 42;
            prepared.track.album = "Shared album".into();
            prepared.track.artwork_id = 53;
            prepared.track.artwork_path = "/PIONEER/Artwork/53.jpg".into();
            prepared.artwork = b"original image bytes".to_vec();
            prepared.track.composer_id = 64;
            prepared.track.composer = "Composer".into();
            prepared.track.remixer_id = 75;
            prepared.track.remixer = "Remixer".into();
            prepared.track.original_artist_id = 86;
            prepared.track.original_artist = "Original artist".into();
            let disk_modified = prepared
                .audio
                .metadata()
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as u32;
            let expected = prepared.track.clone();
            server.add(prepared).unwrap();
            let medium = server.media.get(Slot::USB).unwrap();
            let actual = medium.library().tracks.get(&id).unwrap();
            assert_eq!(actual.artist_id, expected.artist_id);
            assert_eq!(actual.album_id, expected.album_id);
            assert_eq!(actual.artwork_id, expected.artwork_id);
            assert_eq!(actual.composer_id, expected.composer_id);
            assert_eq!(actual.remixer_id, expected.remixer_id);
            assert_eq!(actual.original_artist_id, expected.original_artist_id);
            assert_eq!(actual.genre_id, 0);
            assert_eq!(medium.artwork(53), b"original image bytes");
            assert!(medium.artwork(id).is_empty());
            assert_eq!(medium.description().volume_name, "USB 1");
            assert!(medium.description().created.is_empty());
            assert_eq!(medium.description().track_count, id);
            let tree = server.vfs.read().unwrap();
            let handle = Vfs::handle_for(&format!("/C{AUDIO_PATH}"));
            assert_eq!(tree.attributes(handle).unwrap().mtime_sec, disk_modified);
            assert_eq!(tree.filesystem_stats(handle), server.usb.stats);
        }
        assert_eq!(server.library.artists.len(), 4);
        assert_eq!(server.library.albums.len(), 1);
        assert_eq!(server.library.artwork.len(), 1);
    }

    #[tokio::test]
    async fn empty_source_mount_survives_first_track_publication() {
        use prolink::consume::nfs::{NfsClient, NfsConfig as ClientConfig};
        let fixture = Fixture::new();
        let catalogs = library::with_local_path(None, None);
        assert!(!library::has_local_usb(&catalogs));
        let (key, bytes) = fixture.register(&catalogs, 1);
        assert!(library::has_local_usb(&catalogs));
        let mut server = server().await;
        assert!(
            server
                .media
                .get(Slot::USB)
                .unwrap()
                .library()
                .tracks
                .is_empty()
        );
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
        let id = server
            .add(prepare(&catalogs, key.clone()).unwrap())
            .unwrap();
        let mut metadata = prolink::consume::dbclient::DbClient::connect_at(
            Ipv4Addr::LOCALHOST,
            server._db.port(),
            BrowsableDeviceNumber::new(1).unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
        let info = metadata.track_info(Slot::USB, id).await.unwrap();
        assert_eq!(id, key.track);
        assert_eq!(info.path, AUDIO_PATH);
        assert_eq!(info.size as usize, bytes.len());
        assert_eq!(
            server.track(id).unwrap().filename,
            "Nhyx - Amne\u{301}sie.wav"
        );
        let file = client.open(&mount, &info.path).await.unwrap();
        assert_eq!(client.read_file(&file).await.unwrap(), bytes);
        library::sync_local(&catalogs, vec![]);
        assert!(!library::has_local_usb(&catalogs));
    }
    #[tokio::test]
    async fn one_usb_keeps_original_identity_and_rejects_colliding_libraries() {
        use prolink::consume::nfs::{NfsClient, NfsConfig as ClientConfig};
        let fixture = Fixture::new();
        let catalogs = library::with_local_path(None, None);
        let mut server = server().await;
        let mut expected = vec![];
        let original_load = |key| {
            crate::transcoding::prepare_load(
                &catalogs,
                key,
                "CDJ-2000nexus",
                "original-nfs-regression",
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .unwrap()
        };
        for i in 1..=3 {
            let (key, bytes) = fixture.register(&catalogs, i);
            if i == 1 {
                let wire = server.add(original_load(key.clone())).unwrap();
                assert_eq!(wire, key.track);
                assert_eq!(server.track(wire).unwrap().file_path, AUDIO_PATH);
                assert_eq!(server.add(original_load(key)).unwrap(), wire);
                expected.push((wire, bytes));
            } else {
                assert!(
                    server
                        .validate_source(&key)
                        .unwrap_err()
                        .contains("one local USB")
                );
                assert!(
                    server
                        .add(original_load(key))
                        .unwrap_err()
                        .contains("one local USB")
                );
            }
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
            1
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
    /// Opt-in, read-only check of a mounted export; never contacts a CDJ.
    #[tokio::test]
    #[ignore = "requires OLC_USB_AUDIT_ROOT pointing to a mounted export"]
    async fn mounted_usb_originals_survive_load_preparation_and_nfs_byte_for_byte() {
        use prolink::consume::nfs::{NfsClient, NfsConfig as ClientConfig};
        use sha2::{Digest, Sha256};
        let root = PathBuf::from(std::env::var("OLC_USB_AUDIT_ROOT").unwrap());
        let database = root.join(crate::onelibrary::DATABASE);
        let (library, tags, fingerprint) = crate::onelibrary::read(&database).unwrap();
        let catalogs = library::with_local_path(None, None);
        library::sync_local(
            &catalogs,
            vec![(
                "local-usb:read-only-audit".into(),
                "Audit".into(),
                database,
                Some(library::local_catalog(library, tags, fingerprint)),
            )],
        );
        let generation = library::sources(&catalogs)["sources"][0]["generation"]
            .as_u64()
            .unwrap();
        let mut server = server().await;
        let mut second_client = NfsClient::connect_with(
            Ipv4Addr::LOCALHOST,
            None,
            ClientConfig {
                portmap_port: server._nfs.ports().portmap,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let second_mount = second_client.mount_slot(Slot::USB).await.unwrap();
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
        // Alternate Beau and Soro again within one serving session to exercise
        // reused track IDs and paths, with two clients reading concurrently.
        for track in [892, 1162, 2047, 965, 1162, 965, 1019] {
            let key = Key {
                source: "local-usb:read-only-audit".into(),
                generation,
                track,
                variant: String::new(),
            };
            let original = prepare(&catalogs, key.clone()).unwrap();
            let prepared = crate::transcoding::prepare_load(
                &catalogs,
                key,
                "CDJ-2000nexus",
                "real-usb-audit",
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .unwrap();
            assert_eq!(prepared.audio, original.audio);
            assert_eq!(prepared.track, original.track);
            assert!(prepared.converted.is_none());
            assert!(prepared.key.variant.is_empty());
            let source = std::fs::read(&original.audio).unwrap();
            let wire_id = server.add(prepared).unwrap();
            let track = server.track(wire_id).unwrap();
            assert_eq!(wire_id, original.key.track);
            assert_eq!(track.file_path, original.track.file_path);
            assert_eq!(track.filename, original.track.filename);
            assert_eq!(track.container, prolink_rekordbox::Container::AIFF);
            assert_eq!(track.file_size as usize, source.len());
            let file = client.open(&mount, &track.file_path).await.unwrap();
            let second_file = second_client
                .open(&second_mount, &track.file_path)
                .await
                .unwrap();
            for offset in [0, 72, 5165064, 5259144, source.len() - 1386] {
                let (first, second) = tokio::join!(
                    client.read_range(&file, offset as u64, 9408),
                    second_client.read_range(&second_file, offset as u64, 9408),
                );
                let expected = &source[offset..(offset + 9408).min(source.len())];
                assert_eq!(first.unwrap(), expected);
                assert_eq!(second.unwrap(), expected);
            }
            let served = client.read_file(&file).await.unwrap();
            assert_eq!(served, source);
            assert_eq!(
                Sha256::digest(std::fs::read(&original.audio).unwrap()),
                Sha256::digest(&source)
            );
            eprintln!(
                "original={} bytes={} sha256={:x} NFS identical; no conversion",
                original.key.track,
                served.len(),
                Sha256::digest(&served)
            );
        }
    }

    #[tokio::test]
    async fn original_identity_survives_new_sessions_and_rejects_refresh_in_place() {
        let fixture = Fixture::new();
        let catalogs = library::with_local_path(None, None);
        let (key, _) = fixture.register(&catalogs, 1);
        let mut first = server().await;
        let first_id = first.add(prepare(&catalogs, key.clone()).unwrap()).unwrap();
        let first_path = first.track(first_id).unwrap().file_path.clone();
        let mut second = server().await;
        let second_id = second
            .add(prepare(&catalogs, key.clone()).unwrap())
            .unwrap();
        let second_path = &second.track(second_id).unwrap().file_path;
        assert_eq!(first_id, key.track);
        assert_eq!(first_id, second_id);
        assert_eq!(&first_path, second_path);
        assert_eq!(first_path, AUDIO_PATH);
        assert_eq!(Vfs::handle_for(&first_path), Vfs::handle_for(second_path));
        let mut refreshed = prepare(&catalogs, key.clone()).unwrap();
        refreshed.key.generation += 1;
        assert!(first.add(refreshed).unwrap_err().contains("one local USB"));
        let mut different_audio = prepare(&catalogs, key).unwrap();
        different_audio.key.variant = "different-profile".into();
        assert!(
            first
                .add(different_audio)
                .unwrap_err()
                .contains("different audio settings")
        );
        assert_eq!(first.track(first_id).unwrap().file_path, first_path);
    }

    #[tokio::test]
    async fn converted_audio_keeps_id_but_cannot_replace_original_path_or_variant() {
        let fixture = Fixture::new();
        let catalogs = library::with_local_path(None, None);
        let (key, original_bytes) = fixture.register(&catalogs, 1);
        let mut prepared = prepare(&catalogs, key.clone()).unwrap();
        let original_audio = prepared.audio.clone();
        let cache = fixture.0.join("cached.wav");
        std::fs::write(&cache, b"separate cached audio").unwrap();
        prepared.audio = cache.clone();
        prepared.key.variant = "cached.wav".into();
        prepared.converted = Some(Arc::new(crate::transcoding::Artifact {
            path: cache.clone(),
            info: crate::audio_conversion::ResultInfo {
                input: crate::audio_conversion::AudioInfo {
                    format: "flac".into(),
                    sample_rate: 44100,
                    sample_depth: 24,
                    channels: 2,
                    frames: None,
                },
                sample_rate: 44100,
                sample_depth: 24,
                frames: 0,
                bytes: 21,
                clipped_samples: 0,
            },
        }));
        let mut server = server().await;
        let id = server.add(prepared).unwrap();
        assert_eq!(id, key.track);
        assert_eq!(
            server.track(id).unwrap().file_path,
            "/OLC-converted/1/cached.wav/audio.wav"
        );
        assert!(
            server
                .add(prepare(&catalogs, key).unwrap())
                .unwrap_err()
                .contains("different audio settings")
        );
        assert_eq!(std::fs::read(original_audio).unwrap(), original_bytes);
        assert!(cache.exists());
        drop(server);
        assert!(!cache.exists());
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
            variant: String::new(),
            source: "local-usb:1".into(),
            generation: source["generation"].as_u64().unwrap(),
            track: 1,
        };
        std::fs::remove_file(fixture.0.join("1").join(AUDIO_PATH.trim_start_matches('/'))).unwrap();
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
        assert!(compatible(&track, "CDJ-2000").is_ok());
        track.container = Container::FLAC;
        assert!(compatible(&track, "CDJ-2000nexus").is_err());
        assert!(compatible(&track, "CDJ-2000").is_err());
        assert!(compatible(&track, "CDJ-2000NXS2").is_ok());
        track.container = Container::WAV;
        track.sample_rate = 96000;
        assert!(compatible(&track, "CDJ-2000nexus").is_err());
        assert!(compatible(&track, "CDJ-2000").is_err());
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
