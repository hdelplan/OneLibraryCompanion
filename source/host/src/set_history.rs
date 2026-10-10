//! Host-owned, versioned set history. UI and native shells share this model.
//! Original qualifying play events are immutable; published order is editable.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub type Shared = Arc<Mutex<Store>>;
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn uid(prefix: &str) -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{prefix}-{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: u32,
    pub title: String,
    pub artist: String,
    pub key: String,
    pub rating: u8,
    pub artwork: Option<String>,
    #[serde(default)]
    pub file_path: String,
}
impl Track {
    pub fn from_library(t: &prolink_rekordbox::Track) -> Self {
        Self {
            id: t.id,
            title: t.title.clone(),
            artist: t.artist.clone(),
            key: t.key.clone(),
            rating: t.rating.min(5),
            artwork: None,
            file_path: t.file_path.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(default)]
    pub played_at: Option<u64>,
    pub id: String,
    pub deck: Option<u8>,
    pub source: String,
    pub track: Track,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Set {
    pub id: String,
    pub title: String,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub date_only: bool,
    pub location: String,
    pub comment: String,
    pub origin: String,
    pub source_id: Option<String>,
    pub recovered: bool,
    pub events: Vec<Event>,
    pub order: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Archive {
    version: u32,
    revision: u64,
    sets: Vec<Set>,
    active_id: Option<String>,
    import_note: String,
    #[serde(default)]
    played_tracks: BTreeMap<String, Track>,
    #[serde(default)]
    last_playback_at: Option<u64>,
}
impl Default for Archive {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            sets: vec![],
            active_id: None,
            import_note: String::new(),
            played_tracks: BTreeMap::new(),
            last_playback_at: None,
        }
    }
}
#[derive(Clone)]
pub struct Sample {
    pub deck: u8,
    pub identity: String,
    pub playing: bool,
    pub track: Track,
}
struct Run {
    identity: String,
    since: u64,
    last: u64,
    event_id: Option<String>,
}
pub struct Store {
    dir: PathBuf,
    archive: Archive,
    runs: BTreeMap<u8, Run>,
    played_runs: BTreeMap<u8, (String, u64, u64)>,
    recording: bool,
    dirty: bool,
    error: Option<String>,
    blocked: bool,
    automatic: bool,
    idle_since: Option<u64>,
    interruption_since: Option<u64>,
    suppressed: BTreeMap<u8, String>,
}
impl Store {
    pub fn open(dir: PathBuf) -> Self {
        let mut s = Self {
            dir,
            archive: Archive::default(),
            runs: BTreeMap::new(),
            played_runs: BTreeMap::new(),
            recording: false,
            dirty: false,
            error: None,
            blocked: false,
            automatic: false,
            idle_since: None,
            interruption_since: None,
            suppressed: BTreeMap::new(),
        };
        match std::fs::read(s.dir.join("history.json")) {
            Ok(bytes) => match serde_json::from_slice::<Archive>(&bytes) {
                Ok(mut a) if a.version == 1 && valid_archive(&a) => {
                    if let Some(id) = &a.active_id
                        && let Some(set) = a.sets.iter_mut().find(|s| &s.id == id)
                    {
                        set.recovered = true;
                    }
                    s.archive = a;
                }
                _ => {
                    s.blocked = true;
                    s.error = Some("History file is invalid or uses an unsupported version. Original file preserved.".into());
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                s.blocked = true;
                s.error = Some(format!("Cannot read history: {e}"));
            }
        }
        s
    }
    pub fn ordered_tracks(&self, id: &str) -> Result<Vec<Track>, String> {
        let set = self
            .archive
            .sets
            .iter()
            .find(|set| set.id == id)
            .ok_or("Set not found")?;
        let events: BTreeMap<_, _> = set
            .events
            .iter()
            .map(|event| (&event.id, &event.track))
            .collect();
        Ok(set
            .order
            .iter()
            .filter_map(|id| events.get(id).map(|track| (*track).clone()))
            .collect())
    }

    pub fn snapshot(&self) -> Value {
        json!({"version":1,"revision":self.archive.revision,"sets":self.archive.sets,"activeId":self.archive.active_id,"recording":self.recording,"error":self.error,"importNote":self.archive.import_note,
            "playedTracks":self.archive.played_tracks.values().collect::<Vec<_>>(),
            "observationInterrupted":self.interruption_since.is_some() && self.archive.active_id.is_some(),
            "pending":self.runs.iter().filter(|(_,r)|r.event_id.is_none()).map(|(deck,r)|json!({"deck":deck,"seconds":r.last.saturating_sub(r.since)/1000})).collect::<Vec<_>>()})
    }
    fn changed(&mut self) {
        self.archive.revision += 1;
        self.dirty = true;
    }
    pub fn flush(&mut self) -> Result<(), String> {
        if self.blocked {
            return Err(self.error.clone().unwrap_or_default());
        }
        if !self.dirty {
            return Ok(());
        }
        let result = (|| {
            std::fs::create_dir_all(&self.dir)?;
            let bytes = serde_json::to_vec_pretty(&self.archive)?;
            let temp = self.dir.join("history.json.tmp");
            let mut f = std::fs::File::create(&temp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            std::fs::rename(temp, self.dir.join("history.json"))?;
            std::fs::File::open(&self.dir)?.sync_all()?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })();
        match result {
            Ok(()) => {
                self.dirty = false;
                self.error = None;
                Ok(())
            }
            Err(e) => {
                let msg =
                    format!("History not saved: {e}. Keep the host running and export a copy.");
                self.error = Some(msg.clone());
                Err(msg)
            }
        }
    }
    pub fn command(&mut self, body: &Value) -> Result<(), String> {
        if self.blocked {
            return Err(self.error.clone().unwrap_or_default());
        }
        let action = body["action"].as_str().ok_or("Missing action")?;
        if action == "new" {
            self.finish_automatic(false);
            self.suppressed.clear();
            self.idle_since = None;
            return self.command(&json!({"action":"start"}));
        }
        if action == "merge" {
            let id = body["id"].as_str().ok_or("Missing set")?;
            let previous = body["previousId"].as_str().ok_or("Missing previous set")?;
            if id == previous || self.archive.active_id.as_deref() == Some(previous) {
                return Err("Choose a different finished previous set".into());
            }
            let target = self
                .archive
                .sets
                .iter()
                .position(|s| s.id == id)
                .ok_or("Set not found")?;
            let source = self
                .archive
                .sets
                .iter()
                .position(|s| s.id == previous)
                .ok_or("Previous set not found")?;
            let a = &self.archive.sets[target];
            let b = &self.archive.sets[source];
            if b.started_at > a.started_at
                || self.archive.sets.iter().any(|s| {
                    s.id != id
                        && s.id != previous
                        && s.started_at > b.started_at
                        && s.started_at < a.started_at
                })
            {
                return Err("Only adjacent sessions can be merged".into());
            }
            if a.origin == "imported"
                || b.origin == "imported"
                || a.origin == "sample"
                || b.origin == "sample"
            {
                return Err("Only captured sessions can be merged".into());
            }
            let earlier = self.archive.sets.remove(source);
            let set = self.archive.sets.iter_mut().find(|s| s.id == id).unwrap();
            set.started_at = earlier.started_at;
            let mut events = earlier.events;
            events.append(&mut set.events);
            set.events = events;
            let mut order = earlier.order;
            order.append(&mut set.order);
            set.order = order;
            if !earlier.comment.is_empty() {
                set.comment = format!("{}\n{}", earlier.comment, set.comment);
            }
            if set.location.is_empty() {
                set.location = earlier.location;
            }
            self.changed();
            return self.flush();
        }
        if action == "clearPlayed" {
            self.archive.played_tracks.clear();
            self.played_runs.clear();
            self.changed();
            return self.flush();
        }
        if action == "cancel" || action == "delete" {
            let id = body["id"].as_str().ok_or("Missing set")?;
            let is_active = self.archive.active_id.as_deref() == Some(id);
            if action == "cancel" && !is_active {
                return Err("Only the current set can be canceled".into());
            }
            let set = self
                .archive
                .sets
                .iter()
                .find(|s| s.id == id)
                .ok_or("Set not found")?;
            // Check original events, including tracks removed from the published list.
            // This also covers a track qualifying after the browser's last snapshot.
            if action == "delete" && body["confirmed"].as_bool() != Some(true) {
                return Err("DELETE_CONFIRMATION_REQUIRED".into());
            }
            if !set.events.is_empty() && body["confirmed"].as_bool() != Some(true) {
                return Err("CANCEL_CONFIRMATION_REQUIRED".into());
            }
            let images: Vec<_> = set
                .events
                .iter()
                .filter_map(|e| e.track.artwork.clone())
                .collect();
            let previous = self.archive.clone();
            self.archive.sets.retain(|s| s.id != id);
            if is_active {
                self.archive.active_id = None;
            }
            self.changed();
            if let Err(error) = self.flush() {
                self.archive = previous;
                self.dirty = true;
                return Err(error);
            }
            if is_active {
                self.suppressed = self
                    .runs
                    .iter()
                    .map(|(deck, run)| (*deck, run.identity.clone()))
                    .collect();
                self.recording = false;
                self.runs.clear();
            }
            for name in images {
                if let Some(path) = self.artwork_path(&name) {
                    let _ = std::fs::remove_file(path);
                }
            }
            return Ok(());
        }
        if action == "start" {
            if self.archive.active_id.is_some() {
                return Err("Finish the current set first".into());
            }
            let id = uid("set");
            self.archive.sets.push(Set {
                id: id.clone(),
                title: "Live set".into(),
                started_at: now(),
                ended_at: None,
                date_only: false,
                location: String::new(),
                comment: String::new(),
                origin: "recorded".into(),
                source_id: None,
                recovered: false,
                events: vec![],
                order: vec![],
            });
            self.archive.active_id = Some(id);
            self.recording = true;
            self.runs.clear();
        } else {
            let id = body["id"].as_str().ok_or("Missing set")?;
            let active = self.archive.active_id.as_deref() == Some(id);
            let set = self
                .archive
                .sets
                .iter_mut()
                .find(|s| s.id == id)
                .ok_or("Set not found")?;
            match action {
                "resume" if active => {
                    self.recording = true;
                    set.recovered = false;
                    self.runs.clear();
                }
                "finish" if active => {
                    set.ended_at = Some(
                        self.archive
                            .last_playback_at
                            .unwrap_or_else(now)
                            .max(set.started_at),
                    );
                    self.suppressed = self
                        .runs
                        .iter()
                        .map(|(deck, run)| (*deck, run.identity.clone()))
                        .collect();
                    set.recovered = false;
                    self.archive.active_id = None;
                    self.recording = false;
                    self.runs.clear();
                }
                "metadata" => {
                    for (key, max) in [("location", 200), ("comment", 4000), ("title", 200)] {
                        if let Some(value) = body.get(key)
                            && value.as_str().is_none_or(|v| v.chars().count() > max)
                        {
                            return Err(format!("Invalid or oversized {key}"));
                        }
                    }
                    for (key, target, max) in [
                        ("location", &mut set.location, 200),
                        ("comment", &mut set.comment, 4000),
                        ("title", &mut set.title, 200),
                    ] {
                        if let Some(v) = body[key].as_str() {
                            if v.chars().count() > max {
                                return Err(format!("{key} is too long"));
                            }
                            *target = v.to_owned();
                        }
                    }
                }
                "move" | "remove" => {
                    let entry = body["entry"].as_str().ok_or("Missing track entry")?;
                    let index = set
                        .order
                        .iter()
                        .position(|e| e == entry)
                        .ok_or("Entry no longer exists")?;
                    if action == "remove" {
                        set.order.remove(index);
                    } else {
                        let delta = body["delta"]
                            .as_i64()
                            .filter(|d| *d == -1 || *d == 1)
                            .ok_or("Invalid move")?;
                        let to = index as i64 + delta;
                        if to >= 0 && (to as usize) < set.order.len() {
                            set.order.swap(index, to as usize);
                        }
                    }
                }
                "restore" => {
                    set.order = set.events.iter().map(|e| e.id.clone()).collect();
                }
                _ => return Err("Action unavailable for this set".into()),
            }
        }
        self.changed();
        self.flush()
    }
    fn finish_automatic(&mut self, interrupted: bool) {
        if let Some(id) = self.archive.active_id.take() {
            if let Some(set) = self.archive.sets.iter_mut().find(|s| s.id == id) {
                set.ended_at = Some(
                    self.archive
                        .last_playback_at
                        .unwrap_or(set.started_at)
                        .max(set.started_at),
                );
                set.recovered = false;
                if interrupted {
                    set.comment.push_str("\nPlayback observation was interrupted; the session boundary is uncertain.");
                }
            }
            self.archive
                .sets
                .retain(|s| s.id != id || !s.events.is_empty());
            self.recording = false;
            self.runs.clear();
            self.changed();
        }
    }

    /// Host-owned capture. Silence is measured from playback activity, never track changes.
    pub fn observe_automatic(
        &mut self,
        samples: &[Sample],
        clock: u64,
    ) -> Vec<(String, String, u8, String)> {
        if self.blocked {
            return vec![];
        }
        if !self.automatic
            && self.archive.active_id.is_some()
            && !self.recording
            && self
                .archive
                .last_playback_at
                .is_some_and(|last| now().saturating_sub(last) > 300_000)
        {
            self.finish_automatic(true);
        }
        self.automatic = true;
        self.suppressed.retain(|deck, identity| {
            samples
                .iter()
                .any(|s| s.deck == *deck && s.playing && s.identity == *identity)
        });
        let samples: Vec<_> = samples
            .iter()
            .filter(|s| self.suppressed.get(&s.deck) != Some(&s.identity))
            .cloned()
            .collect();
        if samples.is_empty() && self.archive.active_id.is_some() {
            self.interruption_since.get_or_insert(clock);
            // Preserve qualified entries through a status outage, but reset auditions.
            self.runs.retain(|_, run| run.event_id.is_some());
        } else if !samples.is_empty()
            && let Some(since) = self.interruption_since.take()
        {
            if clock.saturating_sub(since) > 2000
                && let Some(set) = self
                    .archive
                    .sets
                    .iter_mut()
                    .find(|s| Some(&s.id) == self.archive.active_id.as_ref())
            {
                let note =
                    "Playback observation was interrupted; playback during the gap is unavailable.";
                if !set.comment.contains(note) {
                    set.comment.push('\n');
                    set.comment.push_str(note);
                    self.changed();
                }
            }
            for sample in &samples {
                if let Some(run) = self.runs.get_mut(&sample.deck)
                    && run.identity == sample.identity
                    && run.event_id.is_some()
                {
                    run.last = clock;
                }
            }
        }
        let playing = samples.iter().any(|s| s.playing);
        if playing {
            self.idle_since = None;
            let wall = now();
            if self
                .archive
                .last_playback_at
                .is_none_or(|last| wall.saturating_sub(last) >= 1000)
            {
                self.archive.last_playback_at = Some(wall);
                self.changed();
            }
            if self.archive.active_id.is_none() {
                let _ = self.command(&json!({"action":"start"}));
                if let Some(id) = &self.archive.active_id {
                    let set = self.archive.sets.iter_mut().find(|s| &s.id == id).unwrap();
                    set.title = "Automatic set".into();
                    set.origin = "automatic".into();
                    self.changed();
                }
            } else if !self.recording {
                self.recording = true;
                let set = self
                    .archive
                    .sets
                    .iter_mut()
                    .find(|s| Some(&s.id) == self.archive.active_id.as_ref())
                    .unwrap();
                set.recovered = false;
                set.comment.push_str("\nCapture resumed after a host restart; playback during the interruption is unavailable.");
                // Do not count the same still-loaded playback twice after a restart.
                for sample in samples.iter().filter(|s| s.playing) {
                    if let Some(event) = set
                        .events
                        .iter()
                        .rev()
                        .find(|e| e.deck == Some(sample.deck))
                        && event.source == sample.identity
                    {
                        self.runs.insert(
                            sample.deck,
                            Run {
                                identity: sample.identity.clone(),
                                since: clock,
                                last: clock,
                                event_id: Some(event.id.clone()),
                            },
                        );
                    }
                }
                self.changed();
            }
        } else if self.archive.active_id.is_some() {
            let since = *self.idle_since.get_or_insert(clock);
            if clock.saturating_sub(since) > 300_000 {
                self.finish_automatic(samples.is_empty());
                self.idle_since = None;
            }
        }
        if samples.is_empty() {
            self.observe_played(&samples, clock);
            return vec![];
        }
        self.observe(&samples, clock)
    }

    // Qualification uses monotonic elapsed time, separately from stored wall-clock timestamps.
    fn observe_played(&mut self, samples: &[Sample], clock: u64) {
        self.played_runs
            .retain(|deck, _| samples.iter().any(|s| s.deck == *deck && s.playing));
        for sample in samples
            .iter()
            .filter(|s| s.playing && !s.identity.is_empty())
        {
            let run = self
                .played_runs
                .entry(sample.deck)
                .or_insert_with(|| (sample.identity.clone(), clock, clock));
            if run.0 != sample.identity || clock.saturating_sub(run.2) > 2000 {
                *run = (sample.identity.clone(), clock, clock);
            }
            run.2 = clock;
            if clock.saturating_sub(run.1) > 45_000 && !sample.track.file_path.is_empty() {
                let key = serde_json::to_string(&(
                    &sample.track.file_path,
                    sample.track.id,
                    &sample.track.title,
                    &sample.track.artist,
                ))
                .unwrap();
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    self.archive.played_tracks.entry(key)
                {
                    entry.insert(sample.track.clone());
                    self.changed();
                }
            }
        }
    }
    pub fn observe(&mut self, samples: &[Sample], clock: u64) -> Vec<(String, String, u8, String)> {
        self.observe_played(samples, clock);
        if !self.recording || self.blocked {
            self.runs.clear();
            return vec![];
        }
        let Some(set_id) = self.archive.active_id.clone() else {
            return vec![];
        };
        self.runs.retain(|deck, run| {
            samples.iter().any(|s| {
                s.deck == *deck
                    && (s.playing
                        || (self.automatic
                            && run.event_id.is_some()
                            && clock.saturating_sub(run.last) <= 15_000))
            })
        });
        let mut qualified = Vec::new();
        for sample in samples
            .iter()
            .filter(|s| s.playing && !s.identity.is_empty())
        {
            let reset = self.runs.get(&sample.deck).is_none_or(|r| {
                r.identity != sample.identity
                    || clock.saturating_sub(r.last)
                        > if self.automatic && r.event_id.is_some() {
                            15_000
                        } else {
                            2000
                        }
            });
            if reset {
                self.runs.insert(
                    sample.deck,
                    Run {
                        identity: sample.identity.clone(),
                        since: clock,
                        last: clock,
                        event_id: None,
                    },
                );
            }
            let run = self.runs.get_mut(&sample.deck).unwrap();
            run.last = clock;
            if run.event_id.is_none() && clock.saturating_sub(run.since) > 45_000 {
                let id = uid("play");
                run.event_id = Some(id.clone());
                qualified.push((run.since, sample.deck, id, sample.clone()));
            }
        }
        qualified.sort_by_key(|(since, deck, _, _)| (*since, *deck));
        let mut artwork = vec![];
        if !qualified.is_empty() {
            let set = self
                .archive
                .sets
                .iter_mut()
                .find(|s| s.id == set_id)
                .unwrap();
            for (started, deck, id, sample) in qualified {
                if set.events.is_empty() && set.origin == "automatic" {
                    set.started_at = now().saturating_sub(clock.saturating_sub(started));
                }
                set.events.push(Event {
                    played_at: Some(now().saturating_sub(clock.saturating_sub(started))),
                    id: id.clone(),
                    deck: Some(deck),
                    source: sample.identity.clone(),
                    track: sample.track,
                });
                set.order.push(id.clone());
                artwork.push((set_id.clone(), id, deck, sample.identity));
            }
            self.changed();
        }
        // Metadata may arrive after qualification; retain the event even if it never arrives.
        let set = self
            .archive
            .sets
            .iter_mut()
            .find(|s| s.id == set_id)
            .unwrap();
        let mut enriched = false;
        for sample in samples {
            if let Some(run) = self.runs.get(&sample.deck)
                && let Some(event_id) = &run.event_id
                && let Some(event) = set.events.iter_mut().find(|e| &e.id == event_id)
            {
                if event.track.title.starts_with("Track #")
                    && !sample.track.title.starts_with("Track #")
                {
                    let art = event.track.artwork.clone();
                    event.track = sample.track.clone();
                    event.track.artwork = art;
                    enriched = true;
                }
                if event.track.artwork.is_none() {
                    artwork.push((
                        set_id.clone(),
                        event_id.clone(),
                        sample.deck,
                        sample.identity.clone(),
                    ));
                }
            }
        }
        if enriched {
            self.changed();
        }
        artwork
    }
    pub fn save_artwork(
        &mut self,
        set_id: &str,
        event_id: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        if bytes.len() > 1024 * 1024 || !matches!(mime, "image/jpeg" | "image/png") {
            return Ok(());
        }
        let Some(event) = self
            .archive
            .sets
            .iter_mut()
            .find(|s| s.id == set_id)
            .and_then(|s| s.events.iter_mut().find(|e| e.id == event_id))
        else {
            return Ok(());
        };
        if event.track.artwork.is_some() {
            return Ok(());
        }
        let name = format!(
            "{}-{}.{}",
            set_id,
            event_id,
            if mime == "image/png" { "png" } else { "jpg" }
        );
        let dir = self.dir.join("artwork");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(&name), bytes).map_err(|e| e.to_string())?;
        event.track.artwork = Some(name);
        self.changed();
        Ok(())
    }
    pub fn artwork_path(&self, name: &str) -> Option<PathBuf> {
        (!name.is_empty()
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            && !name.contains(".."))
        .then(|| self.dir.join("artwork").join(name))
    }
    pub fn import(
        &mut self,
        library: &prolink_rekordbox::Library,
    ) -> Result<Vec<(String, String, u32)>, String> {
        if self.blocked {
            return Err(self.error.clone().unwrap_or_default());
        }
        let (sets, note) = super::set_history_import::prepare(library)?;
        let mut artwork = vec![];
        for set in sets {
            if self
                .archive
                .sets
                .iter()
                .any(|s| s.source_id == set.source_id)
            {
                continue;
            }
            artwork.extend(
                set.events
                    .iter()
                    .map(|e| (set.id.clone(), e.id.clone(), e.track.id)),
            );
            self.archive.sets.push(set);
        }
        self.archive.import_note = note;
        self.changed();
        self.flush()?;
        Ok(artwork)
    }
}
fn valid_archive(a: &Archive) -> bool {
    let ids: BTreeSet<_> = a.sets.iter().map(|s| &s.id).collect();
    ids.len() == a.sets.len()
        && a.active_id.as_ref().is_none_or(|id| ids.contains(id))
        && a.sets.iter().all(|s| {
            let events: BTreeSet<_> = s.events.iter().map(|e| &e.id).collect();
            events.len() == s.events.len()
                && s.order.iter().all(|id| events.contains(id))
                && s.order.iter().collect::<BTreeSet<_>>().len() == s.order.len()
                && s.events.iter().all(|e| e.track.rating <= 5)
        })
}
pub fn start(dir: PathBuf, live: crate::live::Shared) -> Shared {
    let shared = Arc::new(Mutex::new(Store::open(dir)));
    let state = shared.clone();
    tokio::spawn(async move {
        let clock = Instant::now();
        let mut ticks = tokio::time::interval(Duration::from_millis(100));
        loop {
            ticks.tick().await;
            let samples = crate::live::set_samples(&live);
            let state = state.clone();
            let live = live.clone();
            let elapsed = clock.elapsed().as_millis() as u64;
            let _ = tokio::task::spawn_blocking(move || {
                let mut store = state.lock().unwrap();
                let pending = store.observe_automatic(&samples, elapsed);
                for (set, event, deck, key) in pending {
                    if let Some((mime, bytes)) = crate::live::artwork(&live, deck, &key) {
                        let _ = store.save_artwork(&set, &event, mime, &bytes);
                    }
                }
                let _ = store.flush();
            })
            .await;
        }
    });
    shared
}
pub fn default_directory(root: &Path) -> PathBuf {
    crate::distribution::env("DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".local/app-data"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> Store {
        Store::open(std::env::temp_dir().join(uid("pc-history-test")))
    }
    fn sample(deck: u8, identity: &str, playing: bool) -> Sample {
        Sample {
            deck,
            identity: identity.into(),
            playing,
            track: Track {
                file_path: String::new(),
                id: 1,
                title: "Test title".into(),
                artist: "Artist".into(),
                key: "8A".into(),
                rating: 4,
                artwork: None,
            },
        }
    }
    fn active(s: &Store) -> &Set {
        s.archive
            .sets
            .iter()
            .find(|a| Some(&a.id) == s.archive.active_id.as_ref())
            .unwrap()
    }
    fn seconds(s: &mut Store, samples: &[Sample], from: u64, to: u64) {
        for second in from..=to {
            s.observe(samples, second * 1000);
        }
    }
    fn auto_seconds(s: &mut Store, samples: &[Sample], from: u64, to: u64) {
        for second in from..=to {
            s.observe_automatic(samples, second * 1000);
        }
    }
    #[test]
    fn automatic_long_track_never_splits_and_saves_after_actual_inactivity() {
        let mut s = store();
        auto_seconds(&mut s, &[sample(1, "a", true)], 0, 900);
        assert_eq!(s.archive.sets.len(), 1);
        assert_eq!(active(&s).events.len(), 1);
        assert!(active(&s).events[0].played_at.is_some());
        auto_seconds(&mut s, &[sample(1, "a", false)], 901, 1201);
        assert!(s.archive.active_id.is_some());
        s.observe_automatic(&[sample(1, "a", false)], 1_201_001);
        assert!(s.archive.active_id.is_none());
        assert_eq!(s.archive.sets[0].ended_at, s.archive.last_playback_at);
        assert!(s.archive.sets[0].comment.is_empty());
    }
    #[test]
    fn automatic_any_deck_playback_resets_idle_and_auditions_do_not_save_empty_sets() {
        let mut s = store();
        auto_seconds(&mut s, &[sample(1, "a", true)], 0, 46);
        auto_seconds(&mut s, &[sample(1, "a", false)], 47, 300);
        auto_seconds(&mut s, &[sample(2, "b", true)], 301, 400);
        assert_eq!(active(&s).events.len(), 2);
        auto_seconds(&mut s, &[], 401, 702);
        assert!(s.archive.active_id.is_none());
        assert!(s.archive.sets[0].comment.contains("interrupted"));
        auto_seconds(&mut s, &[sample(1, "c", true)], 703, 720);
        auto_seconds(&mut s, &[], 721, 1022);
        assert_eq!(s.archive.sets.len(), 1);
    }
    #[test]
    fn automatic_boundaries_join_and_short_pause_preserve_tracks() {
        let mut s = store();
        auto_seconds(&mut s, &[sample(1, "a", true)], 0, 60);
        auto_seconds(&mut s, &[sample(1, "a", false)], 61, 65);
        auto_seconds(&mut s, &[sample(1, "a", true)], 66, 120);
        assert_eq!(active(&s).events.len(), 1);
        let previous = active(&s).id.clone();
        s.command(&json!({"action":"new"})).unwrap();
        auto_seconds(&mut s, &[sample(2, "b", true)], 121, 180);
        let current = active(&s).id.clone();
        s.command(&json!({"action":"merge", "id":current,"previousId":previous}))
            .unwrap();
        assert_eq!(s.archive.sets.len(), 1);
        assert_eq!(active(&s).events.len(), 2);
        assert_eq!(active(&s).events[0].source, "a");
        s.command(&json!({"action":"finish","id":current})).unwrap();
        auto_seconds(&mut s, &[sample(2, "b", true)], 181, 300);
        assert!(s.archive.active_id.is_none());
        auto_seconds(&mut s, &[sample(2, "c", true)], 301, 350);
        assert_eq!(active(&s).events.len(), 1);
    }
    #[test]
    fn automatic_restart_resumes_without_duplicate_and_long_outage_closes() {
        let mut s = store();
        auto_seconds(&mut s, &[sample(1, "a", true)], 0, 60);
        s.flush().unwrap();
        let mut reopened = Store::open(s.dir.clone());
        auto_seconds(&mut reopened, &[sample(1, "a", true)], 0, 60);
        assert_eq!(active(&reopened).events.len(), 1);
        assert!(active(&reopened).comment.contains("restart"));
        reopened.archive.last_playback_at = Some(now() - 400_000);
        reopened.changed();
        reopened.flush().unwrap();
        let mut later = Store::open(s.dir.clone());
        auto_seconds(&mut later, &[sample(1, "b", true)], 0, 60);
        assert_eq!(later.archive.sets.len(), 2);
        assert!(later.archive.sets[0].ended_at.is_some());
        assert!(later.archive.sets[0].comment.contains("interrupted"));
    }
    #[test]
    fn automatic_short_outage_preserves_entry_and_notes_uncertain_gap() {
        let mut s = store();
        auto_seconds(&mut s, &[sample(1, "a", true)], 0, 60);
        auto_seconds(&mut s, &[], 61, 80);
        assert_eq!(s.snapshot()["observationInterrupted"], true);
        auto_seconds(&mut s, &[sample(1, "a", true)], 81, 150);
        assert_eq!(active(&s).events.len(), 1);
        assert!(active(&s).comment.contains("gap is unavailable"));
        assert_eq!(s.snapshot()["observationInterrupted"], false);
        auto_seconds(&mut s, &[sample(1, "b", true)], 151, 200);
        auto_seconds(&mut s, &[sample(1, "a", true)], 201, 250);
        assert_eq!(active(&s).events.len(), 3);
    }
    #[test]
    fn automatic_pause_before_qualification_resets_continuous_timer() {
        let mut s = store();
        auto_seconds(&mut s, &[sample(1, "a", true)], 0, 40);
        s.observe_automatic(&[sample(1, "a", false)], 41_000);
        auto_seconds(&mut s, &[sample(1, "a", true)], 42, 82);
        assert!(active(&s).events.is_empty());
        auto_seconds(&mut s, &[sample(1, "a", true)], 83, 88);
        assert_eq!(active(&s).events.len(), 1);
    }
    #[test]
    fn played_tracks_qualify_without_recording_and_persist_until_cleared() {
        let mut s = store();
        let mut track = sample(1, "a", true);
        track.track.file_path = "/Contents/a.mp3".into();
        seconds(&mut s, &[track.clone()], 0, 45);
        assert!(s.archive.played_tracks.is_empty());
        s.observe(&[track.clone()], 45_001);
        assert_eq!(s.archive.played_tracks.len(), 1);
        assert!(s.archive.sets.is_empty());
        let revision = s.archive.revision;
        seconds(&mut s, &[track], 46, 60);
        assert_eq!(s.archive.revision, revision);
        s.flush().unwrap();
        let mut reopened = Store::open(s.dir.clone());
        assert_eq!(reopened.archive.played_tracks.len(), 1);
        reopened.command(&json!({"action":"clearPlayed"})).unwrap();
        assert!(Store::open(s.dir.clone()).archive.played_tracks.is_empty());
    }

    #[test]
    fn played_tracks_reset_qualification_after_pause_or_connection_gap() {
        let mut s = store();
        let mut track = sample(1, "a", true);
        track.track.file_path = "/Contents/a.mp3".into();
        seconds(&mut s, &[track.clone()], 0, 40);
        s.observe(&[], 41_000);
        seconds(&mut s, &[track.clone()], 42, 82);
        assert!(s.archive.played_tracks.is_empty());
        s.observe(&[track.clone()], 90_000);
        seconds(&mut s, &[track], 91, 135);
        assert!(s.archive.played_tracks.is_empty());
    }

    #[test]
    fn strictly_more_than_45_seconds_and_once_per_run() {
        let mut s = store();
        s.command(&json!({"action":"start"})).unwrap();
        seconds(&mut s, &[sample(1, "a", true)], 0, 45);
        assert!(active(&s).events.is_empty());
        s.observe(&[sample(1, "a", true)], 45_001);
        assert_eq!(active(&s).events.len(), 1);
        seconds(&mut s, &[sample(1, "a", true)], 46, 120);
        assert_eq!(active(&s).events.len(), 1);
        std::fs::remove_dir_all(s.dir).unwrap();
    }
    #[test]
    fn pauses_changes_and_gaps_reset_and_replays_are_preserved() {
        let mut s = store();
        s.command(&json!({"action":"start"})).unwrap();
        seconds(&mut s, &[sample(1, "a", true)], 0, 40);
        s.observe(&[sample(1, "a", false)], 41_000);
        seconds(&mut s, &[sample(1, "a", true)], 42, 80);
        assert!(active(&s).events.is_empty());
        seconds(&mut s, &[sample(1, "b", true)], 81, 125);
        assert!(active(&s).events.is_empty());
        seconds(&mut s, &[sample(1, "b", true)], 130, 175);
        assert!(active(&s).events.is_empty());
        s.observe(&[sample(1, "b", true)], 176_000);
        assert_eq!(active(&s).events.len(), 1);
        s.observe(&[], 177_000);
        seconds(&mut s, &[sample(1, "b", true)], 178, 224);
        assert_eq!(active(&s).events.len(), 2);
        assert_ne!(active(&s).events[0].id, active(&s).events[1].id);
        std::fs::remove_dir_all(s.dir).unwrap();
    }
    #[test]
    fn decks_order_edits_and_restart_preserve_original_events() {
        let mut s = store();
        s.command(&json!({"action":"start"})).unwrap();
        seconds(&mut s, &[sample(2, "b", true)], 0, 5);
        seconds(&mut s, &[sample(1, "a", true), sample(2, "b", true)], 6, 52);
        assert_eq!(
            active(&s).events.iter().map(|e| e.deck).collect::<Vec<_>>(),
            [Some(2), Some(1)]
        );
        let id = active(&s).id.clone();
        let entry = active(&s).order[0].clone();
        s.command(&json!({"action":"move","id":id,"entry":entry,"delta":1}))
            .unwrap();
        assert_eq!(active(&s).order[1], entry);
        assert_eq!(active(&s).events[0].id, entry);
        s.command(&json!({"action":"remove","id":id,"entry":entry}))
            .unwrap();
        assert_eq!(active(&s).events.len(), 2);
        assert_eq!(active(&s).order.len(), 1);
        let mut reopened = Store::open(s.dir.clone());
        assert!(!reopened.recording);
        assert!(active(&reopened).recovered);
        seconds(&mut reopened, &[sample(1, "a", true)], 0, 60);
        assert_eq!(active(&reopened).events.len(), 2);
        reopened
            .command(&json!({"action":"restore","id":id}))
            .unwrap();
        assert_eq!(active(&reopened).order.len(), 2);
        reopened
            .command(&json!({"action":"resume","id":id}))
            .unwrap();
        seconds(&mut reopened, &[sample(1, "a", true)], 61, 107);
        assert_eq!(active(&reopened).events.len(), 3);
        reopened
            .command(&json!({"action":"finish","id":id}))
            .unwrap();
        assert!(Store::open(s.dir.clone()).archive.active_id.is_none());
        std::fs::remove_dir_all(s.dir).unwrap();
    }
    #[test]
    fn corrupt_files_are_not_overwritten_and_failures_are_reported() {
        let mut s = store();
        std::fs::create_dir_all(&s.dir).unwrap();
        std::fs::write(s.dir.join("history.json"), b"bad").unwrap();
        let mut loaded = Store::open(s.dir.clone());
        assert!(loaded.command(&json!({"action":"start"})).is_err());
        assert_eq!(std::fs::read(s.dir.join("history.json")).unwrap(), b"bad");
        std::fs::remove_dir_all(&s.dir).unwrap();
        std::fs::write(&s.dir, b"not a directory").unwrap();
        assert!(s.command(&json!({"action":"start"})).is_err());
        assert!(s.snapshot()["error"].is_string());
        std::fs::remove_file(s.dir).unwrap();
    }
    #[test]
    fn bad_edits_do_not_remove_original_events() {
        let mut s = store();
        s.command(&json!({"action":"start"})).unwrap();
        let id = active(&s).id.clone();
        assert!(s.command(&json!({"action":"start"})).is_err());
        assert!(
            s.command(&json!({"action":"remove","id":id,"entry":"missing"}))
                .is_err()
        );
        assert!(s.artwork_path("../history.json").is_none());
        std::fs::remove_dir_all(s.dir).unwrap();
    }
    #[test]
    fn deleting_saved_set_requires_confirmation_and_preserves_recording() {
        let mut s = store();
        s.command(&json!({"action":"start"})).unwrap();
        let past = active(&s).id.clone();
        s.command(&json!({"action":"finish","id":past})).unwrap();
        s.command(&json!({"action":"start"})).unwrap();
        let current = active(&s).id.clone();
        assert!(s.command(&json!({"action":"delete","id":past})).is_err());
        assert_eq!(s.archive.sets.len(), 2);
        s.command(&json!({"action":"delete","id":past,"confirmed":true}))
            .unwrap();
        assert!(s.recording);
        assert_eq!(active(&s).id, current);
        let loaded = Store::open(s.dir.clone());
        assert_eq!(loaded.archive.sets.len(), 1);
        assert_eq!(loaded.archive.active_id.as_deref(), Some(current.as_str()));
        s.command(&json!({"action":"delete","id":current,"confirmed":true}))
            .unwrap();
        assert!(!s.recording);
        assert!(s.archive.active_id.is_none());
        assert!(Store::open(s.dir.clone()).archive.sets.is_empty());
        std::fs::remove_dir_all(s.dir).unwrap();
    }
    #[test]
    fn empty_current_set_cancels_immediately_and_stays_canceled_after_restart() {
        let mut s = store();
        s.command(&json!({"action":"start"})).unwrap();
        let id = active(&s).id.clone();
        s.command(&json!({"action":"cancel","id":id})).unwrap();
        assert!(s.archive.active_id.is_none());
        assert!(!s.recording);
        assert!(s.archive.sets.is_empty());
        assert!(Store::open(s.dir.clone()).archive.sets.is_empty());
        s.command(&json!({"action":"start"})).unwrap();
        assert!(s.recording);
        std::fs::remove_dir_all(s.dir).unwrap();
    }
    #[test]
    fn cancellation_requires_confirmation_for_original_events_and_rejects_past_sets() {
        let mut s = store();
        s.command(&json!({"action":"start"})).unwrap();
        let past = active(&s).id.clone();
        s.command(&json!({"action":"finish","id":past})).unwrap();
        s.command(&json!({"action":"start"})).unwrap();
        let id = active(&s).id.clone();
        seconds(&mut s, &[sample(1, "a", true)], 0, 46);
        let entry = active(&s).order[0].clone();
        s.command(&json!({"action":"remove","id":id,"entry":entry}))
            .unwrap();
        assert!(active(&s).order.is_empty());
        assert_eq!(
            s.command(&json!({"action":"cancel","id":id})).unwrap_err(),
            "CANCEL_CONFIRMATION_REQUIRED"
        );
        assert!(s.recording);
        assert!(
            s.command(&json!({"action":"cancel","id":past,"confirmed":true}))
                .is_err()
        );
        s.command(&json!({"action":"cancel","id":id,"confirmed":true}))
            .unwrap();
        assert_eq!(s.archive.sets.len(), 1);
        assert_eq!(s.archive.sets[0].id, past);
        assert!(Store::open(s.dir.clone()).archive.active_id.is_none());
        std::fs::remove_dir_all(s.dir).unwrap();
    }
}
