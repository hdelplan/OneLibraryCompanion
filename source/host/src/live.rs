//! Observes players, reads USB analysis, and handles explicit library load requests.
use prolink::consume::nfs::NfsClient;
use prolink::monitor::LoadedTrack;
use prolink::{Discovery, Interface, Monitor, Slot, VirtualCdj, VirtualCdjConfig};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    net::Ipv4Addr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[path = "direct_live.rs"]
pub mod direct;
pub struct LiveState {
    direct_task: Option<tokio::task::JoinHandle<()>>,
    direct_controls: Option<tokio::sync::mpsc::Sender<direct::Control>>,
    direct_peers: Vec<Value>,
    pub enabled: bool,
    pub error: Option<String>,
    pub decks: Vec<Value>,
    assets: BTreeMap<String, Asset>,
    last_update: Instant,
    updates: tokio::sync::watch::Sender<Value>,
    loader: Option<crate::loading::Sender>,
}
impl Default for LiveState {
    fn default() -> Self {
        Self {
            direct_task: None,
            direct_controls: None,
            direct_peers: vec![],
            enabled: false,
            error: None,
            decks: vec![],
            assets: BTreeMap::new(),
            last_update: Instant::now(),
            loader: None,
            updates: tokio::sync::watch::channel(json!({"enabled":false,"error":null,"decks":[]}))
                .0,
        }
    }
}
fn qualifies_for_set(status: prolink::monitor::PlayerStatus) -> bool {
    status.is_playing && matches!(status.play_state.0, 3 | 4)
}
fn play_state_label(status: prolink::monitor::PlayerStatus) -> &'static str {
    if status.play_state.0 == 4 && !status.is_playing {
        "paused"
    } else {
        status.play_state.name().unwrap_or("unknown")
    }
}
pub fn subscribe(shared: &Shared) -> tokio::sync::watch::Receiver<Value> {
    shared.lock().unwrap().updates.subscribe()
}
fn publish(state: &LiveState) {
    crate::jog_trace::snapshot(&state.decks);
    state
        .updates
        .send_replace(json!({"enabled":state.enabled,"error":state.error,"decks":state.decks,"directPeers":state.direct_peers}));
}
#[derive(Clone)]
pub(crate) struct Asset {
    pub(crate) artwork: Option<(&'static str, Vec<u8>)>,
    pub(crate) analysis: Value,
    pub(crate) beats: Vec<f64>,
    pub(crate) warning: Option<String>,
}
pub type Shared = Arc<Mutex<LiveState>>;

pub fn start(interface: Option<String>, catalogs: crate::library::Shared) -> Shared {
    let shared = Arc::new(Mutex::new(LiveState {
        enabled: interface.is_some(),
        ..LiveState::default()
    }));
    publish(&shared.lock().unwrap());
    if let Some(name) = interface {
        let state = shared.clone();
        tokio::spawn(async move {
            if let Err(error) = observe(&name, state.clone(), catalogs.clone()).await {
                crate::library::sync_sources(&catalogs, vec![]);
                let mut state = state.lock().unwrap();
                state.loader = None;
                state.error = Some(error);
                state.decks.clear();
                publish(&state);
            }
        });
    }
    shared
}
pub async fn load(shared: &Shared, body: Value) -> Value {
    // A local first load includes file preparation and source discovery before
    // the normal 12-second CDJ confirmation window even starts.
    let wait = if body["source"]
        .as_str()
        .is_some_and(|s| s.starts_with("local-usb:"))
    {
        Duration::from_secs(40)
    } else {
        Duration::from_secs(16)
    };
    let sender = shared.lock().unwrap().loader.clone();
    let Some(sender) = sender else {
        return json!({"outcome":"error","message":"Live CDJ connection is not active"});
    };
    let (reply, receiver) = tokio::sync::oneshot::channel();
    if sender
        .try_send(crate::loading::Command { body, reply })
        .is_err()
    {
        return json!({"outcome":"error","message":"Load queue unavailable or busy"});
    }
    match tokio::time::timeout(wait, receiver).await {
        Ok(Ok(result)) => result,
        _ => {
            json!({"outcome":"unknown","message":"Load outcome unknown. Check the CDJ before trying again; no automatic retry was sent."})
        }
    }
}
pub fn snapshot(shared: &Shared) -> Value {
    let state = shared.lock().unwrap();
    json!({"enabled":state.enabled,"error":state.error,"decks":state.decks,"directPeers":state.direct_peers})
}
pub fn analysis(shared: &Shared, number: u8) -> Option<Value> {
    let state = shared.lock().unwrap();
    let deck = state
        .decks
        .iter()
        .find(|d| d["number"] == number && d["connection"] == "connected")?;
    let key = deck["trackKey"].as_str()?;
    let asset = state.assets.get(key)?;
    let mut analysis = asset.analysis.clone();
    if asset.artwork.is_some() {
        analysis["artworkAvailable"] = json!(true);
    }
    Some(json!({"key":key,"analysis":analysis,"beatGridSeconds":asset.beats}))
}

pub fn artwork(shared: &Shared, number: u8, key: &str) -> Option<(&'static str, Vec<u8>)> {
    let state = shared.lock().unwrap();
    let deck = state.decks.iter().find(|d| {
        d["number"] == number
            && d["connection"] == "connected"
            && d["trackKey"].as_str() == Some(key)
    })?;
    state
        .assets
        .get(deck["trackKey"].as_str()?)?
        .artwork
        .clone()
}

async fn observe(
    name: &str,
    shared: Shared,
    catalogs: crate::library::Shared,
) -> Result<(), String> {
    let observation_epoch = Instant::now();
    let interface = Interface::named(name).map_err(|e| e.to_string())?;
    let discovery = Discovery::start(interface.clone())
        .await
        .map_err(|e| e.to_string())?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let observer = VirtualCdj::observe(
        &discovery,
        VirtualCdjConfig {
            emit_status: false,
            ..VirtualCdjConfig::default()
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    let monitor = Monitor::with_status(interface.clone(), &observer)
        .await
        .map_err(|e| e.to_string())?;
    let (loader, mut commands) = crate::loading::channel();
    shared.lock().unwrap().loader = Some(loader);
    let mut loading = crate::loading::Controller::default();
    let mut identities: BTreeMap<u8, (String, u64)> = BTreeMap::new();
    let mut current_cues: BTreeMap<String, f64> = BTreeMap::new();
    let mut loop_regions: BTreeMap<u8, crate::loop_region::LoopRegion> = BTreeMap::new();
    let mut beat_positions: BTreeMap<u8, crate::beat_position::BeatPosition> = BTreeMap::new();
    let mut bar_positions: BTreeMap<u8, crate::bar_position::BarPosition> = BTreeMap::new();
    let session = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let mut requests: BTreeMap<String, Instant> = BTreeMap::new();
    let mut events = monitor.subscribe();
    let mut clock = tokio::time::interval(Duration::from_millis(250));
    loop {
        let command = tokio::select! { _ = clock.tick() => None, _ = events.recv() => None, command = commands.recv() => command };
        let devices = discovery.devices();
        let mut mounted = Vec::new();
        for device in devices.iter().filter(|d| d.number.get() <= 6 && !d.offline) {
            if let Some(observation) = monitor.player(device.number).and_then(|p| p.status)
                && observation.age < Duration::from_secs(2)
            {
                for (slot, present) in [
                    (Slot::USB, observation.status.usb_present),
                    (Slot::SD, observation.status.sd_present),
                ] {
                    if present {
                        mounted.push((
                            format!("{}-{}", device.mac, slot),
                            format!("CDJ{} {}", device.number, slot.to_string().to_uppercase()),
                            crate::library::Location::Remote(device.ip, slot),
                        ));
                    }
                }
            }
        }
        crate::library::sync_sources(&catalogs, mounted);
        loading.tick(&discovery, &monitor, &catalogs);
        if let Some(command) = command {
            loading
                .accept(
                    command,
                    &discovery,
                    &monitor,
                    &catalogs,
                    observer.number().get(),
                )
                .await;
        }
        let mut decks = Vec::new();
        for device in devices.iter().filter(|d| d.number.get() <= 6) {
            let number = device.number.get();
            let player = monitor.player(device.number);
            let observed = player.as_ref().and_then(|p| p.status);
            let fresh = !device.offline && observed.is_some_and(|s| s.age < Duration::from_secs(2));
            let status = observed.filter(|_| fresh).map(|s| s.status);
            let track = status.and_then(|s| s.track);
            // Epoch prevents an old asynchronous result from attaching after reload/reconnect.
            let identity = observed
                .and_then(|s| s.status.track)
                .map(|t| format!("{}:{}:{}:{}", device.mac, t.source_player, t.slot, t.id))
                .unwrap_or_default();
            let entry = identities.entry(number).or_insert((String::new(), 0));
            if entry.0 != identity {
                entry.0 = identity.clone();
                entry.1 += 1;
            }
            let key = track.map(|_| format!("{session}:{number}:{}:{identity}", entry.1));
            let peer = track
                .and_then(|t| {
                    devices
                        .iter()
                        .find(|d| d.number == t.source_player && !d.offline)
                })
                .map(|d| d.ip);
            let state = shared.lock().unwrap();
            let asset = key.as_ref().and_then(|k| state.assets.get(k));
            if !fresh {
                bar_positions.remove(&number);
                beat_positions.remove(&number);
            }
            let fine_position = status.and_then(|s| {
                bar_positions.entry(number).or_default().position(
                    key.as_deref()?,
                    s,
                    &asset?.beats,
                    observed?.received_at,
                    Instant::now(),
                )
            });
            if !fresh {
                loop_regions.remove(&number);
            }
            let loop_region = status.and_then(|mut s| {
                if fine_position.is_none_or(|p| p.held) {
                    s.bar_position = None;
                }
                loop_regions.entry(number).or_default().observe(
                    key.as_deref()?,
                    s,
                    player.as_ref().and_then(|p| p.beat),
                    &asset?.beats,
                    observed?.received_at,
                )
            });
            let tracked = status.and_then(|s| {
                let tracker = beat_positions.entry(number).or_default();
                let region = (s.play_state.0 == 4)
                    .then_some(loop_region.as_ref())
                    .flatten()
                    .and_then(|r| Some((r["start"].as_f64()?, r["end"].as_f64()?)));
                tracker.configure_loop(key.as_deref()?, region, &asset?.beats);
                tracker.observe(
                    key.as_deref()?,
                    s,
                    observed?.received_at,
                    player
                        .as_ref()
                        .and_then(|p| p.beat)
                        .map(|b| (b.beat, b.received_at)),
                    &asset?.beats,
                    Instant::now(),
                )
            });
            let has_beat_phase = player.as_ref().and_then(|p| p.beat_phase()).is_some();
            let saved_cue = status.and_then(|s| {
                crate::cues::saved_position(
                    s,
                    &asset?.beats,
                    asset?.analysis["cues"]
                        .as_array()
                        .map_or(&[], |c| c.as_slice()),
                )
            });
            let mut position = tracked
                .map(|p| p.seconds)
                .or_else(|| fine_position.map(|p| p.seconds))
                .or(saved_cue)
                .or_else(|| {
                    status.and_then(|s| {
                        // Beat zero is the pre-grid/start region, not an unknown
                        // beat. Its conservative display anchor is track time zero.
                        if s.beat_number == Some(0) && !asset?.beats.is_empty() {
                            return Some(0.0);
                        }
                        let mut index = usize::try_from(s.beat_number?.checked_sub(1)?).ok()?;
                        let beats = &asset?.beats;
                        // The player reports a beat index, not sample-accurate position.
                        let mut phase = if s.is_playing && !s.reverse {
                            player.as_ref()?.beat_phase().unwrap_or(0.0)
                        } else {
                            0.0
                        };
                        if s.is_playing
                            && !s.reverse
                            && let (Some(status_bar), Some(beat), Some(status_obs)) =
                                (s.beat_in_bar, player.as_ref()?.beat, observed)
                            && let Some(beat_bar) = beat.beat.beat_in_bar
                            && !beat.is_stale()
                        {
                            (index, phase) = align_beat(
                                index,
                                phase,
                                status_bar,
                                beat_bar.index() + 1,
                                beat.age < status_obs.age,
                            );
                        }
                        let base = *beats.get(index)?;
                        let next = beats.get(index + 1).copied().unwrap_or(base);
                        Some(base + phase * (next - base).max(0.0))
                    })
                });
            if status.is_some_and(|s| s.play_state.0 == 6)
                && let (Some(key), Some(position)) = (
                    &key,
                    fine_position
                        .map(|p| p.seconds)
                        .or(position.filter(|_| status.is_some_and(|s| s.beat_number == Some(0)))),
                )
            {
                current_cues.insert(key.clone(), position);
            }
            if current_cues.len() > 24 {
                current_cues.retain(|k, _| key.as_ref() == Some(k));
            }
            let current_cue = key.as_ref().and_then(|k| current_cues.get(k)).copied();
            let ready = asset.is_some();
            if status.is_some_and(|s| s.play_state.0 == 4 && s.is_playing)
                && let (Some(value), Some(region)) = (position, &loop_region)
                && let (Some(start), Some(end)) = (region["start"].as_f64(), region["end"].as_f64())
                && end > start
                && value >= end
            {
                position = Some(start + (value - start).rem_euclid(end - start));
            }
            let warning = asset.and_then(|a| a.warning.clone());
            decks.push(json!({"number":number,"name":device.name.as_str(),"ip":device.ip.to_string(),
                "connection":if device.offline {"disconnected"} else if fresh {"connected"} else {"stale"},
                "beatNumber":status.and_then(|s|s.beat_number),
                "loop":loop_region,"currentCue":current_cue,
                "reverse":status.is_some_and(|s|s.reverse),
                "playing":status.is_some_and(|s|s.is_playing),
                "loadProtected":status.is_some_and(|s|s.is_playing || matches!(s.play_state.0, 3 | 4 | 7 | 8 | 9 | 18)),
                "manualMotion":tracked.is_none() && status.is_some_and(|s|s.position_requires_direct_updates()),
                "positionAgeMs":tracked.map(|p|p.at).or_else(||fine_position.and_then(|_|bar_positions.get(&number)?.observed_at())).map(|at|at.elapsed().as_secs_f64()*1000.0),"motionRate":tracked.map(|p|p.rate),"beatAnchorNumber":tracked.map(|p|p.beat_number),"beatCorrectionMs":tracked.map(|p|p.correction_seconds*1000.0),"beatArrivalResidualMs":tracked.map(|p|p.arrival_residual_seconds*1000.0),
                "statusAgeMs":observed.map(|s|s.received_at.elapsed().as_secs_f64()*1000.0),
                "observationId":observed.map(|s|format!("{session}:{}:{}",s.received_at.duration_since(observation_epoch).as_nanos(),tracked.map_or(0,|p|p.at.duration_since(observation_epoch).as_nanos()))),
                "observationTimeMs":observed.map(|s|s.received_at.duration_since(observation_epoch).as_secs_f64()*1000.0),
                "packetCounter":observed.and_then(|s|s.packet_counter),
                "positionSource":if tracked.is_some() {"beat-motion"} else if fine_position.is_some() {"bar-phase"} else if has_beat_phase {"beat-grid-estimate"} else {"status-beat-estimate"},
                "positionQuality":if tracked.is_some() {"beat"} else {fine_position.map_or(if position.is_some() {"coarse"} else {"unavailable"}, |p|p.quality())},
                "playState":status.map(play_state_label),
                "bpm":status.and_then(|s|s.effective_bpm()),"pitch":status.and_then(|s|s.pitch).map(|p|(p.multiplier()-1.0)*100.0),
                "master":status.map(|s|s.is_tempo_master),"sync":status.map(|s|s.is_synced),
                "trackId":track.map(|t|t.id),"qualifyingPlayback":status.is_some_and(qualifies_for_set),
                "trackKey":key,"assetReady":ready,"position":position,"warning":warning,
                "sourceLabel":track.map(|t|format!("CDJ{} {}",t.source_player,t.slot.to_string().to_uppercase()))}));
            if !ready
                && let (Some(key), Some(track), Some(peer)) = (key, track, peer)
                && requests
                    .get(&key)
                    .is_none_or(|last| last.elapsed() > Duration::from_secs(45))
            {
                requests.insert(key.clone(), Instant::now());
                let shared = shared.clone();
                let interface = interface.clone();
                let catalogs = catalogs.clone();
                tokio::spawn(async move {
                    let result = tokio::time::timeout(
                        Duration::from_secs(35),
                        fetch_asset(peer, track, Some(&interface), &catalogs),
                    )
                    .await;
                    let asset = match result {
                        Ok(Ok(asset)) => asset,
                        Ok(Err(error)) => Asset {
                            artwork: None,
                            analysis: json!({"detail":null,"preview":null,"track":null}),
                            beats: vec![],
                            warning: Some(error),
                        },
                        Err(_) => Asset {
                            artwork: None,
                            analysis: json!({"detail":null,"preview":null,"track":null}),
                            beats: vec![],
                            warning: Some("USB read timed out; reload the track to retry".into()),
                        },
                    };
                    shared.lock().unwrap().assets.insert(key, asset);
                });
            }
        }
        let mut state = shared.lock().unwrap();
        // Prune only after collecting every deck, so a later lane keeps its asset.
        if state.assets.len() > 24 {
            state
                .assets
                .retain(|k, _| decks.iter().any(|d| d["trackKey"].as_str() == Some(k)));
        }
        requests.retain(|key, _| decks.iter().any(|d| d["trackKey"].as_str() == Some(key)));
        state.last_update = Instant::now();
        state.decks = decks;
        publish(&state);
        let _keep_alive = &observer;
    }
}

async fn fetch_asset(
    peer: Ipv4Addr,
    track: LoadedTrack,
    interface: Option<&Interface>,
    catalogs: &crate::library::Shared,
) -> Result<Asset, String> {
    if track.slot != Slot::USB && track.slot != Slot::SD {
        return Err("Analysis retrieval currently supports USB and SD sources".into());
    }
    let mut client = NfsClient::connect(peer, interface)
        .await
        .map_err(|e| e.to_string())?;
    let mount = client
        .mount_slot(track.slot)
        .await
        .map_err(|e| e.to_string())?;
    let t = if let Some(track) = crate::library::cached_track(catalogs, peer, track.slot, track.id)
    {
        track
    } else {
        let file = client
            .open(&mount, "/PIONEER/rekordbox/export.pdb")
            .await
            .map_err(|e| e.to_string())?;
        if file.size() > 64 * 1024 * 1024 {
            return Err("Database exceeds 64 MiB limit".into());
        }
        let bytes = client.read_file(&file).await.map_err(|e| e.to_string())?;
        let library =
            tokio::task::spawn_blocking(move || prolink_rekordbox::Library::parse(&bytes))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        library
            .tracks
            .get(&track.id)
            .cloned()
            .ok_or("Loaded track is absent from the source library")?
    };
    let mut metadata = crate::library::metadata(&t);
    metadata["myTags"] = crate::library::cached_tags(catalogs, peer, track.slot, track.id);
    let mut beats = vec![];
    let mut beat_marks = vec![];
    let mut cues = vec![];
    if let Ok(file) = client.open(&mount, &t.analyze_path).await
        && let Ok(bytes) = client.read_file(&file).await
        && let Ok(anlz) = prolink_rekordbox::AnlzFile::parse(&bytes)
    {
        crate::cues::collect(&anlz, &mut cues);
        if let Some(grid) = anlz.beat_grid() {
            beat_marks = grid
                .beats
                .iter()
                .map(|b| json!({"time": f64::from(b.time) / 1000.0, "beatInBar": b.beat_number}))
                .collect::<Vec<_>>();
            beats = grid
                .beats
                .iter()
                .map(|b| f64::from(b.time) / 1000.0)
                .collect();
        }
    }
    let path = std::path::Path::new(&t.analyze_path)
        .with_extension("2EX")
        .to_string_lossy()
        .into_owned();
    let wave = async {
        let file = client
            .open(&mount, &path)
            .await
            .map_err(|e| e.to_string())?;
        let bytes = client.read_file(&file).await.map_err(|e| e.to_string())?;
        pioneer_companion_core::decode(&bytes)
    }
    .await;
    let (mut analysis, warning) = match wave {
        Ok(wave) => (serde_json::to_value(wave).map_err(|e| e.to_string())?, None),
        Err(_) => (
            json!({"detail":null,"preview":null}),
            Some("Native three-band analysis unavailable for this track".into()),
        ),
    };
    let phrase_path = std::path::Path::new(&t.analyze_path).with_extension("EXT");
    let mut phrases = vec![];
    let mut phrase_mood = None;
    let phrase_result = async {
        let file = client
            .open(&mount, &phrase_path.to_string_lossy())
            .await
            .map_err(|e| format!("Phrase file unavailable: {e}"))?;
        let bytes = client
            .read_file(&file)
            .await
            .map_err(|e| format!("Phrase file transfer failed: {e}"))?;
        let anlz = prolink_rekordbox::AnlzFile::parse(&bytes)
            .map_err(|e| format!("Phrase file could not be decoded: {e}"))?;
        crate::cues::collect(&anlz, &mut cues);
        let structure = anlz.song_structure().ok_or_else(|| {
            if anlz.tag(prolink_rekordbox::anlz::FourCc::PSSI).is_some() {
                "Phrase data could not be decoded".to_owned()
            } else {
                "No phrase analysis exported for this track".to_owned()
            }
        })?;
        phrase_mood = crate::phrases::mood_label(structure.mood);
        phrases = crate::phrases::segments(structure, &beats, f64::from(t.duration));
        if phrases.is_empty() {
            return Err("Phrase analysis has no usable beat-grid boundaries".to_owned());
        }
        Ok::<(), String>(())
    }
    .await;
    analysis["phraseMood"] = json!(phrase_mood);
    analysis["phraseWarning"] = json!(phrase_result.err());
    analysis["phrases"] = json!(phrases);
    analysis["cues"] = json!(cues);
    analysis["beats"] = json!(beat_marks);
    if metadata["myTags"].is_null() {
        let tags = tokio::time::timeout(Duration::from_secs(3), async {
            let file = client
                .open(&mount, "/PIONEER/rekordbox/exportExt.pdb")
                .await
                .ok()?;
            if file.size() > 64 * 1024 * 1024 {
                return None;
            }
            let bytes = client.read_file(&file).await.ok()?;
            prolink_rekordbox::mytags::MyTags::parse(&bytes).ok()
        })
        .await
        .ok()
        .flatten();
        metadata["myTags"] = crate::library::tag_metadata(tags.as_ref(), t.id);
    }
    analysis["track"] = metadata;
    let artwork = async {
        if t.artwork_path.is_empty() {
            return None;
        }
        let file = client.open(&mount, &t.artwork_path).await.ok()?;
        let bytes = client.read_file(&file).await.ok()?;
        if bytes.len() > 4 * 1024 * 1024 {
            return None;
        }
        let mime = if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            "image/jpeg"
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else {
            return None;
        };
        Some((mime, bytes))
    }
    .await;
    Ok(Asset {
        artwork,
        analysis,
        beats,
        warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn first_local_load_can_finish_after_old_request_deadline() {
        let (sender, mut commands) = crate::loading::channel();
        let shared = Arc::new(Mutex::new(LiveState {
            loader: Some(sender),
            ..Default::default()
        }));
        let worker = tokio::spawn(async move {
            let command = commands.recv().await.unwrap();
            // A cold USB preparation plus discovery can outlast the former
            // 16-second HTTP waiter before confirmation finishes.
            tokio::time::sleep(Duration::from_secs(17)).await;
            command
                .reply
                .send(json!({"outcome":"confirmed","message":"test"}))
                .unwrap();
        });
        let result = load(&shared, json!({"source":"local-usb:test"})).await;
        worker.await.unwrap();
        assert_eq!(result["outcome"], "confirmed");
    }

    #[tokio::test]
    async fn pushed_snapshots_deliver_latest_state_without_queueing_old_positions() {
        let shared = Arc::new(Mutex::new(LiveState::default()));
        let mut receiver = subscribe(&shared);
        {
            let mut state = shared.lock().unwrap();
            state.decks = vec![json!({"position":10})];
            publish(&state);
            state.decks = vec![json!({"position":9})];
            publish(&state);
        }
        receiver.changed().await.unwrap();
        assert_eq!(receiver.borrow_and_update()["decks"][0]["position"], 9);
    }
    #[test]
    fn old_track_result_cannot_be_served_after_track_change() {
        let shared = Arc::new(Mutex::new(LiveState::default()));
        {
            let mut state = shared.lock().unwrap();
            state.assets.insert(
                "old".into(),
                Asset {
                    artwork: None,
                    analysis: json!({"track":{"id":1}}),
                    beats: vec![],
                    warning: None,
                },
            );
            state.decks = vec![json!({"number":1,"connection":"connected","trackKey":"new"})];
        }
        assert!(analysis(&shared, 1).is_none());
    }
    #[test]
    fn stale_deck_does_not_serve_cached_analysis() {
        let shared = Arc::new(Mutex::new(LiveState::default()));
        {
            let mut state = shared.lock().unwrap();
            state.assets.insert(
                "track".into(),
                Asset {
                    artwork: None,
                    analysis: json!({"track":{"id":1}}),
                    beats: vec![],
                    warning: None,
                },
            );
            state.decks = vec![json!({"number":1,"connection":"stale","trackKey":"track"})];
        }
        assert!(analysis(&shared, 1).is_none());
        shared.lock().unwrap().decks[0]["connection"] = json!("connected");
        assert!(analysis(&shared, 1).is_some());
    }
}

// UDP beat and status packets arrive independently. A new beat pulse must not
// reset the phase of the preceding status beat, which would jump backwards.
fn align_beat(
    index: usize,
    phase: f64,
    status_bar: u8,
    pulse_bar: u8,
    _pulse_newer: bool,
) -> (usize, f64) {
    let delta = (pulse_bar + 4 - status_bar) % 4;
    match delta {
        1 => (index + 1, phase),
        3 => (index, 0.0),
        _ => (index, phase),
    }
}

#[cfg(test)]
mod transport_tests {
    #[test]
    fn independent_packet_arrival_does_not_reset_previous_beat() {
        assert_eq!(super::align_beat(3, 0.01, 4, 1, true), (4, 0.01));
        assert_eq!(super::align_beat(4, 0.98, 1, 4, false), (4, 0.0));
        assert_eq!(super::align_beat(4, 0.3, 1, 1, true), (4, 0.3));
    }
}

/// Small metadata-only snapshots for history; waveform buffers never cross this boundary.
pub(crate) fn set_samples(shared: &Shared) -> Vec<crate::set_history::Sample> {
    let state = shared.lock().unwrap();
    if state.last_update.elapsed() > Duration::from_secs(2) {
        return vec![];
    }
    state
        .decks
        .iter()
        .filter_map(|d| {
            let identity = d["trackKey"].as_str()?;
            let metadata = state.assets.get(identity).map(|a| &a.analysis["track"]);
            let id = d["trackId"].as_u64().unwrap_or(0) as u32;
            let field = |key: &str| {
                metadata
                    .and_then(|m| m[key].as_str())
                    .unwrap_or("")
                    .to_owned()
            };
            let title = field("title");
            Some(crate::set_history::Sample {
                deck: d["number"].as_u64()? as u8,
                identity: identity.into(),
                playing: d["connection"] == "connected" && d["qualifyingPlayback"] == true,
                track: crate::set_history::Track {
                    file_path: field("filePath"),
                    id,
                    title: if title.is_empty() {
                        format!("Track #{id}")
                    } else {
                        title
                    },
                    artist: field("artist"),
                    key: field("key"),
                    rating: metadata
                        .and_then(|m| m["rating"].as_u64())
                        .unwrap_or(0)
                        .min(5) as u8,
                    artwork: None,
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod set_recording_tests {
    use super::*;
    #[test]
    fn set_samples_require_fresh_transport_and_keep_metadata_without_waves() {
        let state = Arc::new(Mutex::new(LiveState::default()));
        {
            let mut live = state.lock().unwrap();
            live.decks = vec![
                json!({"number":1,"trackId":42,"trackKey":"key","connection":"connected","qualifyingPlayback":true}),
            ];
            live.assets.insert("key".into(), Asset { artwork:None, beats:vec![],warning:None,analysis:json!({"track":{"title":"Recorded","artist":"Artist","key":"8A","rating":4}}) });
        }
        let samples = set_samples(&state);
        assert!(samples[0].playing);
        assert_eq!(samples[0].track.rating, 4);
        assert_eq!(samples[0].track.title, "Recorded");
        state.lock().unwrap().decks[0]["qualifyingPlayback"] = json!(false);
        assert!(!set_samples(&state)[0].playing);
        state.lock().unwrap().last_update = Instant::now() - Duration::from_secs(3);
        assert!(set_samples(&state).is_empty());
    }
}
