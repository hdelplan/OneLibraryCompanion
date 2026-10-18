//! Host-owned My Settings profiles and once-per-connection automatic application.
//! Saved media is imported honestly as saved settings, never claimed as live readback.
use crate::{cdj_settings_command, live};
use prolink::{Interface, Slot, consume::nfs::NfsClient};
use prolink_proto::DeviceName;
use prolink_rekordbox::SettingsFile;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub type Shared = Arc<Mutex<Store>>;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub notes: String,
    pub revision: u64,
    pub settings: Vec<u8>,
    pub original_file: Vec<u8>,
    pub origin: String,
    pub created_at: u64,
    pub updated_at: u64,
}
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Automatic {
    pub enabled: bool,
    pub default_profile: Option<String>,
    #[serde(default)]
    pub players: BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Archive {
    version: u32,
    revision: u64,
    profiles: Vec<Profile>,
    automatic: Automatic,
}
impl Default for Archive {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            profiles: vec![],
            automatic: Automatic::default(),
        }
    }
}
pub struct Store {
    path: PathBuf,
    archive: Archive,
    error: Option<String>,
    events: Vec<Value>,
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn uid() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "profile-{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
#[derive(Clone, Serialize)]
pub struct Choice {
    value: u8,
    label: String,
}
#[derive(Clone, Serialize)]
pub struct Field {
    pub id: String,
    label: String,
    group: String,
    offset: usize,
    options: Vec<Choice>,
}
fn field(id: &str, label: &str, group: &str, offset: usize, options: &[(u8, &str)]) -> Field {
    Field {
        id: id.into(),
        label: label.into(),
        group: group.into(),
        offset,
        options: options
            .iter()
            .map(|(value, label)| Choice {
                value: *value,
                label: (*label).into(),
            })
            .collect(),
    }
}
pub fn schema() -> Vec<Field> {
    let toggle = &[(0x80, "Off"), (0x81, "On")];
    vec![
        field(
            "lcdBrightness",
            "LCD brightness",
            "Display",
            1,
            &[
                (0x81, "1 — dim"),
                (0x82, "2"),
                (0x83, "3"),
                (0x84, "4"),
                (0x85, "5 — bright"),
            ],
        ),
        field(
            "timeMode",
            "Time display",
            "Display",
            24,
            &[(0x80, "Elapsed"), (0x81, "Remaining")],
        ),
        field(
            "language",
            "Player language",
            "Display",
            4,
            &[
                (0x81, "English"),
                (0x82, "French"),
                (0x83, "German"),
                (0x84, "Italian"),
                (0x85, "Dutch"),
                (0x86, "Spanish"),
                (0x87, "Russian"),
                (0x88, "Korean"),
                (0x89, "Simplified Chinese"),
                (0x8a, "Traditional Chinese"),
                (0x8b, "Japanese"),
                (0x8c, "Portuguese"),
                (0x8d, "Swedish"),
                (0x8e, "Czech"),
                (0x8f, "Hungarian"),
                (0x90, "Danish"),
                (0x91, "Greek"),
                (0x92, "Turkish"),
            ],
        ),
        field("onAirDisplay", "On-air display", "Illumination", 0, toggle),
        field(
            "jogBrightness",
            "Jog-ring brightness",
            "Illumination",
            6,
            &[(0x80, "Off"), (0x81, "Dim"), (0x82, "Bright")],
        ),
        field(
            "jogIndicator",
            "Jog-ring indicator",
            "Illumination",
            7,
            toggle,
        ),
        field(
            "discIllumination",
            "Disc-slot illumination",
            "Illumination",
            12,
            &[(0x80, "Off"), (0x81, "Dim"), (0x82, "Bright")],
        ),
        field("slipFlashing", "Slip flashing", "Illumination", 8, toggle),
        field(
            "playMode",
            "Play mode",
            "Playback",
            15,
            &[(0x80, "Continuous"), (0x81, "Single")],
        ),
        field(
            "ejectLock",
            "Eject / load lock",
            "Playback",
            13,
            &[(0x80, "Unlocked"), (0x81, "Locked")],
        ),
        field("autoCue", "Auto Cue", "Cueing", 26, toggle),
        field(
            "autoCueLevel",
            "Auto Cue threshold",
            "Cueing",
            3,
            &[
                (0x80, "−36 dB"),
                (0x81, "−42 dB"),
                (0x82, "−48 dB"),
                (0x83, "−54 dB"),
                (0x84, "−60 dB"),
                (0x85, "−66 dB"),
                (0x86, "−72 dB"),
                (0x87, "−78 dB"),
                (0x88, "Memory"),
            ],
        ),
        field(
            "jogMode",
            "Jog mode",
            "Jog & tempo",
            25,
            &[(0x80, "CDJ"), (0x81, "Vinyl")],
        ),
        field(
            "tempoRange",
            "Tempo range",
            "Jog & tempo",
            28,
            &[
                (0x80, "±6%"),
                (0x81, "±10%"),
                (0x82, "±16%"),
                (0x83, "Wide"),
            ],
        ),
        field("masterTempo", "Master Tempo", "Jog & tempo", 27, toggle),
        field("quantize", "Quantize", "Timing", 2, toggle),
        field("sync", "Sync", "Timing", 14, toggle),
    ]
}
fn defaults() -> Vec<u8> {
    vec![
        0x81, 0x83, 0x81, 0x88, 0x81, 1, 0x82, 0x81, 0x81, 1, 1, 1, 0x82, 0x80, 0x80, 0x81, 0x80,
        0x81, 0x80, 0, 0, 0x80, 0, 0, 0x81, 0x81, 0x81, 0x80, 0x80, 0x80, 0, 0,
    ]
}
fn name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        Err("Use a profile name between 1 and 80 characters".into())
    } else {
        Ok(value.into())
    }
}
fn validate(archive: &Archive) -> Result<(), String> {
    if archive.version != 1 || archive.profiles.len() > 100 {
        return Err("Unsupported configuration archive".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    for p in &archive.profiles {
        name(&p.name)?;
        if p.id.len() > 128
            || p.origin.len() > 1024
            || p.id.is_empty()
            || !ids.insert(&p.id)
            || p.settings.len() != 32
            || !(0x81..=0x85).contains(&p.settings[1])
            || p.original_file.len() > 4096
            || p.notes.chars().count() > 2000
        {
            return Err("Invalid configuration profile".into());
        }
    }
    for id in archive
        .automatic
        .default_profile
        .iter()
        .chain(archive.automatic.players.values())
    {
        if !archive.profiles.iter().any(|p| &p.id == id) {
            return Err("An automatic profile does not exist".into());
        }
    }
    if archive.automatic.players.len() > 64
        || archive.automatic.players.keys().any(|k| k.len() > 100)
    {
        return Err("Too many player assignments".into());
    }
    if archive.automatic.enabled
        && archive.automatic.default_profile.is_none()
        && archive.automatic.players.is_empty()
    {
        return Err("Choose an automatic profile first".into());
    }
    Ok(())
}
impl Store {
    fn open(path: PathBuf) -> Self {
        let (archive, error) = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Archive>(&bytes)
                .map_err(|e| e.to_string())
                .and_then(|a| {
                    validate(&a)?;
                    Ok(a)
                }) {
                Ok(a) => (a, None),
                Err(e) => (
                    Archive::default(),
                    Some(format!(
                        "Cannot read saved CDJ configurations: {e}. The original file has been preserved."
                    )),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Archive::default(), None),
            Err(e) => (Archive::default(), Some(e.to_string())),
        };
        Self {
            path,
            archive,
            error,
            events: vec![],
        }
    }
    fn commit(&mut self, mut next: Archive) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        next.revision = self.archive.revision + 1;
        validate(&next)?;
        std::fs::create_dir_all(
            self.path
                .parent()
                .ok_or("Missing configuration directory")?,
        )
        .map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        self.archive = next;
        Ok(())
    }
    pub fn event(&mut self, value: Value) {
        let value = json!({"at":now(),"detail":value});
        self.events.push(value);
        if self.events.len() > 100 {
            self.events.remove(0);
        }
    }
    pub fn snapshot(&self) -> Value {
        let profiles:Vec<_>=self.archive.profiles.iter().map(|p|{
            let values:BTreeMap<_,_>=schema().into_iter().map(|f|(f.id,p.settings[f.offset])).collect();
            json!({"id":p.id,"name":p.name,"notes":p.notes,"revision":p.revision,"origin":p.origin,"createdAt":p.created_at,"updatedAt":p.updated_at,"values":values})
        }).collect();
        json!({"version":1,"revision":self.archive.revision,"profiles":profiles,"automatic":self.archive.automatic,"fields":schema(),"events":self.events,"error":self.error,"supportedModel":"CDJ-2000nexus","liveReadback":false})
    }
    fn add(
        &mut self,
        label: &str,
        notes: &str,
        settings: Vec<u8>,
        original_file: Vec<u8>,
        origin: String,
    ) -> Result<String, String> {
        let p = Profile {
            id: uid(),
            name: name(label)?,
            notes: notes.into(),
            revision: 1,
            settings,
            original_file,
            origin,
            created_at: now(),
            updated_at: now(),
        };
        let id = p.id.clone();
        let mut next = self.archive.clone();
        next.profiles.push(p);
        self.commit(next)?;
        Ok(id)
    }
    pub fn command(&mut self, body: &Value) -> Result<Value, String> {
        let id = body["id"].as_str().unwrap_or("");
        match body["action"].as_str().unwrap_or("") {
            "create" => {
                let id = self.add(
                    body["name"].as_str().unwrap_or("New profile"),
                    "",
                    defaults(),
                    vec![],
                    "OLC defaults · CDJ-2000NXS".into(),
                )?;
                return Ok(json!({"id":id,"state":self.snapshot()}));
            }
            "automatic" => {
                if body["revision"].as_u64() != Some(self.archive.revision) {
                    return Err("Configuration changed in another window. Reload before saving automatic application.".into());
                }
                let mut next = self.archive.clone();
                next.automatic =
                    serde_json::from_value(body["automatic"].clone()).map_err(|e| e.to_string())?;
                self.commit(next)?;
            }
            "update" | "duplicate" | "delete" => {
                let mut next = self.archive.clone();
                let at = next
                    .profiles
                    .iter()
                    .position(|p| p.id == id)
                    .ok_or("Profile not found")?;
                let revision = body["revision"]
                    .as_u64()
                    .ok_or("Missing profile revision")?;
                if next.profiles[at].revision != revision {
                    return Err("Profile changed in another window. Reload before saving.".into());
                }
                match body["action"].as_str().unwrap() {
                    "delete" => {
                        next.profiles.remove(at);
                        if next.automatic.default_profile.as_deref() == Some(id) {
                            next.automatic.default_profile = None;
                        }
                        next.automatic.players.retain(|_, v| v != id);
                        if next.automatic.default_profile.is_none()
                            && next.automatic.players.is_empty()
                        {
                            next.automatic.enabled = false;
                        }
                    }
                    "duplicate" => {
                        let mut p = next.profiles[at].clone();
                        p.id = uid();
                        p.name = name(body["name"].as_str().unwrap_or("Profile copy"))?;
                        p.revision = 1;
                        p.created_at = now();
                        p.updated_at = p.created_at;
                        let id = p.id.clone();
                        next.profiles.push(p);
                        self.commit(next)?;
                        return Ok(json!({"id":id,"state":self.snapshot()}));
                    }
                    _ => {
                        let p = &mut next.profiles[at];
                        p.name = name(body["name"].as_str().ok_or("Missing profile name")?)?;
                        p.notes = body["notes"].as_str().unwrap_or("").into();
                        let edits = body["values"]
                            .as_object()
                            .ok_or("Missing settings values")?;
                        for (key, value) in edits {
                            let f = schema()
                                .into_iter()
                                .find(|f| f.id == *key)
                                .ok_or("Unknown setting")?;
                            let value = value
                                .as_u64()
                                .and_then(|v| u8::try_from(v).ok())
                                .ok_or("Invalid setting value")?;
                            // Unknown imported enumerations survive unchanged; new values must be named choices.
                            if value != p.settings[f.offset]
                                && !f.options.iter().any(|o| o.value == value)
                            {
                                return Err(format!("Unsupported value for {}", f.label));
                            }
                            p.settings[f.offset] = value;
                        }
                        p.revision += 1;
                        p.updated_at = now();
                    }
                }
                self.commit(next)?;
            }
            _ => return Err("Unknown configuration action".into()),
        }
        Ok(json!({"state":self.snapshot()}))
    }
    pub fn import_dat(
        &mut self,
        label: &str,
        bytes: &[u8],
        origin: String,
    ) -> Result<Value, String> {
        if bytes.len() > 4096 {
            return Err("Settings file exceeds 4 KiB".into());
        }
        let file = SettingsFile::parse(bytes).map_err(|e| e.to_string())?;
        if file.payload.len() != 40 || !file.has_magic() || file.wire_settings().len() != 32 {
            return Err(
                "Import MYSETTING.DAT; other settings-file variants are not supported".into(),
            );
        }
        let check = file.checksum_matches();
        let id = self.add(
            label,
            "",
            file.wire_settings().to_vec(),
            bytes.to_vec(),
            origin,
        )?;
        Ok(json!({"id":id,"checksumMatches":check,"state":self.snapshot()}))
    }
    pub fn export_json(&self, id: &str) -> Result<Value, String> {
        let p = self
            .archive
            .profiles
            .iter()
            .find(|p| p.id == id)
            .ok_or("Profile not found")?;
        Ok(json!({"format":"olc-cdj-profile","version":1,"profile":p}))
    }
    pub fn import_json(&mut self, body: &Value) -> Result<Value, String> {
        if body["format"] != "olc-cdj-profile" || body["version"] != 1 {
            return Err("Unsupported profile format".into());
        }
        let p: Profile =
            serde_json::from_value(body["profile"].clone()).map_err(|e| e.to_string())?;
        let id = self.add(
            &p.name,
            &p.notes,
            p.settings,
            p.original_file,
            format!("Imported profile · {}", p.origin),
        )?;
        Ok(json!({"id":id,"state":self.snapshot()}))
    }
}

pub fn supported(model: &str) -> bool {
    model == "CDJ-2000nexus"
}
pub fn target(live: &live::Shared, number: u8, identity: &str) -> Result<Value, String> {
    crate::live::snapshot(live)["decks"]
        .as_array()
        .and_then(|ds| {
            ds.iter()
                .find(|d| d["number"] == number && d["configurationIdentity"] == identity)
        })
        .cloned()
        .ok_or_else(|| "Player connection changed. Select the player again.".into())
}
fn selected(archive: &Archive, identity: &str) -> Option<String> {
    if !archive.automatic.enabled {
        return None;
    }
    archive
        .automatic
        .players
        .get(identity)
        .cloned()
        .or_else(|| archive.automatic.default_profile.clone())
}
fn request(
    store: &Shared,
    number: u8,
    identity: &str,
    ip: &str,
    id: &str,
    revision: u64,
    automatic: bool,
) -> Result<Value, String> {
    let s = store.lock().unwrap();
    let p = s
        .archive
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or("Profile not found")?;
    if p.revision != revision {
        return Err("Profile changed; reload before pushing".into());
    }
    Ok(
        json!({"cdjConfiguration":true,"target":number,"identity":identity,"ip":ip,"profile":id,"revision":revision,"automatic":automatic}),
    )
}
pub fn is_command(body: &Value) -> bool {
    body["cdjConfiguration"] == true
}
/// Revalidated inside the live transport immediately before send, including auto opt-out.
pub struct PlayerEvidence<'a> {
    pub identity: &'a str,
    pub ip: &'a str,
    pub model: &'a str,
    pub number: u8,
    pub age: Duration,
    pub playing: bool,
    pub play_state: u8,
    pub track_loaded: bool,
}
pub fn encode(
    store: &Shared,
    body: &Value,
    player: PlayerEvidence<'_>,
    sender: u8,
    name: DeviceName,
) -> Result<Vec<u8>, String> {
    let PlayerEvidence {
        identity,
        ip,
        model,
        number,
        age,
        playing,
        play_state,
        track_loaded,
    } = player;
    if !supported(model) {
        return Err("This player model has not been validated for remote My Settings".into());
    }
    if body["identity"] != identity || body["ip"] != ip || body["target"] != number {
        return Err("Player connection changed; no settings sent".into());
    }
    if age >= Duration::from_secs(1)
        || playing
        || !(matches!(play_state, 0 | 5 | 6) || (play_state == 14 && !track_loaded))
    {
        return Err("Player must be stopped with fresh status; no settings sent".into());
    }
    let s = store.lock().unwrap();
    let id = body["profile"].as_str().ok_or("Missing profile")?;
    let p = s
        .archive
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or("Profile no longer exists")?;
    if body["revision"].as_u64() != Some(p.revision) {
        return Err("Profile changed before send; try again".into());
    }
    if body["automatic"] == true && selected(&s.archive, identity).as_deref() != Some(id) {
        return Err("Automatic application was changed or disabled; no settings sent".into());
    }
    cdj_settings_command::encode(&name.0, sender, number, &p.settings)
        .map(|p| p.to_vec())
        .map_err(str::to_owned)
}
pub fn sent(body: &Value, sender: u8) -> Value {
    json!({"outcome":"sent","message":"Settings sent. Application is not acknowledged by the player; check its screen.","profile":body["profile"],"revision":body["revision"],"target":body["target"],"identity":body["identity"],"automatic":body["automatic"],"sender":sender,"commandKind":0x34,"bytes":116})
}
pub async fn push(
    store: &Shared,
    live: &live::Shared,
    body: &Value,
    automatic: bool,
) -> Result<Value, String> {
    let number = body["target"]
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .ok_or("Select a player")?;
    let identity = body["identity"].as_str().ok_or("Missing player identity")?;
    let d = target(live, number, identity)?;
    let body = request(
        store,
        number,
        identity,
        d["ip"].as_str().ok_or("Player address unavailable")?,
        body["profile"].as_str().ok_or("Select a profile")?,
        body["revision"]
            .as_u64()
            .ok_or("Missing profile revision")?,
        automatic,
    )?;
    let result = crate::live::load(live, body).await;
    store.lock().unwrap().event(result.clone());
    Ok(result)
}
pub async fn import_player(
    store: &Shared,
    live: &live::Shared,
    body: &Value,
) -> Result<Value, String> {
    let number = body["target"]
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .ok_or("Select a player")?;
    let identity = body["identity"].as_str().ok_or("Missing player identity")?;
    let d = target(live, number, identity)?;
    if d["connection"] != "connected" {
        return Err("Player is not connected".into());
    }
    let (slot, key) = match body["slot"].as_str() {
        Some("usb") => (Slot::USB, "configurationUsb"),
        Some("sd") => (Slot::SD, "configurationSd"),
        _ => return Err("Choose USB or SD".into()),
    };
    if d[key] != true {
        return Err("No saved-settings media in that player slot".into());
    }
    let ip = d["ip"]
        .as_str()
        .ok_or("Player address unavailable")?
        .parse()
        .map_err(|_| "Invalid player address")?;
    let iface = crate::live::snapshot(live)["discovery"]["interface"]
        .as_str()
        .and_then(|name| Interface::named(name).ok());
    let bytes=tokio::time::timeout(Duration::from_secs(12),async {
        let mut nfs=NfsClient::connect(ip,iface.as_ref()).await.map_err(|e|e.to_string())?;
        let mount=nfs.mount_slot(slot).await.map_err(|e|e.to_string())?;
        let file=nfs.open(&mount,"/PIONEER/MYSETTING.DAT").await.map_err(|e|format!("Saved My Settings not found. Save My Settings to this media on the CDJ first. {e}"))?;
        if file.size()>4096 {return Err("Settings file exceeds 4 KiB".into())}
        nfs.read_file(&file).await.map_err(|e|e.to_string())
    }).await.map_err(|_|"Reading saved player settings timed out")??;
    let current = target(live, number, identity)?;
    if current["ip"] != d["ip"] || current["connection"] != "connected" {
        return Err("Player disconnected while reading; retry".into());
    }
    store.lock().unwrap().import_dat(
        body["name"].as_str().unwrap_or("Player settings"),
        &bytes,
        format!("Saved {} My Settings · player {number}", slot),
    )
}

struct Presence {
    absent: Option<Instant>,
    connected: Instant,
    pending: Option<(String, u64)>,
}
#[derive(Default)]
struct AutoEngine {
    players: BTreeMap<String, Presence>,
}
impl AutoEngine {
    fn tick(&mut self, archive: &Archive, decks: &[Value], at: Instant) -> Vec<Value> {
        let online: std::collections::BTreeSet<_> = decks
            .iter()
            .filter(|d| d["connection"] == "connected")
            .filter_map(|d| d["configurationIdentity"].as_str())
            .collect();
        self.players.retain(|id, p| {
            if online.contains(id.as_str()) {
                p.absent = None;
                true
            } else {
                at.duration_since(*p.absent.get_or_insert(at)) < Duration::from_secs(10)
            }
        });
        let mut work = vec![];
        for d in decks.iter().filter(|d| d["connection"] == "connected") {
            let Some(id) = d["configurationIdentity"].as_str() else {
                continue;
            };
            if !supported(d["name"].as_str().unwrap_or("")) {
                continue;
            }
            let p = self.players.entry(id.into()).or_insert_with(|| Presence {
                absent: None,
                connected: at,
                pending: selected(archive, id).and_then(|id| {
                    archive
                        .profiles
                        .iter()
                        .find(|p| p.id == id)
                        .map(|p| (id, p.revision))
                }),
            });
            if at.duration_since(p.connected) < Duration::from_secs(2) {
                continue;
            }
            if let Some((profile, revision)) = p.pending.take() {
                let valid = selected(archive, id).as_deref() == Some(&profile)
                    && archive
                        .profiles
                        .iter()
                        .any(|p| p.id == profile && p.revision == revision);
                if valid {
                    work.push(json!({"target":d["number"],"identity":id,"profile":profile,"revision":revision}));
                }
            }
        }
        work
    }
}
pub fn start(data_dir: &Path, live: live::Shared) -> Shared {
    let store = Arc::new(Mutex::new(Store::open(
        data_dir.join("cdj-configurations.json"),
    )));
    live.lock().unwrap().configurations = Some(store.clone());
    let weak = Arc::downgrade(&store);
    let connection = Arc::downgrade(&live);
    tokio::spawn(async move {
        let mut engine = AutoEngine::default();
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tick.tick().await;
            let (Some(store), Some(live)) = (weak.upgrade(), connection.upgrade()) else {
                break;
            };
            let snapshot = crate::live::snapshot(&live);
            let work = engine.tick(
                &store.lock().unwrap().archive,
                snapshot["decks"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                Instant::now(),
            );
            for body in work {
                if let Err(message) = push(&store, &live, &body, true).await {
                    store.lock().unwrap().event(json!({"outcome":"error","automatic":true,"target":body["target"],"message":message}));
                }
            }
        }
    });
    store
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> Store {
        Store::open(
            std::env::temp_dir()
                .join(format!("olc-cdj-test-{}", uid()))
                .join("profiles.json"),
        )
    }
    fn profile(s: &mut Store) -> String {
        s.add("Club", "", defaults(), vec![], "test".into())
            .unwrap()
    }
    fn auto(s: &mut Store, id: &str) {
        s.command(&json!({"action":"automatic","revision":s.archive.revision,"automatic":{"enabled":true,"defaultProfile":id,"players":{}}})).unwrap();
    }
    fn deck() -> Value {
        json!({"number":1,"name":"CDJ-2000nexus","connection":"connected","configurationIdentity":"mac:one"})
    }
    fn evidence() -> PlayerEvidence<'static> {
        PlayerEvidence {
            identity: "mac:one",
            ip: "192.0.2.1",
            model: "CDJ-2000nexus",
            number: 1,
            age: Duration::from_millis(50),
            playing: false,
            play_state: 5,
            track_loaded: false,
        }
    }
    fn cleanup(s: &Store) {
        std::fs::remove_dir_all(s.path.parent().unwrap()).unwrap();
    }
    #[test]
    fn saves_reopens_edits_and_preserves_unknown_bytes() {
        let mut s = store();
        let mut raw = defaults();
        raw[3] = 0x89;
        raw[19] = 0xde;
        let id = s
            .add("Imported", "", raw.clone(), vec![1, 2, 3], "USB".into())
            .unwrap();
        s.command(&json!({"action":"update","id":id,"revision":1,"name":"Edited","notes":"night","values":{"lcdBrightness":0x85,"autoCueLevel":0x89}})).unwrap();
        raw[1] = 0x85;
        let reopened = Store::open(s.path.clone());
        assert!(reopened.error.is_none());
        let p = &reopened.archive.profiles[0];
        assert_eq!(p.settings, raw);
        assert_eq!(p.original_file, vec![1, 2, 3]);
        assert_eq!(p.revision, 2);
        assert!(
            s.command(&json!({"action":"delete","id":id,"revision":1}))
                .is_err()
        );
        assert!(s.command(&json!({"action":"update","id":id,"revision":2,"name":"Bad","values":{"autoCueLevel":0x90}})).is_err());
        assert_eq!(s.archive.profiles[0].name, "Edited");
        cleanup(&s);
    }
    #[test]
    fn exported_profile_imports_with_new_identity_and_deletion_cleans_rules() {
        let mut s = store();
        let id = profile(&mut s);
        auto(&mut s, &id);
        let exported = s.export_json(&id).unwrap();
        let duplicate = s.import_json(&exported).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(id, duplicate);
        assert_eq!(s.archive.profiles.len(), 2);
        s.command(&json!({"action":"delete","id":id,"revision":1}))
            .unwrap();
        assert!(!s.archive.automatic.enabled);
        assert!(s.archive.automatic.default_profile.is_none());
        cleanup(&s);
    }
    #[test]
    fn corrupt_archive_is_preserved_and_failed_commit_keeps_memory() {
        let mut s = store();
        let id = profile(&mut s);
        std::fs::write(&s.path, b"broken").unwrap();
        let mut reopened = Store::open(s.path.clone());
        assert!(reopened.error.is_some());
        assert!(
            reopened
                .command(&json!({"action":"create","name":"New"}))
                .is_err()
        );
        assert_eq!(std::fs::read(&s.path).unwrap(), b"broken");
        s.path = s
            .path
            .parent()
            .unwrap()
            .join("missing-parent")
            .join("nested")
            .join("profiles.json");
        std::fs::write(
            s.path.parent().unwrap().parent().unwrap(),
            b"file blocks directory",
        )
        .unwrap();
        assert!(
            s.command(&json!({"action":"delete","id":id,"revision":1}))
                .is_err()
        );
        assert_eq!(s.archive.profiles.len(), 1);
        std::fs::remove_dir_all(s.path.parent().unwrap().parent().unwrap().parent().unwrap())
            .unwrap();
    }
    #[test]
    fn automatic_rules_reject_stale_windows_and_missing_profiles() {
        let mut s = store();
        let id = profile(&mut s);
        let rev = s.archive.revision;
        auto(&mut s, &id);
        assert!(s.command(&json!({"action":"automatic","revision":rev,"automatic":{"enabled":false,"defaultProfile":null,"players":{}}})).is_err());
        let mut a = s.archive.clone();
        a.automatic
            .players
            .insert("mac:two".into(), "missing".into());
        assert!(validate(&a).is_err());
        assert_eq!(selected(&s.archive, "mac:other"), Some(id));
        cleanup(&s);
    }
    #[test]
    fn send_rechecks_connection_profile_playback_and_automatic_opt_out() {
        let mut s = store();
        let id = profile(&mut s);
        auto(&mut s, &id);
        let path = s.path.clone();
        let shared = Arc::new(Mutex::new(s));
        let body = request(&shared, 1, "mac:one", "192.0.2.1", &id, 1, true).unwrap();
        let encode_with = |e| encode(&shared, &body, e, 7, DeviceName::default());
        assert_eq!(encode_with(evidence()).unwrap().len(), 116);
        let mut e = evidence();
        e.play_state = 14;
        assert!(encode_with(e).is_ok());
        let mut e = evidence();
        e.play_state = 14;
        e.track_loaded = true;
        assert!(encode_with(e).is_err());
        let mut e = evidence();
        e.playing = true;
        assert!(encode_with(e).is_err());
        let mut e = evidence();
        e.play_state = 3;
        assert!(encode_with(e).is_err());
        let mut e = evidence();
        e.age = Duration::from_secs(1);
        assert!(encode_with(e).is_err());
        let mut e = evidence();
        e.identity = "mac:replacement";
        assert!(encode_with(e).is_err());
        let mut e = evidence();
        e.ip = "192.0.2.2";
        assert!(encode_with(e).is_err());
        let mut e = evidence();
        e.model = "CDJ-3000";
        assert!(encode_with(e).is_err());
        shared.lock().unwrap().archive.automatic.enabled = false;
        assert!(encode_with(evidence()).is_err());
        let manual = request(&shared, 1, "mac:one", "192.0.2.1", &id, 1, false).unwrap();
        assert!(encode(&shared, &manual, evidence(), 15, DeviceName::default()).is_ok());
        shared.lock().unwrap().archive.profiles[0].revision += 1;
        assert!(encode(&shared, &manual, evidence(), 7, DeviceName::default()).is_err());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn automatic_runs_once_and_ignores_short_network_gaps_or_loop_delays() {
        let mut s = store();
        let id = profile(&mut s);
        auto(&mut s, &id);
        let mut engine = AutoEngine::default();
        let at = Instant::now();
        let d = deck();
        assert!(
            engine
                .tick(&s.archive, std::slice::from_ref(&d), at)
                .is_empty()
        );
        assert_eq!(
            engine
                .tick(
                    &s.archive,
                    std::slice::from_ref(&d),
                    at + Duration::from_secs(2)
                )
                .len(),
            1
        );
        assert!(
            engine
                .tick(
                    &s.archive,
                    std::slice::from_ref(&d),
                    at + Duration::from_secs(30)
                )
                .is_empty()
        );
        assert!(
            engine
                .tick(&s.archive, &[], at + Duration::from_secs(31))
                .is_empty()
        );
        assert!(
            engine
                .tick(
                    &s.archive,
                    std::slice::from_ref(&d),
                    at + Duration::from_secs(32)
                )
                .is_empty()
        );
        engine.tick(&s.archive, &[], at + Duration::from_secs(33));
        engine.tick(&s.archive, &[], at + Duration::from_secs(44));
        assert!(
            engine
                .tick(
                    &s.archive,
                    std::slice::from_ref(&d),
                    at + Duration::from_secs(45)
                )
                .is_empty()
        );
        assert_eq!(
            engine
                .tick(&s.archive, &[d], at + Duration::from_secs(47))
                .len(),
            1
        );
        cleanup(&s);
    }
    #[test]
    fn enabling_rules_does_not_push_existing_connections_and_pending_edits_cancel() {
        let mut s = store();
        let id = profile(&mut s);
        let mut e = AutoEngine::default();
        let at = Instant::now();
        let decks = [deck()];
        e.tick(&s.archive, &decks, at);
        auto(&mut s, &id);
        assert!(
            e.tick(&s.archive, &decks, at + Duration::from_secs(3))
                .is_empty()
        );
        let mut e = AutoEngine::default();
        e.tick(&s.archive, &decks, at);
        s.archive.profiles[0].revision += 1;
        assert!(
            e.tick(&s.archive, &decks, at + Duration::from_secs(3))
                .is_empty()
        );
        cleanup(&s);
    }
    #[test]
    fn saved_media_parser_checks_container_and_preserves_settings() {
        let mut s = store();
        let mut data = vec![0; 0x94];
        data[..4].copy_from_slice(&0x60u32.to_le_bytes());
        data[0x64..0x68].copy_from_slice(&40u32.to_le_bytes());
        data[0x68..0x6c].copy_from_slice(&0x12345678u32.to_le_bytes());
        data[0x6c..0x70].copy_from_slice(&1u32.to_le_bytes());
        data[0x70..0x90].copy_from_slice(&defaults());
        let r = s.import_dat("USB", &data, "saved".into()).unwrap();
        assert_eq!(r["checksumMatches"], false);
        assert_eq!(s.archive.profiles[0].original_file, data);
        assert!(s.import_dat("Bad", &[0; 4097], "bad".into()).is_err());
        assert!(s.import_dat("Bad", b"garbage", "bad".into()).is_err());
        cleanup(&s);
    }
}
