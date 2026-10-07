// SPDX-License-Identifier: GPL-3.0-only

//! The browse surface: every list a deck can reach behind the LINK button.
//!
//! One function, [`build`], turns a menu request into the rows it should
//! produce. Nothing here does I/O and nothing here holds state; the result set
//! it returns is what the connection remembers and pages through.
//!
//! # An error and an empty folder look the same on a deck
//!
//! So the set of requests answered here is a **user-visible surface** rather
//! than an internal detail (F40), and it is settled by what a real CDJ-2000NXS
//! actually asks for. Every request type in `S20-browse-ground-truth`,
//! `S18-two-slots`, `S19`, `S22` and `S23` is answered: the eleven root
//! categories we serve, the thirteen drill types the grid generates, all twelve
//! sorts, search, metadata and track info.
//!
//! `FOLDER` is the one root category deliberately omitted. It browses
//! *unanalysed* files by directory with a track-type-2 descriptor, and a
//! rekordbox export does not describe that tree — offering a category we would
//! answer empty is worse than not offering it.
//!
//! # Four places the reference implementation was wrong
//!
//! Each was settled by reading a real deck's replies out of the corpus rather
//! than by reasoning:
//!
//! - **The DEFAULT sort of a track list is by title.** The reference used the
//!   library's artist-then-title order. A real deck answering `MENU_TRACK` with
//!   sort 0 returns `Acidité, Acid Lunch, Acid Storm, ACXD, Add Some More` —
//!   title order, whatever the artist. Inside a playlist DEFAULT still means
//!   the curated order, which is what a playlist is for.
//! - **Argument 9 of a track row is a position and it is not always zero.**
//!   In a plain list it is the track's number within its album (`ACXD` came
//!   back as 23); inside a playlist or a history list it is the 1-based
//!   position in that list.
//! - **Search returns three kinds of row, not tracks.** Matching artists
//!   first, then albums, then tracks, with argument 0 carrying `1`, `2` and `3`
//!   respectively — which is how the deck knows to answer a click on one of
//!   them with an ARTIST drill rather than a load.
//! - **The BITRATE listing is descending.** 2116, 320, 256, 224, 192, 160 off
//!   a real deck; the reference sorted ascending.
//!
//! # A category lists the rows a track references, not the table
//!
//! A rekordbox medium carries artist rows nothing points at — they survive a
//! track being removed, and they arrive through `original_artist_id`,
//! `remixer_id` and `composer_id`, which no track list browses by. Listing the
//! whole table puts them in the ARTIST menu where each opens onto nothing.
//! Confirmed against hardware: in `S20` one CDJ-2000NXS asked another for the
//! ARTIST menu of a medium whose table holds 329 rows and was answered **290**,
//! exactly the number a track references.

use std::collections::BTreeMap;

use prolink_proto::dbserver::{
    Arguments, CamelotKey, Drill, FILTER_ALL, ItemType, METADATA_ITEMS, MenuItem, MessageKind,
    ROOT_CATEGORIES, SORT_MENU, SortOrder, TRACK_INFO_ITEMS,
};
use prolink_rekordbox::{Library, Track};

use super::keys::{self, TOLERANCES, Wheel};
use crate::serve::Medium;

/// The root categories we serve, in the order a real player serves them.
///
/// `ROOT_CATEGORIES` is all twelve; this is the eleven we can answer. The ids
/// and item types are *listed* rather than derived, twice over: two separate
/// derivations each looked right and each had an exception (F26, F40, F43), so
/// the table lives in `prolink-proto` and this is only the subset.
pub(super) const SERVED: [&str; 11] = [
    "PLAYLIST",
    "ALBUM",
    "GENRE",
    "LABEL",
    "ARTIST",
    "BITRATE",
    "DATE ADDED",
    "TRACK",
    "HISTORY",
    "SEARCH",
    "KEY",
];

/// The request type a deck sends for the DATE ADDED category.
///
/// **Never observed.** All eleven other categories have been watched from root
/// item to request type in the corpus; DATE ADDED has not, in either direction
/// — no deck-to-deck session opened it, and the four sessions that browsed our
/// own server's DATE ADDED row produced no request at all. `0x1010` is the
/// pre-hardware literature's `MENU_TIME` and what both reference
/// implementations answer, so it is what we answer *(unknown)*.
///
/// The cost of being wrong is bounded: an unrecognised request is answered with
/// an empty result set rather than an error (F25), so a wrong guess makes DATE
/// ADDED look like an empty category — which is exactly what omitting it would
/// look like anyway.
const MENU_DATE_ADDED: MessageKind = MessageKind::MENU_TIME;

/// Where a track row's argument 9 comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Position {
    /// The track's number within its album, which is what a plain list carries.
    TrackNumber,
    /// The 1-based position in the list being served, which is what a playlist
    /// and a history list carry.
    InList,
}

/// Turn a menu request into the rows it should produce.
///
/// `None` means "this is not a menu request", which the caller answers with a
/// bare acknowledgement — **never** with an error (F25). An empty `Vec` is a
/// different thing: a menu that exists and has no rows.
///
/// `medium` is `None` when the descriptor named a slot we do not serve, which
/// is an empty list rather than a refusal: the deck asked about a medium that
/// is simply not there.
pub(super) fn build(
    kind: MessageKind,
    args: &Arguments,
    medium: Option<&Medium>,
    tags: &[u32],
) -> Option<Vec<MenuItem>> {
    // A phantom medium answers about the track a player already has and about
    // nothing else.
    //
    // The stick is out and only what was cached before it went can still be
    // served, so a menu that offered anything would be offering tracks we
    // cannot produce -- and a DJ would find out by loading one and hearing
    // nothing. An empty list says "there is nothing here", which is true, and
    // it is a state every deck already handles.
    //
    // The metadata requests are exempt because a deck polls them for the track
    // it is *playing*, every couple of seconds, for as long as it plays. Those
    // are the ones that must keep answering; see Medium::go_phantom.
    if let Some(medium) = medium
        && medium.is_phantom()
        && !is_about_one_track(kind)
    {
        // The root and the sort options are emptied too. They are answered
        // without reference to a medium everywhere else, but they are also the
        // way *in* -- leaving them populated would offer a DJ a menu that
        // turned out to be empty one level down, which reads as a fault rather
        // than as a medium that has gone.
        return (is_menu(kind)
            || matches!(
                kind,
                MessageKind::MENU_ROOT | MessageKind::MENU_SORT | MessageKind::MENU_TAG_LIST
            ))
        .then(Vec::new);
    }

    // These two are answered identically whatever the medium, and `MENU_SORT`
    // is answered identically whatever argument 2 names.
    match kind {
        MessageKind::MENU_ROOT => return Some(root()),
        MessageKind::MENU_SORT => return Some(sort_options()),
        _ => {}
    }

    let Some(medium) = medium else {
        return is_menu(kind).then(Vec::new);
    };
    let library = medium.library();
    // Argument 1 is the sort on a track list, a playlist, a drill and a search.
    let sort = SortOrder(args.number(1).unwrap_or(SortOrder::DEFAULT.0));

    let items = match kind {
        MessageKind::MENU_TRACK => track_items(
            sorted(all_tracks(library), sort),
            sort,
            Position::TrackNumber,
        ),
        MessageKind::MENU_ARTIST => named(library, Filter::Artist),
        MessageKind::MENU_ALBUM => named(library, Filter::Album),
        MessageKind::MENU_GENRE => named(library, Filter::Genre),
        MessageKind::MENU_LABEL => named(library, Filter::Label),
        MessageKind::MENU_KEY => named(library, Filter::Key),
        MessageKind::MENU_BITRATE => named(library, Filter::Bitrate),
        MENU_DATE_ADDED => named(library, Filter::DateAdded),
        MessageKind::MENU_HISTORY => history(library),
        // Argument 1 is a sort here as it is on a plain track list — a deck
        // browsing with KEY selected asks for the tag list with `0x0c` (F54).
        // DEFAULT keeps tag order, which is what the DJ built.
        MessageKind::MENU_TAG_LIST => tag_list(library, tags, sort),
        MessageKind::MENU_PLAYLIST => playlist(library, args, sort),
        MessageKind::MENU_SEARCH => search(library, args.text(3).unwrap_or_default()),
        MessageKind::GET_METADATA | MessageKind::GET_GENERIC_METADATA => {
            metadata(library, args.number(1).unwrap_or(0))
        }
        MessageKind::GET_TRACK_INFO => track_info(library, args.number(1).unwrap_or(0)),
        _ => return Drill::parse(kind).and_then(|drill| drilled(drill, args, library, sort)),
    };
    Some(items)
}

/// Mark one row on its way out: tagged, and the track the deck has loaded.
///
/// **At render time, not build time.** A result set is a snapshot of the
/// library; these two bits are live state that changes while the deck is still
/// paging through it. A deck loads a track *from* the list it is scrolling and
/// then keeps scrolling without re-issuing the menu request (F27), so a mark
/// applied when the set was built is a mark the deck never sees — which is
/// exactly why the key-matching indicator stayed dark with the row otherwise
/// byte-perfect (F55).
pub(super) fn mark(item: &MenuItem, tags: &[u32], playing: Option<u32>) -> MenuItem {
    let mut item = item.clone();
    if item.flags & MenuItem::TRACK_FLAGS == 0 {
        return item;
    }
    if playing == Some(item.id) {
        item.flags |= MenuItem::LOADED;
    }
    if tags.contains(&item.id) {
        item.flags |= MenuItem::TAGGED;
    }
    item
}

/// Whether a request asks about **one track the deck already has**, rather than
/// offering a list of things it could load.
///
/// The distinction only matters for a phantom medium, and there it matters a
/// great deal: a deck polls these every couple of seconds for as long as it is
/// playing, so answering them empty would blank the display of a track that is
/// still playing perfectly well.
fn is_about_one_track(kind: MessageKind) -> bool {
    matches!(
        kind,
        MessageKind::GET_METADATA | MessageKind::GET_GENERIC_METADATA | MessageKind::GET_TRACK_INFO
    )
}

/// Whether `kind` is a menu request at all, used only to decide what an
/// unserved slot answers with.
fn is_menu(kind: MessageKind) -> bool {
    matches!(
        kind,
        MessageKind::MENU_TRACK
            | MessageKind::MENU_ARTIST
            | MessageKind::MENU_ALBUM
            | MessageKind::MENU_GENRE
            | MessageKind::MENU_LABEL
            | MessageKind::MENU_KEY
            | MessageKind::MENU_BITRATE
            | MENU_DATE_ADDED
            | MessageKind::MENU_HISTORY
            | MessageKind::MENU_PLAYLIST
            | MessageKind::MENU_SEARCH
            | MessageKind::GET_METADATA
            | MessageKind::GET_GENERIC_METADATA
            | MessageKind::GET_TRACK_INFO
    ) || Drill::parse(kind).is_some()
}

// -- the two fixed menus --------------------------------------------------

/// The root category list: the subset of [`ROOT_CATEGORIES`] we can answer.
///
/// Three details are copied from a real player rather than invented, because a
/// deck renders wrong ones perfectly and then declines to *open* the category,
/// which reads as a category that exists and is empty (F26): argument 1 carries
/// a per-category id and not zero, the label is wrapped in U+FFFA/U+FFFB, and
/// argument 7 is zero rather than the `0x01000000` a track row carries.
fn root() -> Vec<MenuItem> {
    ROOT_CATEGORIES
        .iter()
        .filter(|category| SERVED.contains(&category.label))
        .map(|category| category.to_item())
        .collect()
}

/// The twelve sort orders, which are the same twelve whatever menu is being
/// sorted.
fn sort_options() -> Vec<MenuItem> {
    SORT_MENU.iter().map(|option| option.to_item()).collect()
}

// -- the drill-down grid --------------------------------------------------

/// What one level of a drill narrows by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Filter {
    /// Genre row id.
    Genre,
    /// Artist row id.
    Artist,
    /// Album row id.
    Album,
    /// Record label row id.
    Label,
    /// Key row id.
    Key,
    /// The bitrate itself, which is its own id.
    Bitrate,
    /// The date added, packed as `YYYYMMDD`.
    DateAdded,
}

impl Filter {
    /// The value this level reads off a track.
    fn value(self, track: &Track) -> u32 {
        match self {
            Self::Genre => track.genre_id,
            Self::Artist => track.artist_id,
            Self::Album => track.album_id,
            Self::Label => track.label_id,
            Self::Key => track.key_id,
            Self::Bitrate => track.bitrate,
            Self::DateAdded => packed_date(&track.date_added),
        }
    }

    /// The label a row at this level carries.
    ///
    /// Empty for BITRATE, whose value *is* its label and which the deck formats
    /// itself — confirmed against a real deck, which sends both labels empty
    /// and the number in argument 1.
    fn label(self, track: &Track, library: &Library) -> String {
        match self {
            Self::Genre => library.genres.get(&track.genre_id).cloned(),
            Self::Artist => library.artists.get(&track.artist_id).cloned(),
            Self::Album => library.albums.get(&track.album_id).cloned(),
            Self::Label => library.labels.get(&track.label_id).cloned(),
            Self::Key => library.keys.get(&track.key_id).cloned(),
            Self::Bitrate => Some(String::new()),
            Self::DateAdded => Some(track.date_added.clone()),
        }
        .unwrap_or_default()
    }

    /// The item type a row at this level carries.
    fn item_type(self) -> ItemType {
        match self {
            Self::Genre => ItemType::GENRE,
            Self::Artist => ItemType::ARTIST,
            Self::Album => ItemType::ALBUM,
            Self::Label => ItemType::LABEL,
            Self::Key => ItemType::KEY,
            Self::Bitrate => ItemType::BITRATE,
            Self::DateAdded => ItemType::DATE_ADDED,
        }
    }

    /// Whether a row at this level needs a name to be worth showing.
    ///
    /// BITRATE and DATE ADDED are their own labels, so a row with no name in a
    /// side table is not a broken row there.
    fn needs_name(self) -> bool {
        !matches!(self, Self::Bitrate | Self::DateAdded)
    }

    /// How rows at this level are ordered.
    fn order(self, id: u32, label: &str) -> (String, i64) {
        match self {
            // Wheel order rather than text order — the one deliberate
            // difference from the hardware (§8).
            Self::Key => (keys::sort_text(label), 0),
            // Newest first, and highest first: both are what a browsing DJ
            // wants at the top, and the descending BITRATE listing is what a
            // real deck sends.
            Self::Bitrate | Self::DateAdded => (String::new(), -i64::from(id)),
            _ => (label.to_lowercase(), 0),
        }
    }
}

/// The flat menu a drill's category byte names.
///
/// `Drill` carries the low byte of the *flat* menu's request type, so putting
/// `0x1000` back in front of it recovers the menu being drilled into: `0x14`
/// is KEY here, which is `MENU_KEY` `0x1014` and **not** the root-item id
/// `0x0c` (F40, F42).
fn flat_menu(category: u8) -> MessageKind {
    MessageKind(0x1000 | u16::from(category))
}

/// Which filter each level of a category's drill narrows by.
///
/// The chains are read off a real session: GENRE narrows to an artist, then an
/// album, then tracks; ARTIST skips straight to albums; ALBUM straight to
/// tracks. The last entry is the level that yields tracks.
///
/// `None` for a category with no chain — including the two that are their own
/// shape, KEY (a harmonic tolerance first, F44) and HISTORY (a playlist, in
/// play order), which [`drilled`] handles before reaching here.
fn chain(category: u8) -> Option<&'static [Filter]> {
    Some(match flat_menu(category) {
        MessageKind::MENU_GENRE => &[Filter::Genre, Filter::Artist, Filter::Album],
        MessageKind::MENU_ARTIST => &[Filter::Artist, Filter::Album],
        MessageKind::MENU_ALBUM => &[Filter::Album],
        MessageKind::MENU_LABEL => &[Filter::Label, Filter::Artist, Filter::Album],
        MessageKind::MENU_BITRATE => &[Filter::Bitrate],
        MENU_DATE_ADDED => &[Filter::DateAdded],
        _ => return None,
    })
}

/// Rows `depth` levels into a category, narrowed by one filter id per level.
///
/// Argument 1 is the sort here too, so a list reached by drilling sorts like
/// any other — LABEL → artist → album → tracks was the one place sorting still
/// did nothing.
fn drilled(
    drill: Drill,
    args: &Arguments,
    library: &Library,
    sort: SortOrder,
) -> Option<Vec<MenuItem>> {
    let depth = usize::from(drill.depth);
    // One filter per level, from argument 2. A missing one means "do not
    // narrow", which is what the ALL row sends anyway.
    let filters: Vec<u32> = (0..depth)
        .map(|level| args.number(2 + level).unwrap_or(FILTER_ALL))
        .collect();
    let first = filters.first().copied().unwrap_or(FILTER_ALL);

    // The category byte is the *flat* menu's request type's low byte, which is
    // a different numbering from the root-item id — KEY is `0x14` here and
    // `0x0c` there, and conflating them is exactly how F40's bug happened.
    match flat_menu(drill.category) {
        MessageKind::MENU_KEY => return Some(key_drill(library, depth, &filters, sort)),
        MessageKind::MENU_HISTORY => return Some(history_tracks(library, first, sort)),
        _ => {}
    }

    let chain = chain(drill.category)?;
    let tracks = narrowed(library, chain, &filters);

    let Some(&next) = chain.get(depth) else {
        return Some(track_items(
            sorted(tracks, sort),
            sort,
            Position::TrackNumber,
        ));
    };

    let mut items = rows(&tracks, next, library);
    if items.is_empty() {
        // A level with nothing to show is a dead end, and on a real medium it
        // happens: a track with no album belongs to no entry in an album list,
        // so ARTIST → albums comes back empty and that artist's music is
        // unreachable. Skip the level rather than show a blank screen.
        return Some(track_items(
            sorted(tracks, sort),
            sort,
            Position::TrackNumber,
        ));
    }
    // A real reply heads a filtered list with ALL, but **only when there is
    // more than one entry**: a single-entry level goes out bare. Also when some
    // of these tracks have no value at this level, since ALL is then the only
    // row that reaches them.
    if items.len() > 1 || unclassified(&tracks, next, library) {
        items.insert(0, MenuItem::all());
    }
    Some(items)
}

/// The tracks left after applying one filter id per level.
fn narrowed<'a>(library: &'a Library, chain: &[Filter], filters: &[u32]) -> Vec<&'a Track> {
    library
        .tracks
        .values()
        .filter(|track| {
            chain
                .iter()
                .zip(filters)
                .all(|(filter, &value)| value == FILTER_ALL || filter.value(track) == value)
        })
        .collect()
}

/// Whether any of `tracks` would be unreachable below a level narrowed by
/// `filter`.
fn unclassified(tracks: &[&Track], filter: Filter, library: &Library) -> bool {
    tracks.iter().any(|track| {
        filter.value(track) == 0 || (filter.needs_name() && filter.label(track, library).is_empty())
    })
}

/// The distinct values `tracks` carry at one level, as menu rows.
fn rows(tracks: &[&Track], filter: Filter, library: &Library) -> Vec<MenuItem> {
    let mut seen: BTreeMap<u32, String> = BTreeMap::new();
    for track in tracks {
        let id = filter.value(track);
        if id == 0 {
            continue;
        }
        let label = filter.label(track, library);
        if filter.needs_name() && label.is_empty() {
            continue;
        }
        seen.entry(id).or_insert(label);
    }
    ordered(seen, filter)
}

/// Every value the whole library carries at one level, as menu rows.
///
/// This is the flat category listing — ARTIST, ALBUM, GENRE, LABEL, KEY,
/// BITRATE, DATE ADDED — and it lists only the rows a track references. See the
/// module documentation for the 290-of-329 measurement that settles it.
fn named(library: &Library, filter: Filter) -> Vec<MenuItem> {
    let tracks: Vec<&Track> = library.tracks.values().collect();
    rows(&tracks, filter, library)
}

/// Sort collected rows the way their level is ordered, and build the items.
fn ordered(seen: BTreeMap<u32, String>, filter: Filter) -> Vec<MenuItem> {
    let mut sorted: Vec<(String, i64, u32, String)> = seen
        .into_iter()
        .map(|(id, label)| {
            let (text, number) = filter.order(id, &label);
            (text, number, id, label)
        })
        .collect();
    sorted.sort();
    sorted
        .into_iter()
        .map(|(_, _, id, label)| MenuItem::named(id, filter.item_type(), &label))
        .collect()
}

/// KEY's extra level: a harmonic tolerance, and only then tracks (F44).
///
/// Choosing a key does not list its tracks. A real player offers three widening
/// matches — the key alone, plus its relative, plus the adjacent wheel
/// positions — all carrying the same key id and differing only in argument 0,
/// and `0x1214` then takes `(key id, tolerance)` and returns tracks.
fn key_drill(library: &Library, depth: usize, filters: &[u32], sort: SortOrder) -> Vec<MenuItem> {
    let key_id = filters.first().copied().unwrap_or(0);
    let Some(chosen) = library
        .keys
        .get(&key_id)
        .and_then(|name| Wheel::parse(name))
    else {
        // A key this table cannot place: an exact match, which is at least
        // correct, rather than inventing neighbours.
        let tracks = library
            .tracks
            .values()
            .filter(|track| track.key_id == key_id)
            .collect();
        return track_items(sorted(tracks, sort), sort, Position::TrackNumber);
    };

    // Every key the library holds, by wheel spot, so a tolerance can be turned
    // back into the ids and names it covers.
    let placed: BTreeMap<Wheel, (u32, &str)> = library
        .keys
        .iter()
        .filter_map(|(&id, name)| Wheel::parse(name).map(|spot| (spot, (id, name.as_str()))))
        .collect();

    if depth <= 1 {
        return (0..TOLERANCES)
            .map(|tolerance| {
                let names: Vec<&str> = keys::harmonic_set(chosen, tolerance)
                    .into_iter()
                    .filter_map(|spot| placed.get(&spot).map(|&(_, name)| name))
                    .collect();
                let mut item = MenuItem::named(key_id, ItemType::KEY, &names.join(", "));
                // The tolerance travels in argument 0, which is what makes the
                // three rows distinguishable when the deck sends one back.
                item.argument0 = tolerance;
                item
            })
            .collect();
    }

    let tolerance = filters.get(1).copied().unwrap_or(0);
    let wanted: Vec<u32> = keys::harmonic_set(chosen, tolerance)
        .into_iter()
        .filter_map(|spot| placed.get(&spot).map(|&(id, _)| id))
        .collect();
    let tracks = library
        .tracks
        .values()
        .filter(|track| wanted.contains(&track.key_id))
        .collect();
    track_items(sorted(tracks, sort), sort, Position::TrackNumber)
}

// -- history --------------------------------------------------------------

/// The history playlists, newest first.
///
/// A real deck answers `HISTORY 002` before `HISTORY 001`. Empty on a stick
/// written straight from rekordbox and never played anywhere, which is ordinary.
fn history(library: &Library) -> Vec<MenuItem> {
    library
        .history
        .values()
        .rev()
        .map(|playlist| MenuItem::named(playlist.id, ItemType::HISTORY_PLAYLIST, &playlist.name))
        .collect()
}

/// One history playlist's tracks, in play order.
fn history_tracks(library: &Library, playlist_id: u32, sort: SortOrder) -> Vec<MenuItem> {
    let tracks: Vec<&Track> = library
        .history
        .get(&playlist_id)
        .into_iter()
        .flat_map(|playlist| playlist.track_ids.iter())
        .filter_map(|id| library.tracks.get(id))
        .collect();
    // DEFAULT keeps the order the tracks were played in, which is the whole
    // point of a history list.
    let tracks = match sort {
        SortOrder::DEFAULT => tracks,
        _ => sorted(tracks, sort),
    };
    track_items(tracks, sort, Position::InList)
}

/// The tracks one deck has tagged.
///
/// `DEFAULT` keeps the order the DJ tagged in, the way a history list keeps
/// play order; any other sort is applied as it would be to a track list. A
/// real deck's reply looks alphabetical only because the tracks were tagged
/// that way, and its exact collation is unresolved — it orders "antidepressant
/// o44" before "Anti Gravity Racing", which no space-respecting comparison
/// does (F53).
///
/// An id that names nothing is dropped rather than served as a blank row: the
/// medium can be swapped under a tag list this server is still holding, and a
/// row the deck cannot load is worse than a shorter list.
fn tag_list(library: &Library, tags: &[u32], sort: SortOrder) -> Vec<MenuItem> {
    let tracks: Vec<&Track> = tags
        .iter()
        .filter_map(|id| library.tracks.get(id))
        .collect();
    let tracks = match sort {
        SortOrder::DEFAULT => tracks,
        _ => sorted(tracks, sort),
    };
    track_items(tracks, sort, Position::InList)
}

// -- playlists ------------------------------------------------------------

/// The playlist tree, or one playlist's tracks.
///
/// Arguments are `[descriptor, sort, playlist id, is folder]`. A real deck
/// lists **folders before playlists**, and argument 9 of each row is the node's
/// own sort order rather than its position in the listing — read off two
/// independent deck-to-deck captures, which agree.
fn playlist(library: &Library, args: &Arguments, sort: SortOrder) -> Vec<MenuItem> {
    let playlist_id = args.number(2).unwrap_or(0);
    let is_folder = args.number(3).unwrap_or(0) != 0;

    if is_folder {
        let mut children: Vec<_> = library
            .playlists
            .values()
            .filter(|node| match playlist_id {
                // A node whose parent is not in the tree is a root, which is
                // how rekordbox marks the top level.
                0 => !library.playlists.contains_key(&node.parent_id),
                parent => node.parent_id == parent,
            })
            .collect();
        children.sort_by(|a, b| {
            (!a.is_folder, a.sort_order, &a.name).cmp(&(!b.is_folder, b.sort_order, &b.name))
        });
        return children
            .into_iter()
            .map(|node| {
                let item_type = if node.is_folder {
                    ItemType::FOLDER
                } else {
                    ItemType::PLAYLIST
                };
                let mut item = MenuItem::named(node.id, item_type, &node.name);
                item.playlist_position = node.sort_order;
                item
            })
            .collect();
    }

    // Most of a DJ's browsing happens inside a playlist, so ignoring the sort
    // here is ignoring it almost everywhere. DEFAULT keeps the curated order,
    // which is the whole point of a playlist and must not be replaced by an
    // alphabetical one.
    let tracks = library.playlist_tracks(playlist_id);
    let tracks = match sort {
        SortOrder::DEFAULT => tracks,
        _ => sorted(tracks, sort),
    };
    track_items(tracks, sort, Position::InList)
}

// -- search ---------------------------------------------------------------

/// Argument 0 of a search row: which kind of thing it names.
///
/// A real deck's search result is **not** a track list. It is matching artists,
/// then matching albums, then matching tracks, and this byte is how the deck
/// knows that clicking the first opens an ARTIST drill and clicking the last
/// loads a track. Read off `S20-browse-ground-truth`: searching `H` returned
/// artist rows with `1`, searching `HEL` a track row with `3`.
const SEARCH_ARTIST: u32 = 1;
/// A matching album. Inferred from the pair either side of it — the deck
/// answered a click on a middle row with an ALBUM drill *(unknown)*.
const SEARCH_ALBUM: u32 = 2;
/// A matching track.
const SEARCH_TRACK: u32 = 3;

/// Everything matching `term`: artists, then albums, then tracks.
///
/// A deck searches as you type, one request per keystroke, so this runs on
/// every letter.
///
/// Tracks match on their **title** alone. Matching a track by its artist would
/// duplicate the artist row that is already in the list and is already the way
/// to reach that artist's tracks, and the observed result counts are far too
/// small for it.
fn search(library: &Library, term: &str) -> Vec<MenuItem> {
    let needle = term.to_lowercase();
    let matches = |text: &str| text.to_lowercase().contains(&needle);

    let referenced = |ids: &dyn Fn(&Track) -> u32, table: &BTreeMap<u32, String>| {
        let mut found: BTreeMap<String, (String, u32)> = BTreeMap::new();
        for track in library.tracks.values() {
            let id = ids(track);
            let Some(name) = table.get(&id).filter(|name| !name.is_empty()) else {
                continue;
            };
            if matches(name) {
                found.insert(
                    format!("{}\u{0}{id}", name.to_lowercase()),
                    (name.clone(), id),
                );
            }
        }
        found
    };

    let artists = referenced(&|track: &Track| track.artist_id, &library.artists);
    let albums = referenced(&|track: &Track| track.album_id, &library.albums);

    let mut tracks: Vec<&Track> = library
        .tracks
        .values()
        .filter(|track| matches(&track.title))
        .collect();
    tracks.sort_by_key(|track| (track.title.to_lowercase(), track.id));

    let mut items: Vec<MenuItem> = Vec::with_capacity(artists.len() + albums.len() + tracks.len());
    for (group, kind, item_type) in [
        (artists, SEARCH_ARTIST, ItemType::ARTIST),
        (albums, SEARCH_ALBUM, ItemType::ALBUM),
    ] {
        items.extend(group.into_values().map(|(name, id)| {
            let mut item = MenuItem::named(id, item_type, &name);
            item.argument0 = kind;
            item
        }));
    }
    items.extend(tracks.into_iter().map(|track| {
        let mut item = MenuItem::track(
            track.id,
            &track.title,
            "",
            ItemType::TRACK_TITLE,
            track.artwork_id,
        );
        item.argument0 = SEARCH_TRACK;
        item.with_key(CamelotKey::parse(&track.key))
    }));
    items
}

// -- one track ------------------------------------------------------------

/// A track's thirteen metadata items, in a fixed order (§5.9).
///
/// Thirteen, not nine: a player renders whatever it is given and looks entirely
/// correct with colour, date added, bitrate and label missing, which is why the
/// shortfall survived so long (F32). Items go out unconditionally, empty ones
/// included, because the count is what the client pages against.
///
/// Two things a correct reply does that a plausible one does not. Each item
/// carries the id of the row it **references** — the artist item carries the
/// artist's id, which is what lets a player offer "more by this artist" — and
/// the **title item carries the artwork id**, without which a player never
/// requests the image and INFO shows no cover.
fn metadata(library: &Library, track_id: u32) -> Vec<MenuItem> {
    let Some(track) = library.tracks.get(&track_id) else {
        return Vec::new();
    };
    METADATA_ITEMS
        .iter()
        .map(|slot| {
            let (id, label) = match slot.item_type {
                ItemType::TRACK_TITLE => (track.id, track.title.as_str()),
                ItemType::ARTIST => (track.artist_id, track.artist.as_str()),
                ItemType::ALBUM => (track.album_id, track.album.as_str()),
                // Numeric fields carry their value in the id and no label.
                ItemType::DURATION => (u32::from(track.duration), ""),
                ItemType::TEMPO => (track.tempo, ""),
                ItemType::COMMENT => (track.id, track.comment.as_str()),
                ItemType::KEY => (track.key_id, track.key.as_str()),
                ItemType::RATING => (u32::from(track.rating), ""),
                ItemType::COLOR => (u32::from(track.color_id), track.color.as_str()),
                ItemType::GENRE => (track.genre_id, track.genre.as_str()),
                ItemType::DATE_ADDED => (track.id, track.date_added.as_str()),
                ItemType::BITRATE => (track.bitrate, ""),
                _ => (track.label_id, track.label.as_str()),
            };
            let mut item = MenuItem::named(id, slot.item_type, label);
            item.argument0 = slot.argument0;
            if slot.item_type == ItemType::TRACK_TITLE {
                item.flags = MenuItem::TRACK_FLAGS;
                item.artwork_id = track.artwork_id;
            }
            item
        })
        .collect()
}

/// A track's six track-info items (§5.10).
///
/// Six, not one. Returning only the path renders the track and walks it over
/// NFS and is **not enough to load it**: a deck sat at NOW LOADING and then
/// reported that it could not decode the format, having issued no READ at all,
/// so the verdict came from this reply and nowhere else (F31).
///
/// Two traps, and the reference fell into both. **Argument 0 of the path item
/// is the file size** — zero on every other menu item in every capture, which
/// is exactly why it reads as structural padding, and the one thing a load
/// needs that browsing does not. And **item 1 is the container, not the
/// title**: `0x04` means the title in a `GET_METADATA` reply and the container
/// here, so announcing the disc number there makes a disc-2 MP3 call itself
/// AAC (F34, F35).
fn track_info(library: &Library, track_id: u32) -> Vec<MenuItem> {
    let Some(track) = library.tracks.get(&track_id) else {
        return Vec::new();
    };
    TRACK_INFO_ITEMS
        .iter()
        .map(|&item_type| {
            let (argument0, id, label) = match item_type {
                ItemType::TRACK_TITLE => (0, u32::from(track.container.0), ""),
                ItemType::DURATION => (0, u32::from(track.duration), ""),
                ItemType::TEMPO => (0, track.tempo, ""),
                ItemType::COMMENT => (0, track.id, track.comment.as_str()),
                ItemType::PATH => (track.file_size, track.id, track.file_path.as_str()),
                _ => (0, 1, ""),
            };
            let mut item = MenuItem::named(id, item_type, label);
            item.argument0 = argument0;
            item
        })
        .collect()
}

// -- track rows and sorting -----------------------------------------------

/// Every track in the library, unordered.
fn all_tracks(library: &Library) -> Vec<&Track> {
    library.tracks.values().collect()
}

/// Track rows, with the second column the sort selects.
///
/// **The sort selects the item's second column**, and the item type is
/// `(column field type << 8) | 0x04` — so the familiar `0x0704` is not "title
/// and artist" but *a track whose second column is the ARTIST field* (F43).
/// A numeric column sends an **empty** label and puts the raw number in
/// argument 0, which the deck formats itself; a text column sends the text and
/// the referenced row's id.
fn track_items<'a>(
    tracks: impl IntoIterator<Item = &'a Track>,
    sort: SortOrder,
    position: Position,
) -> Vec<MenuItem> {
    tracks
        .into_iter()
        .enumerate()
        .map(|(index, track)| {
            let (argument0, column) = second_column(track, sort);
            let mut item = MenuItem::track(
                track.id,
                &track.title,
                column,
                sort.track_item_type(),
                track.artwork_id,
            );
            item.argument0 = argument0;
            item.playlist_position = match position {
                Position::TrackNumber => track.track_number,
                Position::InList => u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1),
            };
            // The deck draws the key-matching indicator from this. It knows
            // what is playing and we do not, so all a server does is say which
            // key the row is in; sending zero leaves the indicator dark beside
            // every track.
            item.with_key(CamelotKey::parse(&track.key))
        })
        .collect()
}

/// The second column's value and text for one track under one sort.
fn second_column(track: &Track, sort: SortOrder) -> (u32, &str) {
    let column = sort.column().unwrap_or(ItemType::ARTIST);
    let (value, text) = match column {
        ItemType::ALBUM => (track.album_id, track.album.as_str()),
        ItemType::GENRE => (track.genre_id, track.genre.as_str()),
        ItemType::LABEL => (track.label_id, track.label.as_str()),
        ItemType::KEY => (track.key_id, track.key.as_str()),
        // The date column is the one text column whose id is the track's own.
        ItemType::DATE_ADDED => (track.id, track.date_added.as_str()),
        ItemType::TEMPO => (track.tempo, ""),
        ItemType::RATING => (u32::from(track.rating), ""),
        ItemType::BITRATE => (track.bitrate, ""),
        ItemType::PLAY_COUNT => (u32::from(track.play_count), ""),
        _ => (track.artist_id, track.artist.as_str()),
    };
    if sort.column_is_numeric() {
        (value, "")
    } else {
        (value, text)
    }
}

/// Order tracks by a sort order.
///
/// `DEFAULT` is **by title**, not "unsorted" and not the library's own
/// artist-then-title order: a real deck answering `MENU_TRACK` with sort 0
/// returns titles in order whatever their artists. Inside a playlist or a
/// history list the caller does not come here at all, because DEFAULT there
/// means the curated order.
///
/// The direction of the four numeric sorts is a choice, not an observation:
/// where more is better — RATING, DJ PLAY COUNT, DATE ADDED — the largest comes
/// first, and BPM and BITRATE ascend.
fn sorted(tracks: Vec<&Track>, sort: SortOrder) -> Vec<&Track> {
    let mut keyed: Vec<(String, i64, String, u32, &Track)> = tracks
        .into_iter()
        .map(|track| {
            let title = track.title.to_lowercase();
            let (text, number) = match sort {
                SortOrder::ARTIST => (track.artist.to_lowercase(), 0),
                SortOrder::ALBUM => (track.album.to_lowercase(), i64::from(track.track_number)),
                SortOrder::GENRE => (track.genre.to_lowercase(), 0),
                SortOrder::LABEL => (track.label.to_lowercase(), 0),
                SortOrder::KEY => (keys::sort_text(&track.key), 0),
                SortOrder::BPM => (String::new(), i64::from(track.tempo)),
                SortOrder::RATING => (String::new(), -i64::from(track.rating)),
                SortOrder::BITRATE => (String::new(), i64::from(track.bitrate)),
                SortOrder::PLAY_COUNT => (String::new(), -i64::from(track.play_count)),
                SortOrder::DATE_ADDED => {
                    (String::new(), -i64::from(packed_date(&track.date_added)))
                }
                // DEFAULT, ALPHABET, and any order a real SORT menu never
                // offers: by title, which is what a real deck's DEFAULT is.
                _ => (title.clone(), 0),
            };
            (text, number, title, track.id, track)
        })
        .collect();
    keyed.sort_by(|a, b| (&a.0, a.1, &a.2, a.3).cmp(&(&b.0, b.1, &b.2, b.3)));
    keyed.into_iter().map(|(_, _, _, _, track)| track).collect()
}

/// `2025-07-10` as `20250710`, or 0 for anything else.
///
/// A stable id for a DATE ADDED row: the deck sends back whatever id the row
/// carried, and deriving it from the date rather than from a position in the
/// listing means the drill cannot be thrown off by the library changing between
/// the two requests.
fn packed_date(date: &str) -> u32 {
    let mut parts = date.split('-');
    let year: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let month: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let day: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    if parts.next().is_some() || year == 0 {
        return 0;
    }
    year.saturating_mul(10_000)
        .saturating_add(month.saturating_mul(100))
        .saturating_add(day)
}
