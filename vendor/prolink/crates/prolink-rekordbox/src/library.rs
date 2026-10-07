// SPDX-License-Identifier: GPL-3.0-only

//! The joined library: an `export.pdb` with its foreign keys resolved.
//!
//! The pdb stores a track with integer references into side tables — artist,
//! album, genre, key, label, colour, artwork — so the raw rows are not by
//! themselves browsable. This joins them once, and it is the shape the Pro DJ
//! Link browse surface is built from.
//!
//! # The ids are kept, not just the names
//!
//! A dbserver metadata item carries the **referenced row's** id — artist 122,
//! album 86 — not the track's own, and a player uses it to open "more from this
//! artist". Sending the track id there is wrong in a way that still renders
//! correctly on screen, so it survives casual inspection and then the wrong
//! menu opens. Hence every [`Track`] carries both the resolved string and the
//! id it came from.
//!
//! # Ordering
//!
//! Playlist entries carry an explicit index and the on-disk row order is not
//! the playlist order, so entries are sorted before being attached. Playlists
//! and folders sort on `(sort_order, name)`, which is rekordbox's own order.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::pdb::{
    AlbumRow, ArtistRow, ArtworkRow, ColorRow, Container, GenreRow, HistoryEntryRow,
    HistoryPlaylistRow, KeyRow, LabelRow, Pdb, PlaylistEntryRow, PlaylistTreeRow, StableDigest,
    TrackRow,
};

/// One track, with its references resolved.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Track {
    /// The track's own row id.
    pub id: u32,
    /// Title.
    pub title: String,
    /// Artist name, empty when the track has none.
    pub artist: String,
    /// Album name.
    pub album: String,
    /// Genre name.
    pub genre: String,
    /// Key name, in whichever notation the collection uses.
    pub key: String,
    /// Record label name.
    pub label: String,
    /// Composer's name, from the artist table.
    pub composer: String,
    /// Original artist's name, from the artist table.
    pub original_artist: String,
    /// Remixer's name, from the artist table.
    pub remixer: String,
    /// Colour label name.
    pub color: String,
    /// The DJ's comment.
    pub comment: String,
    /// International Standard Recording Code, if the file carried one.
    pub isrc: String,
    /// When the track was added, `YYYY-MM-DD`.
    pub date_added: String,
    /// Release date.
    pub release_date: String,
    /// Mix or remix name.
    pub mix_name: String,

    /// Absolute path to the audio file **in the player's namespace**. Prefix a
    /// mount point to get a local path.
    pub file_path: String,
    /// File name without a directory.
    pub filename: String,
    /// Path to the `.DAT` analysis file. See [`Track::analyze_ext_path`].
    pub analyze_path: String,
    /// Path to the artwork image, from the artwork table.
    pub artwork_path: String,

    /// Tempo in centi-BPM; see [`Track::bpm`].
    pub tempo: u32,
    /// Playing time in seconds.
    pub duration: u16,
    /// Bitrate in kbps.
    pub bitrate: u32,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Bits per sample.
    pub sample_depth: u16,
    /// File size in bytes. A load needs this and no browse menu shows it (F31).
    pub file_size: u32,
    /// Track number within its album.
    pub track_number: u32,
    /// Disc number.
    pub disc_number: u16,
    /// Release year.
    pub year: u16,
    /// Rating, 0–5 stars.
    pub rating: u8,
    /// Times played.
    pub play_count: u16,
    /// The container the audio is stored in (F34). A player takes this at face
    /// value, so the wrong one makes it fetch the file and refuse to decode it.
    pub container: Container,

    /// Artist row id, or zero.
    pub artist_id: u32,
    /// Album row id, or zero.
    pub album_id: u32,
    /// Genre row id, or zero.
    pub genre_id: u32,
    /// Key row id, or zero.
    pub key_id: u32,
    /// Label row id, or zero.
    pub label_id: u32,
    /// Colour row id, or zero.
    pub color_id: u8,
    /// Artwork row id, or zero. A menu item must carry this or the player never
    /// asks for the image.
    pub artwork_id: u32,
    /// Composer's artist row id, or zero.
    pub composer_id: u32,
    /// Original artist's row id, or zero.
    pub original_artist_id: u32,
    /// Remixer's artist row id, or zero.
    pub remixer_id: u32,
}

impl Track {
    /// Tempo in BPM. Stored as an integer ×100 so the format needs no floats.
    pub fn bpm(&self) -> f64 {
        f64::from(self.tempo) / 100.0
    }

    /// Playing time as `m:ss`.
    pub fn duration_text(&self) -> String {
        let minutes = self.duration / 60;
        let seconds = self.duration % 60;
        format!("{minutes}:{seconds:02}")
    }

    /// The `.EXT` companion of [`Track::analyze_path`], by extension swap.
    ///
    /// The pdb records only the `.DAT`; the `.EXT` beside it is found by
    /// swapping the extension, and there is no field pointing at it.
    ///
    /// `None` when the track has no analysis path or its last component has no
    /// extension — an empty string here would be indistinguishable from the
    /// filesystem root, and asking a deck for `/` is not a better failure.
    pub fn analyze_ext_path(&self) -> Option<String> {
        let (stem, extension) = self.analyze_path.rsplit_once('.')?;
        // Only the final component's extension counts: a directory with a dot
        // in its name must not truncate the path.
        if extension.contains('/') {
            return None;
        }
        Some(format!("{stem}.EXT"))
    }
}

/// A playlist or a folder in the playlist tree.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Playlist {
    /// This node's row id.
    pub id: u32,
    /// The name shown when browsing.
    pub name: String,
    /// Id of the folder this sits in, or zero at the root.
    pub parent_id: u32,
    /// Whether this is a folder rather than a playlist.
    pub is_folder: bool,
    /// Where this sorts among its siblings.
    pub sort_order: u32,
    /// The tracks, in playlist order.
    pub track_ids: Vec<u32>,
    /// Ids of the nodes inside this folder, in sort order.
    pub children: Vec<u32>,
}

impl Playlist {
    /// How many tracks the playlist holds.
    pub fn track_count(&self) -> usize {
        self.track_ids.len()
    }
}

/// One history playlist, as a player records it on each mount.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HistoryPlaylist {
    /// This playlist's row id.
    pub id: u32,
    /// The name, `HISTORY 001` and so on.
    pub name: String,
    /// The tracks, in play order.
    pub track_ids: Vec<u32>,
}

/// Everything on one medium, assembled from its `export.pdb`.
///
/// Maps are `BTreeMap`s so iteration order is the row-id order rather than a
/// hash order — a browse menu that changes order between runs is a bug that is
/// hard to see and easy to avoid.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Library {
    /// Tracks by row id.
    pub tracks: BTreeMap<u32, Track>,
    /// Playlists and folders by row id.
    pub playlists: BTreeMap<u32, Playlist>,
    /// History playlists by row id.
    pub history: BTreeMap<u32, HistoryPlaylist>,
    /// Artist names by row id.
    pub artists: BTreeMap<u32, String>,
    /// Album names by row id.
    pub albums: BTreeMap<u32, String>,
    /// Genre names by row id.
    pub genres: BTreeMap<u32, String>,
    /// Key names by row id.
    pub keys: BTreeMap<u32, String>,
    /// Label names by row id.
    pub labels: BTreeMap<u32, String>,
    /// Colour names by row id.
    pub colors: BTreeMap<u8, String>,
    /// Artwork paths by row id.
    pub artwork: BTreeMap<u32, String>,
    /// A cache key that survives the player's own bookkeeping writes (F13).
    pub digest: StableDigest,
}

impl Library {
    /// Read a whole library from the bytes of an `export.pdb`.
    pub fn parse(data: &[u8]) -> Result<Self> {
        Ok(Self::from_pdb(&Pdb::new(data)?))
    }

    /// Join an already-parsed database.
    pub fn from_pdb(pdb: &Pdb<'_>) -> Self {
        let artists = names(pdb.rows::<ArtistRow>(), |row| (row.id, row.name.text));
        let albums = names(pdb.rows::<AlbumRow>(), |row| (row.id, row.name.text));
        let genres = names(pdb.rows::<GenreRow>(), |row| (row.id, row.name.text));
        let keys = names(pdb.rows::<KeyRow>(), |row| (row.id, row.name.text));
        let labels = names(pdb.rows::<LabelRow>(), |row| (row.id, row.name.text));
        let colors = names(pdb.rows::<ColorRow>(), |row| (row.id, row.name.text));
        let artwork = names(pdb.rows::<ArtworkRow>(), |row| (row.id, row.path.text));

        let mut library = Self {
            digest: pdb.stable_digest(),
            tracks: BTreeMap::new(),
            playlists: BTreeMap::new(),
            history: BTreeMap::new(),
            artists,
            albums,
            genres,
            keys,
            labels,
            colors,
            artwork,
        };

        for row in pdb.rows::<TrackRow>() {
            let track = library.join_track(row);
            library.tracks.insert(track.id, track);
        }

        for row in pdb.rows::<PlaylistTreeRow>() {
            library.playlists.insert(
                row.id,
                Playlist {
                    id: row.id,
                    name: row.name.text,
                    parent_id: row.parent_id,
                    is_folder: row.node_is_folder != 0,
                    sort_order: row.sort_order,
                    track_ids: Vec::new(),
                    children: Vec::new(),
                },
            );
        }

        // Entries carry an explicit index; the on-disk row order is not the
        // playlist order.
        let mut entries = pdb.rows::<PlaylistEntryRow>();
        entries.sort_by_key(|row| (row.playlist_id, row.entry_index));
        for entry in entries {
            if let Some(playlist) = library.playlists.get_mut(&entry.playlist_id) {
                playlist.track_ids.push(entry.track_id);
            }
        }

        library.link_children();

        for row in pdb.rows::<HistoryPlaylistRow>() {
            library.history.insert(
                row.id,
                HistoryPlaylist {
                    id: row.id,
                    name: row.name.text,
                    track_ids: Vec::new(),
                },
            );
        }
        let mut history_entries = pdb.rows::<HistoryEntryRow>();
        history_entries.sort_by_key(|row| (row.playlist_id, row.entry_index));
        for entry in history_entries {
            if let Some(playlist) = library.history.get_mut(&entry.playlist_id) {
                playlist.track_ids.push(entry.track_id);
            }
        }

        library
    }

    fn join_track(&self, row: TrackRow) -> Track {
        let name = |map: &BTreeMap<u32, String>, id: u32| map.get(&id).cloned().unwrap_or_default();
        Track {
            id: row.id,
            title: row.strings.title.text,
            artist: name(&self.artists, row.artist_id),
            album: name(&self.albums, row.album_id),
            genre: name(&self.genres, row.genre_id),
            key: name(&self.keys, row.key_id),
            label: name(&self.labels, row.label_id),
            composer: name(&self.artists, row.composer_id),
            original_artist: name(&self.artists, row.original_artist_id),
            remixer: name(&self.artists, row.remixer_id),
            color: self.colors.get(&row.color_id).cloned().unwrap_or_default(),
            comment: row.strings.comment.text,
            isrc: row.strings.isrc.text,
            date_added: row.strings.date_added.text,
            release_date: row.strings.release_date.text,
            mix_name: row.strings.mix_name.text,
            file_path: row.strings.file_path.text,
            filename: row.strings.filename.text,
            analyze_path: row.strings.analyze_path.text,
            artwork_path: name(&self.artwork, row.artwork_id),
            tempo: row.tempo,
            duration: row.duration,
            bitrate: row.bitrate,
            sample_rate: row.sample_rate,
            sample_depth: row.sample_depth,
            file_size: row.file_size,
            track_number: row.track_number,
            disc_number: row.disc_number,
            year: row.year,
            rating: row.rating,
            play_count: row.play_count,
            container: row.container,
            artist_id: row.artist_id,
            album_id: row.album_id,
            genre_id: row.genre_id,
            key_id: row.key_id,
            label_id: row.label_id,
            color_id: row.color_id,
            artwork_id: row.artwork_id,
            composer_id: row.composer_id,
            original_artist_id: row.original_artist_id,
            remixer_id: row.remixer_id,
        }
    }

    /// Attach each node to its parent, in rekordbox's own sort order.
    fn link_children(&mut self) {
        let mut order: Vec<(u32, u32, String, u32)> = self
            .playlists
            .values()
            .map(|node| (node.parent_id, node.sort_order, node.name.clone(), node.id))
            .collect();
        order.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
        for (parent_id, _, _, id) in order {
            if parent_id == id {
                continue;
            }
            if let Some(parent) = self.playlists.get_mut(&parent_id) {
                parent.children.push(id);
            }
        }
    }

    /// The top-level playlists and folders, in rekordbox's own sort order.
    ///
    /// A node whose parent is not in the tree is a root, which is how rekordbox
    /// marks the top level — the parent id there is zero and no playlist has id
    /// zero.
    pub fn root_playlists(&self) -> Vec<&Playlist> {
        let mut roots: Vec<&Playlist> = self
            .playlists
            .values()
            .filter(|node| !self.playlists.contains_key(&node.parent_id))
            .collect();
        roots.sort_by(|a, b| (a.sort_order, &a.name).cmp(&(b.sort_order, &b.name)));
        roots
    }

    /// The tracks of one playlist, in playlist order, skipping any whose row is
    /// missing.
    pub fn playlist_tracks(&self, playlist_id: u32) -> Vec<&Track> {
        self.playlists
            .get(&playlist_id)
            .into_iter()
            .flat_map(|playlist| playlist.track_ids.iter())
            .filter_map(|id| self.tracks.get(id))
            .collect()
    }

    /// Every track, sorted by artist then title, case-insensitively.
    pub fn track_list(&self) -> Vec<&Track> {
        let mut tracks: Vec<&Track> = self.tracks.values().collect();
        tracks.sort_by_key(|track| {
            (
                track.artist.to_lowercase(),
                track.title.to_lowercase(),
                track.id,
            )
        });
        tracks
    }

    /// Tracks whose title, artist or album contains `term`, case-insensitively.
    pub fn search(&self, term: &str) -> Vec<&Track> {
        let needle = term.to_lowercase();
        self.track_list()
            .into_iter()
            .filter(|track| {
                track.title.to_lowercase().contains(&needle)
                    || track.artist.to_lowercase().contains(&needle)
                    || track.album.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// Counts a media query has to answer with truthfully, or a deck will not
    /// list the medium at all (F24).
    pub fn summary(&self) -> Summary {
        Summary {
            tracks: self.tracks.len(),
            artists: self.artists.len(),
            albums: self.albums.len(),
            genres: self.genres.len(),
            keys: self.keys.len(),
            playlists: self.playlists.values().filter(|p| !p.is_folder).count(),
            folders: self.playlists.values().filter(|p| p.is_folder).count(),
        }
    }
}

/// What a media query reports about a medium.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Summary {
    /// Tracks.
    pub tracks: usize,
    /// Artists.
    pub artists: usize,
    /// Albums.
    pub albums: usize,
    /// Genres.
    pub genres: usize,
    /// Keys.
    pub keys: usize,
    /// Playlists, folders excluded.
    pub playlists: usize,
    /// Folders.
    pub folders: usize,
}

fn names<Row, Id, F>(rows: Vec<Row>, extract: F) -> BTreeMap<Id, String>
where
    F: Fn(Row) -> (Id, String),
    Id: Ord,
{
    rows.into_iter().map(extract).collect()
}
