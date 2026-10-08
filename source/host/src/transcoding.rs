//! Host-owned preferences, bounded local cache, and observable conversion jobs.
use crate::{
    audio_conversion::{self, AudioInfo, ResultInfo, Target},
    local_serving::Prepared,
};
use prolink_rekordbox::Container;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    #[default]
    Auto,
    Wav16,
    Wav24,
    Aiff16,
    Aiff24,
}
impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Wav16 => "WAV 16-bit",
            Self::Wav24 => "WAV 24-bit",
            Self::Aiff16 => "AIFF 16-bit",
            Self::Aiff24 => "AIFF 24-bit",
        }
    }
    fn target(self, info: &AudioInfo) -> Target {
        Target {
            rate: match info.sample_rate {
                44100 | 48000 => info.sample_rate,
                88200 | 176400 => 44100,
                _ => 48000,
            },
            depth: match self {
                Self::Wav16 | Self::Aiff16 => 16,
                Self::Auto if info.sample_depth == 16 => 16,
                _ => 24,
            },
            aiff: matches!(self, Self::Aiff16 | Self::Aiff24),
        }
    }
}
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Settings {
    pub profile: Profile,
}
pub struct Artifact {
    pub path: PathBuf,
    pub info: ResultInfo,
}
impl Drop for Artifact {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
struct State {
    root: PathBuf,
    settings_file: PathBuf,
    settings: Settings,
    cache: BTreeMap<String, Arc<Artifact>>,
    jobs: BTreeMap<String, Value>,
    cancels: BTreeMap<String, Arc<AtomicBool>>,
    sequence: u64,
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static WORK: Mutex<()> = Mutex::new(());
fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        Mutex::new(State {
            root: std::env::temp_dir().join("olc-transcode-uninitialized"),
            settings_file: PathBuf::new(),
            settings: Settings::default(),
            cache: BTreeMap::new(),
            jobs: BTreeMap::new(),
            cancels: BTreeMap::new(),
            sequence: 0,
        })
    })
}
pub fn init(data: &std::path::Path) {
    // The host owns its data directory exclusively. Clear only our generated
    // prior-process session directories; never follow symlinks or touch USBs.
    if STATE.get().is_none() {
        let cache = data.join("transcoding-cache");
        if let Ok(entries) = std::fs::read_dir(&cache) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.split_once('-').is_some_and(|(pid, stamp)| {
                    !pid.is_empty()
                        && !stamp.is_empty()
                        && pid.bytes().all(|b| b.is_ascii_digit())
                        && stamp.bytes().all(|b| b.is_ascii_digit())
                }) && entry.file_type().is_ok_and(|t| t.is_dir())
                {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }
    let mut s = state().lock().unwrap();
    for cancel in s.cancels.values() {
        cancel.store(true, Ordering::Relaxed);
    }
    s.cancels.clear();
    s.jobs.clear();
    s.cache.clear();
    s.settings_file = data.join("transcoding.json");
    s.settings = std::fs::read(&s.settings_file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    s.root = data.join("transcoding-cache").join(format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
}
pub fn settings() -> Settings {
    state().lock().unwrap().settings.clone()
}
pub fn snapshot() -> Value {
    let s = state().lock().unwrap();
    json!({"settings":s.settings,"profiles":[{"id":"auto","label":"Automatic — preserve quality"},{"id":"wav16","label":"WAV 16-bit"},{"id":"wav24","label":"WAV 24-bit"},{"id":"aiff16","label":"AIFF 16-bit"},{"id":"aiff24","label":"AIFF 24-bit"}],"jobs":s.jobs,"cacheBytes":s.cache.values().map(|a|a.info.bytes).sum::<u64>(),"cacheLimitBytes":2u64*1024*1024*1024,"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,"engine":"Symphonia 0.6.1 / Rubato 0.16.2 / PCM v1"})
}
pub fn configure(body: &Value) -> Result<(), String> {
    let next: Settings = serde_json::from_value(body.clone()).map_err(|e| e.to_string())?;
    let mut s = state().lock().unwrap();
    let parent = s
        .settings_file
        .parent()
        .ok_or("Local configuration unavailable")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = s.settings_file.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(&next).unwrap()).map_err(|e| e.to_string())?;
    std::fs::rename(temporary, &s.settings_file).map_err(|e| e.to_string())?;
    s.settings = next;
    Ok(())
}
pub fn job_id(body: &Value) -> String {
    body["conversionJob"]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 96
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "load-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            )
        })
}
pub fn cancel(id: &str) {
    if let Some(flag) = state().lock().unwrap().cancels.get(id) {
        flag.store(true, Ordering::Relaxed);
    }
}
fn publish(id: &str, value: Value) {
    let mut s = state().lock().unwrap();
    s.jobs.insert(id.into(), value);
}
pub fn known_model(model: &str) -> bool {
    matches!(
        model
            .to_uppercase()
            .replace(|c: char| !c.is_ascii_alphanumeric(), "")
            .as_str(),
        "CDJ2000" | "CDJ2000NEXUS" | "CDJ2000NXS" | "CDJ2000NXS2" | "CDJ2000NEXUS2" | "CDJ3000"
    )
}
fn apply_info(prepared: &mut Prepared, info: &AudioInfo) {
    prepared.track.sample_rate = info.sample_rate;
    prepared.track.sample_depth = info.sample_depth as u16;
    prepared.track.container = match info.format.as_str() {
        "flac" => Container::FLAC,
        "mp3" => Container::MP3,
        "aac" | "alac" => Container::AAC,
        "wav" => Container::WAV,
        "aiff" => Container::AIFF,
        _ => prepared.track.container,
    };
}
pub fn prepare_load(
    catalogs: &crate::library::Shared,
    key: crate::local_serving::Key,
    model: &str,
    id: &str,
    cancel: Arc<AtomicBool>,
) -> Result<Prepared, String> {
    let prepared = crate::local_serving::prepare(catalogs, key)?;
    prepare_audio(prepared, model, id, cancel)
}

fn prepare_audio(
    mut prepared: Prepared,
    model: &str,
    id: &str,
    cancel: Arc<AtomicBool>,
) -> Result<Prepared, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Load cancelled; no command sent".into());
    }
    // Gate on the same exported metadata used by the UI's unsupported label.
    // Unlabelled tracks must never enter inspection, normalization or the cache.
    if crate::local_serving::compatible(&prepared.track, model).is_ok() {
        prolink::serve::diagnostics::record(format!(
            "audio_original track={} reason=no_unsupported_format_label",
            prepared.key.track
        ));
        return Ok(prepared);
    }
    let info = audio_conversion::inspect(&prepared.audio)?;
    apply_info(&mut prepared, &info);
    // Container variants are not a reason to rewrite supported source audio.
    // Use the inspected codec (ALAC and AAC share an MPEG-4 container).
    if info.format == "other" {
        return Err("Unknown audio format; conversion cannot be selected safely".into());
    }
    let unsupported = crate::local_serving::compatible_audio(
        &info.format,
        info.sample_rate,
        info.sample_depth,
        model,
    )
    .err();
    prolink::serve::diagnostics::record(format!(
        "audio_inspected track={} format={} rate={} depth={} channels={} unsupported={unsupported:?}",
        prepared.key.track, info.format, info.sample_rate, info.sample_depth, info.channels
    ));
    if unsupported.is_none() {
        return Ok(prepared);
    }
    if !known_model(model) {
        return Err(
            "Unknown player audio capabilities; conversion cannot be selected safely".into(),
        );
    }
    let profile = settings().profile;
    let artifact = run(&prepared, profile, id, false, cancel)?;
    prepared.key.variant = artifact
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    prepared.audio = artifact.path.clone();
    prepared.track.container = if matches!(profile, Profile::Aiff16 | Profile::Aiff24) {
        Container::AIFF
    } else {
        Container::WAV
    };
    prepared.track.sample_rate = artifact.info.sample_rate;
    prepared.track.sample_depth = artifact.info.sample_depth as u16;
    prepared.track.file_size = artifact.info.bytes as u32;
    prepared.track.bitrate = artifact.info.sample_rate * 2 * artifact.info.sample_depth / 1000;
    prolink::serve::diagnostics::record(format!(
        "audio_converted track={} profile={} rate={} depth={} bytes={}",
        prepared.key.track,
        profile.name(),
        artifact.info.sample_rate,
        artifact.info.sample_depth,
        artifact.info.bytes
    ));
    prepared.ui.analysis["conversion"] = json!({"profile":profile.name(),"result":artifact.info});
    prepared.converted = Some(artifact);
    crate::local_serving::compatible(&prepared.track, model)?;
    Ok(prepared)
}
struct Partial(PathBuf);
impl Drop for Partial {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
/// One worker prevents simultaneous conversions from starving audio serving.
/// A cache artifact is pinned by the serving session until it stops.
pub fn run(
    prepared: &Prepared,
    profile: Profile,
    id: &str,
    benchmark: bool,
    cancel: Arc<AtomicBool>,
) -> Result<Arc<Artifact>, String> {
    let _worker = WORK
        .try_lock()
        .map_err(|_| "Another conversion is running; wait for it to finish".to_owned())?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Conversion cancelled; no load sent".into());
    }
    let started = Instant::now();
    let info = audio_conversion::inspect(&prepared.audio)?;
    let target = profile.target(&info);
    let metadata = prepared.audio.metadata().map_err(|e| e.to_string())?;
    let stamp = metadata.modified().map_err(|e| e.to_string())?;
    let identity = format!(
        "pcm-v1:{:?}:{}:{:?}:{}:{}:{}",
        prepared.audio,
        metadata.len(),
        stamp,
        target.rate,
        target.depth,
        target.aiff
    );
    let hash = format!("{:x}", Sha256::digest(identity.as_bytes()));
    let (path, old_cache) = {
        let mut s = state().lock().unwrap();
        if !benchmark && let Some(a) = s.cache.get(&hash).filter(|a| a.path.is_file()).cloned() {
            s.jobs.insert(id.into(),json!({"state":"ready","title":prepared.track.title,"profile":profile.name(),"cacheHit":true,"seconds":0.0,"result":a.info}));
            return Ok(a);
        }
        // Only evict entries not referenced by an active serving session.
        let used = s.cache.values().map(|a| a.info.bytes).sum::<u64>();
        if used + 512 * 1024 * 1024 > 2 * 1024 * 1024 * 1024 {
            s.cache.retain(|_, a| Arc::strong_count(a) > 1);
        }
        if s.cache.values().map(|a| a.info.bytes).sum::<u64>() + 512 * 1024 * 1024
            > 2 * 1024 * 1024 * 1024
        {
            return Err("Local conversion cache is full of served tracks. Reconnect after stopping both players.".into());
        }
        while s.jobs.len() >= 32 {
            let remove = s
                .jobs
                .keys()
                .find(|key| !s.cancels.contains_key(*key))
                .cloned();
            if let Some(key) = remove {
                s.jobs.remove(&key);
            } else {
                break;
            }
        }
        std::fs::create_dir_all(&s.root).map_err(|e| e.to_string())?;
        s.sequence += 1;
        let path = s.root.join(format!(
            "{hash}-{}.{}",
            s.sequence,
            if target.aiff { "aiff" } else { "wav" }
        ));
        (path, s.cache.get(&hash).cloned())
    };
    let partial = Partial(path.with_extension("partial"));
    let mut last = Instant::now() - std::time::Duration::from_secs(1);
    let title = prepared.track.title.clone();
    let result = (|| {
        let result = audio_conversion::convert(
            &prepared.audio,
            &partial.0,
            target,
            &|| cancel.load(Ordering::Relaxed),
            &mut |fraction| {
                if last.elapsed().as_millis() >= 150 || fraction >= 1.0 {
                    publish(
                        id,
                        json!({"state":"transcoding","title":title,"profile":profile.name(),"progress":fraction,"seconds":started.elapsed().as_secs_f64(),"benchmark":benchmark}),
                    );
                    last = Instant::now();
                }
            },
        )?;
        let now = prepared.audio.metadata().map_err(|e| e.to_string())?;
        if now.len() != metadata.len() || now.modified().ok() != Some(stamp) {
            return Err("Source changed during conversion; no load sent".into());
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("Conversion cancelled; no load sent".into());
        }
        std::fs::rename(&partial.0, &path).map_err(|e| e.to_string())?;
        Ok(Arc::new(Artifact { path, info: result }))
    })();
    let mut s = state().lock().unwrap();
    s.cancels.remove(id);
    match &result {
        Ok(a) => {
            s.jobs.insert(id.into(),json!({"state":"ready","title":title,"profile":profile.name(),"cacheHit":false,"seconds":started.elapsed().as_secs_f64(),"result":a.info,"benchmark":benchmark}));
            // Benchmarks always do real work; do not displace a served cache entry.
            if !benchmark {
                s.cache.insert(hash, a.clone());
            }
        }
        Err(e) => {
            s.jobs.insert(
                id.into(),
                json!({"state":"error","title":title,"message":e,"benchmark":benchmark}),
            );
        }
    }
    drop(old_cache);
    result
}

// Dropping a waiting HTTP/load future cancels its worker cooperatively.
pub struct Cancellation {
    id: String,
    pub flag: Arc<AtomicBool>,
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        self.flag.store(true, Ordering::Relaxed);
        let mut s = state().lock().unwrap();
        s.cancels.remove(&self.id);
    }
}
pub fn register(id: &str) -> Result<Cancellation, String> {
    let mut s = state().lock().unwrap();
    if s.cancels.contains_key(id) {
        return Err("Conversion request already active".into());
    }
    let flag = Arc::new(AtomicBool::new(false));
    s.cancels.insert(id.into(), flag.clone());
    Ok(Cancellation {
        id: id.into(),
        flag,
    })
}
pub fn benchmark_error(id: &str, error: &str) {
    publish(
        id,
        json!({"state":"error","benchmark":true,"message":error}),
    );
}

pub fn benchmark_queued(id: &str) {
    publish(id, json!({"state":"queued","benchmark":true}));
}

pub fn load_finished(id: &str, result: &Value) -> Value {
    let mut s = state().lock().unwrap();
    if let Some(job) = s.jobs.get_mut(id) {
        job["loadSeconds"] = result["elapsedSeconds"].clone();
        job["loadOutcome"] = result["outcome"].clone();
        job.clone()
    } else {
        Value::Null
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_sources_keep_original_bytes_path_analysis_and_no_conversion_job() {
        let root = std::env::temp_dir().join(format!(
            "olc-original-load-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let wav = root.join("reference.wav");
        std::fs::write(
            &wav,
            include_bytes!("../tests/fixtures/transcoding/reference.wav"),
        )
        .unwrap();
        let aiff = root.join("classic.aiff");
        audio_conversion::convert(
            &wav,
            &aiff,
            Target {
                rate: 44100,
                depth: 16,
                aiff: true,
            },
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        let classic = std::fs::read(&aiff).unwrap();
        for code in [*b"NONE", *b"sowt"] {
            let mut bytes = b"FORM\0\0\0\0AIFCFVER\0\0\0\x04\xa2\x80\x51\x40COMM".to_vec();
            bytes.extend(24u32.to_be_bytes());
            bytes.extend(&classic[20..38]);
            bytes.extend(code);
            bytes.extend([0, 0]);
            bytes.extend(&classic[38..54]);
            for sample in classic[54..].as_chunks::<2>().0 {
                if code == *b"sowt" {
                    bytes.extend(sample.iter().rev());
                } else {
                    bytes.extend(sample);
                }
            }
            let len = bytes.len() as u32 - 8;
            bytes[4..8].copy_from_slice(&len.to_be_bytes());
            std::fs::write(
                root.join(format!(
                    "Isolée-{}.aiff",
                    std::str::from_utf8(&code).unwrap()
                )),
                bytes,
            )
            .unwrap();
        }
        std::fs::write(
            root.join("source.flac"),
            include_bytes!("../tests/fixtures/transcoding/reference.flac"),
        )
        .unwrap();
        std::fs::write(
            root.join("source.m4a"),
            include_bytes!("../tests/fixtures/transcoding/reference.m4a"),
        )
        .unwrap();
        std::fs::write(
            root.join("uninspectable.wav"),
            b"original bytes: do not inspect or transform",
        )
        .unwrap();
        for (name, model, container) in [
            ("uninspectable.wav", "CDJ-2000nexus", Container::WAV),
            ("reference.wav", "CDJ-2000nexus", Container::WAV),
            ("classic.aiff", "CDJ-2000nexus", Container::AIFF),
            ("Isolée-NONE.aiff", "CDJ-2000nexus", Container::AIFF),
            ("Isolée-sowt.aiff", "CDJ-2000nexus", Container::AIFF),
            ("source.flac", "CDJ-2000NXS2", Container::FLAC),
            ("source.m4a", "CDJ-3000", Container::AAC),
        ] {
            let path = root.join(name);
            let before = std::fs::read(&path).unwrap();
            let analysis = Arc::new(prolink::serve::medium::Analysis::default());
            let prepared = Prepared {
                usb: Default::default(),
                key: crate::local_serving::Key {
                    source: "local-usb:test".into(),
                    generation: 1,
                    track: 1162,
                    variant: String::new(),
                },
                track: prolink_rekordbox::Track {
                    id: 1162,
                    container,
                    file_size: before.len() as u32,
                    sample_rate: 44100,
                    sample_depth: 16,
                    ..Default::default()
                },
                audio: path.clone(),
                converted: None,
                analysis: analysis.clone(),
                artwork: vec![],
                ui: crate::live::Asset {
                    artwork: None,
                    analysis: json!({}),
                    beats: vec![],
                    warning: None,
                },
            };
            let original_track = prepared.track.clone();
            let result = prepare_audio(
                prepared,
                model,
                "supported-source-regression",
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
            assert_eq!(result.audio, path, "{name}");
            assert_eq!(result.track, original_track);
            assert_eq!(result.track.file_size as usize, before.len());
            assert!(result.key.variant.is_empty());
            assert!(result.converted.is_none());
            assert!(Arc::ptr_eq(&result.analysis, &analysis));
            assert_eq!(std::fs::read(path).unwrap(), before);
            assert!(
                !state()
                    .lock()
                    .unwrap()
                    .jobs
                    .contains_key("supported-source-regression")
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn conversion_eligibility_is_target_format_rate_and_depth_support() {
        for model in ["CDJ-2000nexus", "CDJ-2000NXS2", "CDJ-3000"] {
            for format in ["wav", "aiff", "mp3", "aac", "flac", "alac"] {
                let unsupported =
                    crate::local_serving::compatible_audio(format, 44100, 16, model).is_err();
                assert_eq!(
                    unsupported,
                    model == "CDJ-2000nexus" && matches!(format, "flac" | "alac"),
                    "{model}: {format}"
                );
            }
        }
        assert!(
            crate::local_serving::compatible_audio("aiff", 96000, 24, "CDJ-2000nexus").is_err()
        );
        assert!(crate::local_serving::compatible_audio("aiff", 96000, 24, "CDJ-2000NXS2").is_ok());
        assert!(crate::local_serving::compatible_audio("wav", 44100, 32, "CDJ-3000").is_err());
    }

    #[test]
    fn profiles_preserve_supported_rate_and_depth_and_reduce_high_rates() {
        let mut input = AudioInfo {
            format: "flac".into(),
            sample_rate: 44100,
            sample_depth: 16,
            channels: 2,
            frames: None,
        };
        let target = Profile::Auto.target(&input);
        assert_eq!((target.rate, target.depth), (44100, 16));
        input.sample_rate = 96000;
        input.sample_depth = 24;
        let target = Profile::Auto.target(&input);
        assert_eq!((target.rate, target.depth), (48000, 24));
        input.sample_rate = 88200;
        let target = Profile::Aiff16.target(&input);
        assert_eq!((target.rate, target.depth, target.aiff), (44100, 16, true));
        assert!(serde_json::from_value::<Settings>(json!({"profile":"mp3"})).is_err());
        assert!(known_model("CDJ-2000"));
        assert!(!known_model("CDJ-2000X"));
        assert!(!known_model("CDJ-3000X"));
    }
    #[test]
    fn cached_audio_is_deleted_only_after_its_last_serving_lease() {
        let path = std::env::temp_dir().join(format!("olc-lease-test-{}", std::process::id()));
        std::fs::write(&path, b"audio").unwrap();
        let artifact = Arc::new(Artifact {
            path: path.clone(),
            info: ResultInfo {
                input: AudioInfo {
                    format: "flac".into(),
                    sample_rate: 44100,
                    sample_depth: 24,
                    channels: 2,
                    frames: None,
                },
                sample_rate: 44100,
                sample_depth: 24,
                frames: 0,
                bytes: 5,
                clipped_samples: 0,
            },
        });
        let serving = artifact.clone();
        drop(artifact);
        assert!(path.exists());
        drop(serving);
        assert!(!path.exists());
    }
}
