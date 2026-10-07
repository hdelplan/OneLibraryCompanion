//! Read-only, generation-scoped USB catalogs. No player control or second monitor.
use prolink::{Interface, Slot, consume::nfs::NfsClient};
use prolink_rekordbox::{Library, Track, mytags::MyTags};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    net::Ipv4Addr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const MAX_DATABASE: u64 = 64 * 1024 * 1024;
pub type Shared = Arc<Mutex<Catalogs>>;
pub type Query = HashMap<String, String>;
#[derive(Clone, PartialEq, Eq)]
pub enum Location {
    Remote(Ipv4Addr, Slot),
    Direct(Ipv4Addr),
    Local(PathBuf),
}
pub struct Source {
    label: String,
    location: Location,
    available: bool,
    generation: u64,
    state: &'static str,
    error: Option<String>,
    catalog: Option<Arc<Catalog>>,
}
pub struct Catalogs {
    session: u64,
    interface: Option<String>,
    sources: BTreeMap<String, Source>,
}
pub struct Catalog {
    pub artwork: crate::artwork::Cache,
    pub library: Library,
    tags: Option<MyTags>,
    tag_error: Option<String>,
    fingerprint: String,
}
/// Browse durable set metadata without requiring any USB catalog.
pub fn saved_tracks(query: &Query, tracks: &[crate::set_history::Track]) -> Result<Value, String> {
    Catalog {
        artwork: Default::default(),
        library: Library::default(),
        tags: None,
        tag_error: None,
        fingerprint: String::new(),
    }
    .tracks_for_set(0, query, Some(tracks))
}

pub fn with_local_path(interface: Option<String>, local_path: Option<PathBuf>) -> Shared {
    let session = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let mut sources = BTreeMap::new();
    if let Some(path) = local_path {
        sources.insert(
            "local".into(),
            Source {
                label: "LOCAL EXPORT".into(),
                location: Location::Local(path),
                available: true,
                generation: session,
                state: "idle",
                error: None,
                catalog: None,
            },
        );
    }
    Arc::new(Mutex::new(Catalogs {
        session,
        interface,
        sources,
    }))
}
/// Atomically register a validated local OneLibrary catalog. Other source kinds
/// and unchanged local catalogs retain their identity and cached artwork.
pub(crate) fn sync_local(shared: &Shared, ready: Vec<(String, String, PathBuf, Option<Catalog>)>) {
    let mut state = shared.lock().unwrap();
    for (id, source) in &mut state.sources {
        if id.starts_with("local-usb:")
            && !ready.iter().any(|(key, _, _, _)| key == id)
            && source.available
        {
            source.available = false;
            source.generation += 1;
            source.catalog = None;
            source.state = "unavailable";
        }
    }
    let session = state.session;
    for (id, label, path, catalog) in ready {
        let source = state.sources.entry(id).or_insert_with(|| Source {
            label: label.clone(),
            location: Location::Local(path.clone()),
            available: false,
            generation: session,
            state: "idle",
            error: None,
            catalog: None,
        });
        if let Some(catalog) = catalog {
            source.generation += 1;
            source.catalog = Some(Arc::new(catalog));
        } else if source.catalog.is_none() {
            // A manual refresh owns its in-flight generation. A periodic scan
            // with unchanged data must not cancel it by marking it unavailable.
            continue;
        }
        source.available = source.catalog.is_some();
        source.location = Location::Local(path);
        source.label = label;
        source.state = if source.available {
            "ready"
        } else {
            "unavailable"
        };
        source.error = None;
    }
}
pub(crate) fn local_catalog(
    library: Library,
    tags: Option<MyTags>,
    fingerprint: String,
) -> Catalog {
    Catalog {
        artwork: Default::default(),
        library,
        tags,
        tag_error: None,
        fingerprint,
    }
}

/// Direct sources are deliberately distinct from loadable, discovered sources.
pub fn connect_direct(shared: &Shared, ip: Ipv4Addr) -> Result<(), String> {
    {
        let mut state = shared.lock().unwrap();
        if state.interface.is_some() {
            return Err("Stop live monitoring before connecting a manual source".into());
        }
        if state
            .sources
            .get("direct")
            .is_some_and(|s| s.state == "loading")
        {
            return Err("A library read is already running; wait before connecting again".into());
        }
        let generation = state
            .sources
            .get("direct")
            .map_or(state.session, |s| s.generation + 1);
        state.sources.insert(
            "direct".into(),
            Source {
                label: format!("USB · {ip} · MANUAL"),
                location: Location::Direct(ip),
                available: true,
                generation,
                state: "idle",
                error: None,
                catalog: None,
            },
        );
    }
    refresh(shared, "direct")
}

/// One independent catalog per live IP. Reconnection replaces its generation.
pub fn connect_direct_peer(shared: &Shared, ip: Ipv4Addr, number: u8) -> Result<(), String> {
    let id = format!("direct:{ip}");
    {
        let mut state = shared.lock().unwrap();
        if state.interface.is_some() {
            return Err("Desktop live session is active".into());
        }
        let generation = state
            .sources
            .get(&id)
            .map_or(state.session, |s| s.generation + 1);
        state.sources.insert(
            id.clone(),
            Source {
                label: format!("CDJ {number} · USB · {ip}"),
                location: Location::Direct(ip),
                available: true,
                generation,
                state: "idle",
                error: None,
                catalog: None,
            },
        );
    }
    refresh(shared, &id)
}
pub fn invalidate_direct_peer(shared: &Shared, ip: Ipv4Addr) {
    let mut state = shared.lock().unwrap();
    for source in state.sources.values_mut() {
        if source.location == Location::Direct(ip) {
            source.available = false;
            source.generation += 1;
            source.catalog = None;
            source.state = "unavailable";
            source.error = None;
        }
    }
}
pub fn invalidate_direct(shared: &Shared) {
    let mut state = shared.lock().unwrap();
    for source in state.sources.values_mut() {
        if matches!(source.location, Location::Direct(_)) {
            source.available = false;
            source.generation += 1;
            source.catalog = None;
            source.state = "unavailable";
            source.error = None;
        }
    }
}
pub fn connection(
    location: &Location,
    interface: Option<String>,
) -> Result<(Ipv4Addr, Slot, Option<Interface>), String> {
    match location {
        Location::Direct(ip) => Ok((*ip, Slot::USB, None)),
        Location::Remote(ip, slot) => {
            let name = interface.ok_or("Live interface is unavailable")?;
            Ok((
                *ip,
                *slot,
                Some(Interface::named(&name).map_err(|e| e.to_string())?),
            ))
        }
        Location::Local(_) => Err("This source is a local file".into()),
    }
}

/// Called by the existing monitor with fresh, mounted slots, including idle decks.
pub fn sync_sources(shared: &Shared, discovered: Vec<(String, String, Location)>) {
    let mut state = shared.lock().unwrap();
    for (id, source) in &mut state.sources {
        if matches!(source.location, Location::Remote(..))
            && !discovered
                .iter()
                .any(|(key, _, loc)| key == id && *loc == source.location)
            && source.available
        {
            source.available = false;
            source.generation += 1;
            source.catalog = None;
            source.state = "unavailable";
            source.error = None;
        }
    }
    let session = state.session;
    for (id, label, location) in discovered {
        let source = state.sources.entry(id).or_insert_with(|| Source {
            label: label.clone(),
            location: location.clone(),
            available: false,
            generation: session,
            state: "idle",
            error: None,
            catalog: None,
        });
        if !source.available || source.location != location {
            source.generation += 1;
            source.catalog = None;
            source.state = "idle";
            source.error = None;
        }
        source.available = true;
        source.location = location;
        source.label = label;
    }
}
pub fn artwork_location(
    shared: &Shared,
    id: &str,
    generation: u64,
) -> Option<(Location, Option<String>)> {
    let state = shared.lock().unwrap();
    let source = state.sources.get(id)?;
    (source.available && source.generation == generation)
        .then(|| (source.location.clone(), state.interface.clone()))
}
/// Resolve a selection at send time, never trusting a browser-supplied address.
pub fn load_source(
    shared: &Shared,
    id: &str,
    generation: u64,
    track_id: u32,
) -> Result<(Ipv4Addr, String), String> {
    let state = shared.lock().unwrap();
    let source = state.sources.get(id).ok_or("Unknown USB source")?;
    if !source.available || source.generation != generation {
        return Err("USB changed or disconnected; select the track again".into());
    }
    let Location::Remote(ip, Slot::USB) = source.location else {
        return Err(
            "Loading requires a USB connected to a CDJ; local exports and SD are browse-only"
                .into(),
        );
    };
    let track = source
        .catalog
        .as_ref()
        .and_then(|c| c.library.tracks.get(&track_id))
        .ok_or("Selected track is no longer in the catalog")?;
    Ok((ip, track.title.clone()))
}
pub fn sources(shared: &Shared) -> Value {
    let state = shared.lock().unwrap();
    json!({"sources":state.sources.iter().map(|(id,s)|json!({"id":id,"label":s.label,"available":s.available,"loadable":matches!(s.location, Location::Remote(_, Slot::USB)) || (state.interface.is_none() && id.starts_with("local-usb:") && matches!(s.location, Location::Local(_))),"direct":matches!(s.location, Location::Direct(_)),"generation":s.generation,"state":s.state,"error":s.error,"count":s.catalog.as_ref().map(|c|c.library.tracks.len())})).collect::<Vec<_>>()})
}
pub fn refresh(shared: &Shared, id: &str) -> Result<(), String> {
    let (location, generation, interface) = {
        let mut state = shared.lock().unwrap();
        let interface = state.interface.clone();
        let source = state.sources.get_mut(id).ok_or("Unknown library source")?;
        if !source.available {
            return Err("USB is unavailable".into());
        }
        if source.state == "loading" {
            return Ok(());
        }
        source.generation += 1;
        source.state = "loading";
        source.catalog = None;
        source.error = None;
        (source.location.clone(), source.generation, interface)
    };
    let shared = shared.clone();
    let id = id.to_owned();
    tokio::spawn(async move {
        let result =
            match tokio::time::timeout(Duration::from_secs(60), read(location, interface)).await {
                Ok(result) => result,
                Err(_) => Err("Library read timed out. Retry when the USB is available.".into()),
            };
        complete(&shared, &id, generation, result);
    });
    Ok(())
}
fn complete(shared: &Shared, id: &str, generation: u64, result: Result<Catalog, String>) {
    let mut state = shared.lock().unwrap();
    if let Some(source) = state.sources.get_mut(id)
        && source.available
        && source.generation == generation
    {
        match result {
            Ok(catalog) => {
                source.catalog = Some(Arc::new(catalog));
                source.state = "ready";
            }
            Err(error) => {
                source.state = "error";
                source.error = Some(error);
            }
        }
    }
}
async fn remote_file(client: &mut NfsClient, slot: Slot, path: &str) -> Result<Vec<u8>, String> {
    let mount = client.mount_slot(slot).await.map_err(|e| e.to_string())?;
    let file = client.open(&mount, path).await.map_err(|e| e.to_string())?;
    if file.size() > MAX_DATABASE {
        return Err("Database exceeds 64 MiB limit".into());
    }
    client.read_file(&file).await.map_err(|e| e.to_string())
}
fn local_file(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    file.take(MAX_DATABASE + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > MAX_DATABASE {
        return Err("Database exceeds 64 MiB limit".into());
    }
    Ok(data)
}
async fn read(location: Location, interface: Option<String>) -> Result<Catalog, String> {
    if let Location::Local(path) = &location
        && path.file_name().is_some_and(|n| n == "exportLibrary.db")
    {
        let path = path.clone();
        return tokio::task::spawn_blocking(move || {
            let (library, tags, fingerprint) = crate::onelibrary::read(&path)?;
            Ok(Catalog {
                artwork: Default::default(),
                library,
                tags,
                tag_error: None,
                fingerprint,
            })
        })
        .await
        .map_err(|e| e.to_string())?;
    }

    let (base, extension) = match location {
        Location::Remote(..) | Location::Direct(_) => {
            let (ip, slot, interface) = connection(&location, interface)?;
            let mut client = NfsClient::connect(ip, interface.as_ref())
                .await
                .map_err(|e| e.to_string())?;
            let base = remote_file(&mut client, slot, "/PIONEER/rekordbox/export.pdb")
                .await
                .map_err(|e| format!("Cannot read supported export.pdb: {e}"))?;
            // An optional extension failure must not hide the usable base catalog.
            let ext = tokio::time::timeout(
                Duration::from_secs(15),
                remote_file(&mut client, slot, "/PIONEER/rekordbox/exportExt.pdb"),
            )
            .await
            .unwrap_or_else(|_| Err("My Tags read timed out".into()));
            (base, ext)
        }
        Location::Local(path) => tokio::task::spawn_blocking(move || {
            Ok::<_, String>((
                local_file(&path)?,
                local_file(&path.with_file_name("exportExt.pdb")),
            ))
        })
        .await
        .map_err(|e| e.to_string())??,
    };
    tokio::task::spawn_blocking(move || parse(&base, extension))
        .await
        .map_err(|e| e.to_string())?
}
fn parse(base: &[u8], extension: Result<Vec<u8>, String>) -> Result<Catalog, String> {
    let library = Library::parse(base).map_err(|e| e.to_string())?;
    let extension_digest = extension
        .as_ref()
        .ok()
        .map(|bytes| format!("{:?}", prolink_rekordbox::stable_digest(bytes)));
    let fingerprint = format!(
        "{:?}:{}",
        library.digest,
        extension_digest.unwrap_or_default()
    );
    let tags = extension.and_then(|data| MyTags::parse(&data).map_err(|e| e.to_string()));
    let (tags, tag_error) = match tags {
        Ok(tags) => (Some(tags), None),
        Err(e) => (None, Some(format!("My Tags unavailable: {e}"))),
    };
    Ok(Catalog {
        artwork: crate::artwork::Cache::default(),
        library,
        tags,
        tag_error,
        fingerprint,
    })
}
pub fn catalog(shared: &Shared, id: &str, query: &Query) -> Result<(u64, Arc<Catalog>), String> {
    let state = shared.lock().unwrap();
    let source = state.sources.get(id).ok_or("Unknown library source")?;
    if !source.available {
        return Err("Source disconnected or USB removed".into());
    }
    if query.get("generation").and_then(|s| s.parse::<u64>().ok()) != Some(source.generation) {
        return Err("Library changed. Refresh the browser.".into());
    }
    Ok((
        source.generation,
        source.catalog.clone().ok_or("Library is not ready")?,
    ))
}
/// Reuse an already-read catalog for status metadata without retaining its lock.
pub fn cached_track(shared: &Shared, ip: Ipv4Addr, slot: Slot, id: u32) -> Option<Track> {
    let state = shared.lock().unwrap();
    state
        .sources
        .values()
        .find(|s| {
            s.available
                && (s.location == Location::Remote(ip, slot)
                    || (slot == Slot::USB && s.location == Location::Direct(ip)))
        })?
        .catalog
        .as_ref()?
        .library
        .tracks
        .get(&id)
        .cloned()
}
pub(crate) fn tag_metadata(tags: Option<&MyTags>, id: u32) -> Value {
    tags.map(|tags| {
        json!(
            tags.track_tags
                .get(&id)
                .into_iter()
                .flatten()
                .filter_map(|id| tags.values.get(id))
                .map(
                    |tag| json!({"name":tag.name,"category":tags.categories.get(&tag.category_id)})
                )
                .collect::<Vec<_>>()
        )
    })
    .unwrap_or(Value::Null)
}
pub(crate) fn cached_tags(shared: &Shared, ip: Ipv4Addr, slot: Slot, id: u32) -> Value {
    let state = shared.lock().unwrap();
    let catalog = state
        .sources
        .values()
        .find(|s| {
            s.available
                && (s.location == Location::Remote(ip, slot)
                    || (slot == Slot::USB && s.location == Location::Direct(ip)))
        })
        .and_then(|s| s.catalog.as_ref());
    tag_metadata(catalog.and_then(|c| c.tags.as_ref()), id)
}
impl Catalog {
    pub(crate) fn track_metadata(&self, track: &Track) -> Value {
        let mut value = metadata(track);
        value["myTags"] = tag_metadata(self.tags.as_ref(), track.id);
        value
    }
    pub fn description(&self, generation: u64) -> Value {
        let facets = [
            "genre", "artist", "album", "key", "label", "color", "format",
        ];
        let facets: serde_json::Map<String, Value> = facets
            .into_iter()
            .map(|field| {
                let mut counts = BTreeMap::<String, usize>::new();
                for track in self.library.tracks.values() {
                    *counts.entry(field_value(track, field)).or_default() += 1;
                }
                let mut counts: Vec<_> = counts.into_iter().collect();
                if field == "key" {
                    counts.sort_by(|a, b| crate::musical_key::compare(&a.0, &b.0));
                }
                (
                    field.into(),
                    json!(
                        counts
                            .into_iter()
                            .map(|(value, count)| json!({"value":value,"count":count}))
                            .collect::<Vec<_>>()
                    ),
                )
            })
            .collect();
        json!({"generation":generation,"fingerprint":self.fingerprint,"count":self.library.tracks.len(),"tagError":self.tag_error,"tagsAvailable":self.tags.is_some(),"facets":facets,
            "playlists":self.library.playlists.values().map(|p|json!({"id":p.id,"name":p.name,"parentId":p.parent_id,"folder":p.is_folder,"order":p.sort_order,"count":p.track_count()})).collect::<Vec<_>>(),
            "categories":self.tags.as_ref().map(|t|t.categories.iter().map(|(id,name)|json!({"id":id,"name":name})).collect::<Vec<_>>()).unwrap_or_default(),
            "tags":self.tags.as_ref().map(|t|t.values.values().map(|v|json!({"id":v.id,"categoryId":v.category_id,"order":v.index,"name":v.name})).collect::<Vec<_>>()).unwrap_or_default()})
    }
    pub fn tracks(&self, generation: u64, query: &Query) -> Result<Value, String> {
        self.tracks_for_set(generation, query, None)
    }
    pub fn tracks_for_set(
        &self,
        generation: u64,
        query: &Query,
        set: Option<&[crate::set_history::Track]>,
    ) -> Result<Value, String> {
        // Resolve against the selected USB, never by a bare rekordbox row ID.
        // IDs can refer to entirely different tracks on another export.
        let resolved: Option<Vec<Track>> = set.map(|entries| {
            entries
                .iter()
                .map(|saved| {
                    let mut matches = self.library.tracks.values().filter(|track| {
                        if !saved.file_path.is_empty() {
                            track.file_path == saved.file_path
                                && track.title == saved.title
                                && track.artist == saved.artist
                        } else {
                            !saved.title.is_empty()
                                && track.title == saved.title
                                && track.artist == saved.artist
                        }
                    });
                    let first = matches.next();
                    if let Some(track) = first.filter(|_| matches.next().is_none()) {
                        track.clone()
                    } else {
                        Track {
                            title: saved.title.clone(),
                            artist: saved.artist.clone(),
                            key: saved.key.clone(),
                            rating: saved.rating,
                            ..Track::default()
                        }
                    }
                })
                .collect()
        });
        let get = |key: &str| query.get(key).map(String::as_str).unwrap_or("");
        let playlist = get("playlist").parse::<u32>().ok();
        let mut tracks: Vec<(usize, &Track)> = if let Some(entries) = &resolved {
            entries.iter().enumerate().collect()
        } else if let Some(id) = playlist {
            let p = self.library.playlists.get(&id).ok_or("Unknown playlist")?;
            if p.is_folder {
                vec![]
            } else {
                p.track_ids
                    .iter()
                    .enumerate()
                    .filter_map(|(i, id)| self.library.tracks.get(id).map(|t| (i, t)))
                    .collect()
            }
        } else {
            self.library.tracks.values().enumerate().collect()
        };
        let needle = get("q").to_lowercase();
        let tag_ids: Vec<u32> = get("tags")
            .split(',')
            .filter_map(|id| id.parse().ok())
            .collect();
        if !tag_ids.is_empty() && self.tags.is_none() {
            return Err(
                "My Tags are unavailable; remove the tag filter or retry the source".into(),
            );
        }
        let mut category_filters = Vec::new();
        for (key, raw) in query
            .iter()
            .filter(|(key, _)| key.starts_with("tagCategory:") && !key.ends_with(":mode"))
        {
            if raw.is_empty() {
                continue;
            }
            let values: Vec<String> =
                serde_json::from_str(raw).map_err(|_| "Invalid My Tag category filter")?;
            if values.is_empty() {
                continue;
            }
            let tags = self
                .tags
                .as_ref()
                .ok_or("My Tags unavailable for this source")?;
            let category = key
                .trim_start_matches("tagCategory:")
                .parse::<u32>()
                .map_err(|_| "Invalid My Tag category")?;
            let ids = values
                .iter()
                .map(|s| s.parse::<u32>().map_err(|_| "Invalid My Tag value"))
                .collect::<Result<Vec<_>, _>>()?;
            if !tags.categories.contains_key(&category)
                || ids.iter().any(|id| {
                    tags.values
                        .get(id)
                        .is_none_or(|t| t.category_id != category)
                })
            {
                return Err("My Tag category or value changed; select the filters again".into());
            }
            let mode = query
                .get(&format!("{key}:mode"))
                .map(String::as_str)
                .unwrap_or("any");
            category_filters.push((ids, mode));
        }
        let rules: Vec<Value> = if get("rules").is_empty() {
            vec![]
        } else {
            serde_json::from_str(get("rules")).map_err(|_| "Invalid metadata rules")?
        };
        validate_rules(&rules)?;
        let number = |key: &str| get(key).parse::<f64>().ok().filter(|v| v.is_finite());
        tracks.retain(|(_, t)| {
            for (ids, mode) in &category_filters {
                let assigned = self
                    .tags
                    .as_ref()
                    .and_then(|tags| tags.track_tags.get(&t.id));
                let has = |id: &u32| assigned.is_some_and(|set| set.contains(id));
                let matches = match *mode {
                    "all" => ids.iter().all(has),
                    "none" => !ids.iter().any(has),
                    _ => ids.iter().any(has),
                };
                if !matches {
                    return false;
                }
            }
            if !rules.is_empty() && !matches_rules(&metadata(t), &rules) {
                return false;
            }
            if !needle.is_empty()
                && ![
                    &t.title,
                    &t.artist,
                    &t.album,
                    &t.comment,
                    &t.genre,
                    &t.label,
                    &t.composer,
                    &t.remixer,
                    &t.original_artist,
                    &t.mix_name,
                    &t.filename,
                    &t.isrc,
                ]
                .iter()
                .any(|s| s.to_lowercase().contains(&needle))
            {
                return false;
            }
            for field in [
                "genre", "artist", "album", "key", "label", "color", "format",
            ] {
                if let Some(raw) = query.get(field).filter(|raw| !raw.is_empty()) {
                    let Ok(values) = serde_json::from_str::<Vec<String>>(raw) else {
                        return false;
                    };
                    if !values.is_empty() && !values.contains(&field_value(t, field)) {
                        return false;
                    }
                }
            }
            if (get("rating") == "unrated" && t.rating != 0)
                || number("rating").is_some_and(|n| f64::from(t.rating) < n)
            {
                return false;
            }
            if number("bpmMin").is_some_and(|n| t.bpm() < n)
                || number("bpmMax").is_some_and(|n| t.bpm() > n)
            {
                return false;
            }
            if number("yearMin").is_some_and(|n| f64::from(t.year) < n)
                || number("yearMax").is_some_and(|n| f64::from(t.year) > n)
            {
                return false;
            }
            if number("durationMin").is_some_and(|n| f64::from(t.duration) < n)
                || number("durationMax").is_some_and(|n| f64::from(t.duration) > n)
            {
                return false;
            }
            if !get("addedFrom").is_empty() && t.date_added.as_str() < get("addedFrom") {
                return false;
            }
            if !get("addedTo").is_empty()
                && (t.date_added.is_empty() || t.date_added.as_str() > get("addedTo"))
            {
                return false;
            }
            if !tag_ids.is_empty() {
                let assigned = self
                    .tags
                    .as_ref()
                    .and_then(|tags| tags.track_tags.get(&t.id));
                let has = |id: &u32| assigned.is_some_and(|set| set.contains(id));
                if !match get("tagMode") {
                    "all" => tag_ids.iter().all(has),
                    "none" => !tag_ids.iter().any(has),
                    _ => tag_ids.iter().any(has),
                } {
                    return false;
                }
            }
            true
        });
        let sort = get("sort");
        if sort != "playlist" || (playlist.is_none() && set.is_none()) {
            tracks.sort_by(|a, b| {
                let a = a.1;
                let b = b.1;
                let order = match sort {
                    "bpm" => a.tempo.cmp(&b.tempo),
                    "rating" => a.rating.cmp(&b.rating),
                    "duration" => a.duration.cmp(&b.duration),
                    "artist" => a.artist.to_lowercase().cmp(&b.artist.to_lowercase()),
                    "key" => crate::musical_key::compare(&a.key, &b.key),
                    "genre" => a.genre.to_lowercase().cmp(&b.genre.to_lowercase()),
                    "color" => a.color.cmp(&b.color),
                    _ => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                }
                .then(a.id.cmp(&b.id));
                if get("direction") == "desc" {
                    order.reverse()
                } else {
                    order
                }
            });
        }
        let total = tracks.len();
        let offset = get("offset").parse::<usize>().unwrap_or(0);
        let limit = if get("all") == "true" {
            total
        } else {
            get("limit").parse::<usize>().unwrap_or(50).clamp(1, 100)
        };
        Ok(
            json!({"generation":generation,"total":total,"offset":offset,"tracks":tracks.into_iter().skip(offset).take(limit).map(|(entry,t)| {
            let mut value = metadata(t);
            value["entry"] = json!(entry);
            if let Some(saved) = set.and_then(|entries| entries.get(entry)) {
                value["available"] = json!(t.id != 0);
                value["savedArtwork"] = json!(saved.artwork);
            }
            value["myTags"] = self.tags.as_ref().map(|tags|json!(tags.track_tags.get(&t.id).into_iter().flatten().filter_map(|id|tags.values.get(id)).map(|tag|json!({"id":tag.id,"name":tag.name,"category":tags.categories.get(&tag.category_id)})).collect::<Vec<_>>())).unwrap_or(Value::Null);
            value
        }).collect::<Vec<_>>() }),
        )
    }
}
fn validate_rules(rules: &[Value]) -> Result<(), String> {
    if rules.len() > 16 {
        return Err("At most 16 metadata rules are supported".into());
    }
    let example = metadata(&Track::default());
    for rule in rules {
        let field = rule["field"].as_str().ok_or("Rule field is required")?;
        let value = example.get(field).ok_or("Unknown metadata field")?;
        let op = rule["op"].as_str().ok_or("Rule operator is required")?;
        if ![
            "contains",
            "notContains",
            "eq",
            "gte",
            "lte",
            "empty",
            "notEmpty",
        ]
        .contains(&op)
        {
            return Err("Unknown metadata operator".into());
        }
        if op != "empty" && op != "notEmpty" {
            let input = rule["value"].as_str().ok_or("Rule value is required")?;
            if value.is_number()
                && (matches!(op, "contains" | "notContains")
                    || input.parse::<f64>().ok().is_none_or(|n| !n.is_finite()))
            {
                return Err(
                    "Numeric rules require a finite number and a comparison operator".into(),
                );
            }
        }
    }
    Ok(())
}
fn matches_rules(metadata: &Value, rules: &[Value]) -> bool {
    rules.iter().all(|rule| {
        let value = &metadata[rule["field"].as_str().unwrap_or("")];
        let op = rule["op"].as_str().unwrap_or("");
        let input = rule["value"].as_str().unwrap_or("");
        let empty = value.is_null() || value.as_str() == Some("");
        if op == "empty" {
            return empty;
        }
        if op == "notEmpty" {
            return !empty;
        }
        if empty {
            return false;
        }
        if let Some(number) = value.as_f64() {
            let Ok(input) = input.parse::<f64>() else {
                return false;
            };
            return match op {
                "eq" => number == input,
                "gte" => number >= input,
                "lte" => number <= input,
                _ => false,
            };
        }
        let text = value.as_str().unwrap_or("").to_lowercase();
        let input = input.to_lowercase();
        match op {
            "contains" => text.contains(&input),
            "notContains" => !text.contains(&input),
            "eq" => text == input,
            "gte" => text >= input,
            "lte" => text <= input,
            _ => false,
        }
    })
}
fn field_value(t: &Track, field: &str) -> String {
    match field {
        "genre" => t.genre.clone(),
        "artist" => t.artist.clone(),
        "album" => t.album.clone(),
        "key" => t.key.clone(),
        "label" => t.label.clone(),
        "color" => t.color.clone(),
        "format" => format!("{:?}", t.container),
        _ => String::new(),
    }
}
pub(crate) fn metadata(t: &Track) -> Value {
    json!({"id":t.id,"title":t.title,"artist":t.artist,"album":t.album,"genre":t.genre,"key":t.key,"label":t.label,"composer":t.composer,"originalArtist":t.original_artist,"remixer":t.remixer,"color":t.color,"colorId":t.color_id,"comment":t.comment,"isrc":t.isrc,"dateAdded":t.date_added,"releaseDate":t.release_date,"mixName":t.mix_name,"filePath":t.file_path,"filename":t.filename,"analysisPath":t.analyze_path,"artworkPath":t.artwork_path,"bpm":t.bpm(),"duration":t.duration,"bitrate":t.bitrate,"sampleRate":t.sample_rate,"sampleDepth":t.sample_depth,"fileSize":t.file_size,"trackNumber":t.track_number,"discNumber":t.disc_number,"year":t.year,"rating":t.rating,"playCount":t.play_count,"format":format!("{:?}",t.container)})
}

#[cfg(test)]
pub(crate) fn install_test_direct_catalog(
    shared: &Shared,
    ip: Ipv4Addr,
    track_id: u32,
) -> (String, u64) {
    let id = format!("direct:{ip}");
    let mut state = shared.lock().unwrap();
    let generation = state.session;
    let mut library = Library::default();
    library.tracks.insert(
        track_id,
        Track {
            id: track_id,
            ..Track::default()
        },
    );
    state.sources.insert(
        id.clone(),
        Source {
            label: id.clone(),
            location: Location::Direct(ip),
            available: true,
            generation,
            state: "ready",
            error: None,
            catalog: Some(Arc::new(Catalog {
                artwork: Default::default(),
                library,
                tags: None,
                tag_error: None,
                fingerprint: "fixture".into(),
            })),
        },
    );
    (id, generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prolink_rekordbox::{Playlist, mytags::MyTag};
    #[test]
    fn saved_sets_browse_offline_with_order_search_and_disabled_loading() {
        let c = fixture();
        let mut saved: Vec<_> = [3, 1, 3]
            .iter()
            .map(|id| crate::set_history::Track::from_library(&c.library.tracks[id]))
            .collect();
        saved[0].artwork = Some("saved-cover.jpg".into());
        let mut q = query(&[("sort", "playlist")]);
        let result = saved_tracks(&q, &saved).unwrap();
        let rows = result["tracks"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|t| t["title"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["Track 3", "Track 1", "Track 3"]
        );
        assert!(rows.iter().all(|t| t["available"] == false));
        assert_eq!(rows[0]["savedArtwork"], "saved-cover.jpg");
        assert_eq!(rows[2]["entry"], 2);
        q.insert("q".into(), "Track 1".into());
        assert_eq!(saved_tracks(&q, &saved).unwrap()["total"], 1);
        q.remove("q");
        q.insert("rating".into(), "5".into());
        assert_eq!(saved_tracks(&q, &saved).unwrap()["total"], 2);
    }

    #[test]
    fn saved_sets_keep_order_repeats_and_use_the_same_search_and_filters() {
        let c = fixture();
        let saved: Vec<_> = [3, 1, 3]
            .iter()
            .map(|id| crate::set_history::Track::from_library(&c.library.tracks[id]))
            .collect();
        let mut query = Query::from([("sort".into(), "playlist".into())]);
        let result = c.tracks_for_set(1, &query, Some(&saved)).unwrap();
        let ids = |result: &Value| {
            result["tracks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["id"].as_u64().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&result), vec![3, 1, 3]);
        assert_eq!(result["tracks"][2]["entry"], 2);
        query.insert("q".into(), "Track 3".into());
        assert_eq!(
            ids(&c.tracks_for_set(1, &query, Some(&saved)).unwrap()),
            vec![3, 3]
        );
        query.remove("q");
        query.insert("rating".into(), "5".into());
        assert_eq!(
            ids(&c.tracks_for_set(1, &query, Some(&saved)).unwrap()),
            vec![3, 3]
        );
        query.remove("rating");
        query.insert("sort".into(), "bpm".into());
        assert_eq!(
            ids(&c.tracks_for_set(1, &query, Some(&saved)).unwrap()),
            vec![1, 3, 3]
        );
        query.insert("tags".into(), "11".into());
        let filtered = c.tracks_for_set(1, &query, Some(&saved)).unwrap();
        assert_eq!(ids(&filtered), vec![1]);
        assert_eq!(
            filtered["tracks"][0]["myTags"],
            c.tracks(1, &query).unwrap()["tracks"][0]["myTags"]
        );
    }

    #[test]
    fn saved_tracks_resolve_on_selected_usb_without_trusting_old_ids() {
        let mut c = fixture();
        let mut saved = crate::set_history::Track::from_library(&c.library.tracks[&1]);
        saved.id = 3; // Different export: this ID belongs to another song.
        let query = Query::from([("sort".into(), "playlist".into())]);
        let resolved = c.tracks_for_set(1, &query, Some(&[saved.clone()])).unwrap();
        assert_eq!(resolved["tracks"][0]["id"], 1);
        assert_eq!(resolved["tracks"][0]["available"], true);
        c.library.tracks.get_mut(&1).unwrap().file_path = "/Contents/song.mp3".into();
        let mut duplicate = c.library.tracks[&1].clone();
        duplicate.id = 4;
        duplicate.file_path = "/Contents/other-version.mp3".into();
        c.library.tracks.insert(4, duplicate);
        let unresolved = c.tracks_for_set(1, &query, Some(&[saved.clone()])).unwrap();
        assert_eq!(unresolved["tracks"][0]["available"], false);
        assert_eq!(unresolved["tracks"][0]["title"], "Track 1");
        saved.file_path = "/Contents/song.mp3".into();
        assert_eq!(
            c.tracks_for_set(1, &query, Some(&[saved.clone()])).unwrap()["tracks"][0]["id"],
            1
        );
        saved.file_path = "/Contents/missing.mp3".into();
        assert_eq!(
            c.tracks_for_set(1, &query, Some(&[saved])).unwrap()["tracks"][0]["available"],
            false
        );
    }

    fn fixture() -> Catalog {
        let mut library = Library::default();
        for (id, rating, color, bpm) in [
            (1, 4, "Blue", 12400),
            (2, 0, "", 12600),
            (3, 5, "Red", 12800),
        ] {
            library.tracks.insert(
                id,
                Track {
                    id,
                    title: format!("Track {id}"),
                    artist: "Élan".into(),
                    genre: "House".into(),
                    rating,
                    color: color.into(),
                    tempo: bpm,
                    ..Track::default()
                },
            );
        }
        library.playlists.insert(
            7,
            Playlist {
                id: 7,
                track_ids: vec![3, 1, 3, 999],
                ..Playlist::default()
            },
        );
        let tags = MyTags {
            categories: BTreeMap::from([(1, "Mood".into())]),
            values: BTreeMap::from([
                (
                    10,
                    MyTag {
                        id: 10,
                        category_id: 1,
                        index: 0,
                        name: "Warm".into(),
                    },
                ),
                (
                    11,
                    MyTag {
                        id: 11,
                        category_id: 1,
                        index: 1,
                        name: "Vocal".into(),
                    },
                ),
            ]),
            track_tags: BTreeMap::from([(1, [10, 11].into()), (3, [10].into())]),
        };
        Catalog {
            artwork: crate::artwork::Cache::default(),
            library,
            tags: Some(tags),
            tag_error: None,
            fingerprint: "test".into(),
        }
    }
    fn query(values: &[(&str, &str)]) -> Query {
        values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
    #[tokio::test]
    async fn dual_direct_catalogs_isolate_disconnect_and_late_completion() {
        let shared = with_local_path(None, None);
        let a = Ipv4Addr::new(192, 0, 2, 1);
        let b = Ipv4Addr::new(192, 0, 2, 2);
        let (first, generation) = install_test_direct_catalog(&shared, a, 1);
        let (second, other_generation) = install_test_direct_catalog(&shared, b, 1);
        invalidate_direct_peer(&shared, a);
        complete(&shared, &first, generation, Ok(fixture()));
        assert!(
            catalog(
                &shared,
                &first,
                &query(&[("generation", &generation.to_string())])
            )
            .is_err()
        );
        assert!(
            catalog(
                &shared,
                &second,
                &query(&[("generation", &other_generation.to_string())])
            )
            .is_ok()
        );
        assert!(!shared.lock().unwrap().sources[&first].available);
        connect_direct_peer(&shared, a, 1).unwrap();
        assert!(shared.lock().unwrap().sources[&first].generation > generation);
        assert_eq!(
            shared.lock().unwrap().sources[&second].generation,
            other_generation
        );
    }
    #[tokio::test]
    async fn manual_sources_cannot_load_and_ignore_late_catalogs() {
        let shared = with_local_path(None, None);
        let ip = Ipv4Addr::new(192, 0, 2, 1);
        connect_direct(&shared, ip).unwrap();
        let old = shared.lock().unwrap().sources["direct"].generation;
        assert_eq!(sources(&shared)["sources"][0]["loadable"], false);
        assert_eq!(sources(&shared)["sources"][0]["direct"], true);
        assert!(load_source(&shared, "direct", old, 1).is_err());
        assert!(connect_direct(&shared, ip).is_err());
        shared
            .lock()
            .unwrap()
            .sources
            .get_mut("direct")
            .unwrap()
            .state = "error";
        connect_direct(&shared, Ipv4Addr::new(192, 0, 2, 2)).unwrap();
        complete(&shared, "direct", old, Err("old response".into()));
        assert_eq!(shared.lock().unwrap().sources["direct"].state, "loading");
        assert!(artwork_location(&shared, "direct", old).is_none());
        assert!(connect_direct(&with_local_path(Some("en1".into()), None), ip).is_err());
    }

    #[test]
    fn removed_facets_do_not_hide_tracks_matching_remaining_tags() {
        let c = fixture();
        for field in [
            "genre", "artist", "album", "key", "label", "color", "format",
        ] {
            let mut filters = query(&[("tagCategory:1", "[\"10\"]")]);
            filters.insert(field.into(), "[\"no match\"]".into());
            assert_eq!(c.tracks(1, &filters).unwrap()["total"], 0);
            for cleared in ["", "[]"] {
                filters.insert(field.into(), cleared.into());
                assert_eq!(c.tracks(1, &filters).unwrap()["total"], 2);
            }
            filters.remove(field);
            assert_eq!(c.tracks(1, &filters).unwrap()["total"], 2);
        }
        // Selecting 'Not set' remains a real filter, distinct from clearing it.
        assert_eq!(
            c.tracks(1, &query(&[("color", "[\"\"]")])).unwrap()["total"],
            1
        );
    }

    #[test]
    fn combined_filters_and_missing_color() {
        let c = fixture();
        let result = c
            .tracks(
                1,
                &query(&[
                    ("genre", "[\"House\"]"),
                    ("rating", "4"),
                    ("bpmMax", "125"),
                    ("tags", "10,11"),
                    ("tagMode", "all"),
                ]),
            )
            .unwrap();
        assert_eq!(result["total"], 1);
        assert_eq!(result["tracks"][0]["id"], 1);
        let result = c
            .tracks(1, &query(&[("color", "[\"\"]"), ("rating", "unrated")]))
            .unwrap();
        assert_eq!(result["tracks"][0]["id"], 2);
        assert_eq!(c.tracks(1, &query(&[("q", "ÉLAN")])).unwrap()["total"], 3);
    }
    #[test]
    fn tags_any_all_none_and_unavailable() {
        let mut c = fixture();
        for (mode, count) in [("any", 2), ("all", 1), ("none", 1)] {
            assert_eq!(
                c.tracks(1, &query(&[("tags", "10,11"), ("tagMode", mode)]))
                    .unwrap()["total"],
                count
            );
        }
        c.tags = None;
        assert!(
            c.tracks(1, &query(&[("tags", "10"), ("tagMode", "none")]))
                .is_err()
        );
        assert!(c.tracks(1, &Query::new()).unwrap()["tracks"][0]["myTags"].is_null());
    }
    #[test]
    fn all_matching_tracks_are_returned_without_page_limit() {
        let mut c = fixture();
        let template = c.library.tracks[&1].clone();
        for id in 4..=175 {
            let mut track = template.clone();
            track.id = id;
            c.library.tracks.insert(id, track);
        }
        let result = c.tracks(1, &query(&[("all", "true")])).unwrap();
        assert_eq!(result["total"], 175);
        assert_eq!(result["tracks"].as_array().unwrap().len(), 175);
        let playlist = c
            .tracks(
                1,
                &query(&[("all", "true"), ("playlist", "7"), ("sort", "playlist")]),
            )
            .unwrap();
        assert_eq!(playlist["tracks"].as_array().unwrap().len(), 3);
        assert_eq!(playlist["tracks"][0]["id"], playlist["tracks"][2]["id"]);
    }
    #[test]
    fn pagination_preserves_playlist_duplicates_and_ignores_missing_tracks() {
        let c = fixture();
        let result = c
            .tracks(
                2,
                &query(&[
                    ("playlist", "7"),
                    ("sort", "playlist"),
                    ("offset", "1"),
                    ("limit", "2"),
                ]),
            )
            .unwrap();
        assert_eq!(result["total"], 3);
        assert_eq!(result["tracks"][0]["id"], 1);
        assert_eq!(result["tracks"][1]["id"], 3);
        assert_eq!(result["tracks"][1]["entry"], 2);
    }
    #[test]
    fn ejection_and_reconnect_reject_old_completion_even_with_same_track_ids() {
        let shared = Arc::new(Mutex::new(Catalogs {
            session: 0,
            interface: None,
            sources: BTreeMap::new(),
        }));
        let source = || {
            vec![(
                "one".into(),
                "USB".into(),
                Location::Remote(Ipv4Addr::LOCALHOST, Slot::USB),
            )]
        };
        sync_sources(&shared, source());
        let old = shared.lock().unwrap().sources["one"].generation;
        complete(&shared, "one", old, Ok(fixture()));
        assert!(catalog(&shared, "one", &query(&[("generation", &old.to_string())])).is_ok());
        sync_sources(&shared, vec![]);
        sync_sources(&shared, source());
        complete(&shared, "one", old, Ok(fixture()));
        assert!(shared.lock().unwrap().sources["one"].catalog.is_none());
        assert!(catalog(&shared, "one", &query(&[("generation", &old.to_string())])).is_err());
    }
    #[test]
    fn category_filters_combine_and_validate_custom_values() {
        let mut c = fixture();
        let tags = c.tags.as_mut().unwrap();
        tags.categories.insert(2, "Venue".into());
        tags.values.insert(
            20,
            MyTag {
                id: 20,
                category_id: 2,
                index: 0,
                name: "Small room".into(),
            },
        );
        tags.track_tags.get_mut(&1).unwrap().insert(20);
        assert_eq!(
            c.tracks(
                1,
                &query(&[("tagCategory:1", "[\"10\"]"), ("tagCategory:2", "[\"20\"]")])
            )
            .unwrap()["total"],
            1
        );
        assert_eq!(
            c.tracks(
                1,
                &query(&[
                    ("tagCategory:1", "[\"10\"]"),
                    ("tagCategory:2", "[\"20\"]"),
                    ("tagCategory:2:mode", "none")
                ])
            )
            .unwrap()["total"],
            1
        );
        assert!(
            c.tracks(1, &query(&[("tagCategory:1", "[\"20\"]")]))
                .is_err()
        );
        assert_eq!(
            c.tracks(1, &query(&[("tagCategory:1", "")])).unwrap()["total"],
            3
        );
    }
    #[test]
    fn keys_use_harmonic_order_for_tracks_and_filter_values() {
        let mut c = fixture();
        for (id, key) in [(1, "10A"), (2, "2B"), (3, "Ebm")] {
            c.library.tracks.get_mut(&id).unwrap().key = key.into();
        }
        let result = c
            .tracks(1, &query(&[("sort", "key"), ("all", "true")]))
            .unwrap();
        assert_eq!(result["tracks"][0]["id"], 3);
        assert_eq!(result["tracks"][1]["id"], 2);
        assert_eq!(result["tracks"][2]["id"], 1);
        let desc = c
            .tracks(1, &query(&[("sort", "key"), ("direction", "desc")]))
            .unwrap();
        assert_eq!(desc["tracks"][0]["id"], 1);
        let info = c.description(1);
        assert_eq!(info["facets"]["key"][0]["value"], "Ebm");
        assert_eq!(info["facets"]["key"][1]["value"], "2B");
        assert_eq!(info["facets"]["key"][2]["value"], "10A");
    }

    #[test]
    fn sort_applies_to_full_result_before_pagination() {
        let c = fixture();
        for (sort, first, last) in [
            ("bpm", 1, 3),
            ("rating", 2, 3),
            ("color", 2, 3),
            ("title", 1, 3),
        ] {
            assert_eq!(
                c.tracks(1, &query(&[("sort", sort), ("limit", "1")]))
                    .unwrap()["tracks"][0]["id"],
                first
            );
            assert_eq!(
                c.tracks(
                    1,
                    &query(&[("sort", sort), ("direction", "desc"), ("limit", "1")])
                )
                .unwrap()["tracks"][0]["id"],
                last
            );
        }
    }
    #[test]
    fn advanced_metadata_rules_compare_numbers_text_and_empty_fields() {
        let c = fixture();
        let rules = json!([{"field":"rating","op":"eq","value":"4"},{"field":"artist","op":"contains","value":"ÉL"},{"field":"comment","op":"empty","value":""}]).to_string();
        assert_eq!(
            c.tracks(1, &query(&[("rules", &rules)])).unwrap()["total"],
            1
        );
        let rules = json!([{"field":"rating","op":"gte","value":"NaN"}]).to_string();
        assert!(c.tracks(1, &query(&[("rules", &rules)])).is_err());
        let rules = json!([{"field":"notMetadata","op":"eq","value":"x"}]).to_string();
        assert!(c.tracks(1, &query(&[("rules", &rules)])).is_err());
    }
    #[tokio::test]
    async fn refresh_deduplicates_and_failure_can_be_retried() {
        let shared = Arc::new(Mutex::new(Catalogs {
            session: 0,
            interface: None,
            sources: BTreeMap::from([(
                "local".into(),
                Source {
                    label: "Test".into(),
                    location: Location::Local(PathBuf::from("/nonexistent-pc-library/export.pdb")),
                    available: true,
                    generation: 0,
                    state: "idle",
                    error: None,
                    catalog: None,
                },
            )]),
        }));
        refresh(&shared, "local").unwrap();
        refresh(&shared, "local").unwrap();
        assert_eq!(shared.lock().unwrap().sources["local"].generation, 1);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if shared.lock().unwrap().sources["local"].state == "error" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        refresh(&shared, "local").unwrap();
        assert_eq!(shared.lock().unwrap().sources["local"].generation, 2);
    }
    #[test]
    fn sources_do_not_share_bare_track_ids() {
        let shared = Arc::new(Mutex::new(Catalogs {
            session: 0,
            interface: None,
            sources: BTreeMap::new(),
        }));
        sync_sources(
            &shared,
            vec![
                (
                    "a".into(),
                    "A".into(),
                    Location::Remote(Ipv4Addr::new(10, 0, 0, 1), Slot::USB),
                ),
                (
                    "b".into(),
                    "B".into(),
                    Location::Remote(Ipv4Addr::new(10, 0, 0, 2), Slot::USB),
                ),
            ],
        );
        complete(&shared, "a", 1, Ok(fixture()));
        assert!(cached_track(&shared, Ipv4Addr::new(10, 0, 0, 1), Slot::USB, 1).is_some());
        assert!(cached_track(&shared, Ipv4Addr::new(10, 0, 0, 2), Slot::USB, 1).is_none());
    }
}
