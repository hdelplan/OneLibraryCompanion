// SPDX-License-Identifier: GPL-3.0-only

//! The row layouts of each `export.pdb` table.
//!
//! Everything here is little-endian. Fields whose meaning is unknown are still
//! read and still public, named for the position they occupy, because this
//! format has a history of "padding" turning out to matter: row offset `0x5a`
//! of a track was called `unknown6` for years and is the container the file is
//! stored in (F34).
//!
//! # Strings do not live inline
//!
//! A row that carries text stores a **relative offset** to it instead, and the
//! string itself sits after the fixed part of the row. So every string field
//! here is read by seeking to `row base + offset` and coming back. An offset of
//! zero points at the row's own header, which can never be a string, and means
//! the slot is unused, and the field reads back empty.
//!
//! # Two rows have a near and a far form
//!
//! An artist row of subtype `0x64` and an album row of subtype `0x84` put their
//! name offset in a `u16` further along instead of the `u8` the common form
//! uses. The artist case is documented; the album case is not, and is handled
//! here by analogy — see [`AlbumRow`].

use std::io::{Read, Seek};

use binrw::{BinRead, BinResult, Endian, binread};

use super::PageType;
use crate::string::{DeviceSqlString, read_at};

/// A table row this crate can decode.
///
/// Implemented rather than blanket-derived so the page type and the row layout
/// are declared in one place and cannot drift apart.
pub trait Row: Sized {
    /// The table this row lives in.
    const PAGE_TYPE: PageType;

    /// Decode one row from a reader positioned at its first byte.
    fn read_row<R: Read + Seek>(reader: &mut R) -> BinResult<Self>;
}

macro_rules! impl_row {
    ($ty:ty, $page_type:expr) => {
        impl Row for $ty {
            const PAGE_TYPE: PageType = $page_type;

            fn read_row<R: Read + Seek>(reader: &mut R) -> BinResult<Self> {
                <$ty>::read_le(reader)
            }
        }
    };
}

/// Consume nothing and report where the reader is.
fn current_position<R: Read + Seek>(reader: &mut R, _: Endian, _: ()) -> BinResult<u64> {
    reader.stream_position().map_err(binrw::Error::Io)
}

/// Read the string a row points at, without disturbing the row cursor.
#[expect(
    clippy::unnecessary_wraps,
    reason = "binrw's parse_with signature is fallible; this reader is not"
)]
fn string_at<R: Read + Seek>(
    reader: &mut R,
    _: Endian,
    (base, offset): (u64, u16),
) -> BinResult<DeviceSqlString> {
    Ok(read_at(reader, base, offset))
}

// -- tracks ----------------------------------------------------------------

/// The container a track's audio is stored in, from row offset `0x5a` (F34).
///
/// A newtype, not an enum: the table below is every value seen, not every value
/// that exists, and a container we have not met must not take out the row.
///
/// Settled by building the medium the question needed — one source track
/// rendered into all 40 formats a CDJ-2000NXS accepts — and diffing raw pdb
/// rows across containers. 651 rows, no exceptions within a container. The
/// published schema leaves this field named `unknown6`.
///
/// | Container | value | rows |
/// |---|---|---|
/// | `.mp3` | 1 | 617 |
/// | `.m4a` | 4 | 8 |
/// | `.flac` | 5 | 1 |
/// | `.wav` | 11 | 12 |
/// | `.aiff` | 12 | 4 |
///
/// It matters more than an identifier usually would: this is what a player is
/// told over dbserver, and a player believes it. Announcing MP3 for a WAV makes
/// the deck fetch the whole file, try to decode it as an MP3, and put
/// "CDJ DOES NOT DECODE THIS FORMAT" on its screen — which is exactly what a
/// hardcoded `1` did.
///
/// Zero is *not* a container any medium carries. It is what the `Default` impl
/// holds, and it exists only so aggregates over a track can derive `Default`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, BinRead)]
#[br(little)]
pub struct Container(pub u16);

impl Container {
    /// `.mp3`
    pub const MP3: Self = Self(1);
    /// `.m4a`, AAC in an MPEG-4 container.
    pub const AAC: Self = Self(4);
    /// `.flac`
    pub const FLAC: Self = Self(5);
    /// `.wav`
    pub const WAV: Self = Self(11);
    /// `.aiff`
    pub const AIFF: Self = Self(12);

    /// A name for logs, or `None` for a value never observed.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::MP3 => "mp3",
            Self::AAC => "aac",
            Self::FLAC => "flac",
            Self::WAV => "wav",
            Self::AIFF => "aiff",
            _ => return None,
        })
    }
}

impl std::fmt::Debug for Container {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "Container({})", self.0),
        }
    }
}

/// The 21 string slots a track row points at.
///
/// Slot numbers are the schema: the offset table at row `0x5e` holds 21 `u16`s
/// and slot *n* is the one at `0x5e + 2n`. Nine of them have no known meaning
/// and are kept under their published names so nothing is silently dropped.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct TrackStrings {
    /// Slot 0. The International Standard Recording Code, in the mangled form
    /// rekordbox writes it — see [`crate::string::StringForm::Isrc`]. Present
    /// on 245 of the 651 rows in `testdata/export.pdb`.
    pub isrc: DeviceSqlString,
    /// Slot 1. Unknown.
    pub unknown_string1: DeviceSqlString,
    /// Slot 2. Unknown.
    pub unknown_string2: DeviceSqlString,
    /// Slot 3. Unknown.
    pub unknown_string3: DeviceSqlString,
    /// Slot 4. Unknown.
    pub unknown_string4: DeviceSqlString,
    /// Slot 5. Unknown; called `message` in the published schema.
    pub message: DeviceSqlString,
    /// Slot 6. `"ON"` when the track may be published to kuvo.com.
    pub kuvo_public: DeviceSqlString,
    /// Slot 7. `"ON"` when hot cues should be auto-loaded.
    pub autoload_hotcues: DeviceSqlString,
    /// Slot 8. Unknown.
    pub unknown_string5: DeviceSqlString,
    /// Slot 9. Unknown.
    pub unknown_string6: DeviceSqlString,
    /// Slot 10. When the track was added to the collection, `YYYY-MM-DD`.
    pub date_added: DeviceSqlString,
    /// Slot 11. Release date.
    pub release_date: DeviceSqlString,
    /// Slot 12. Name of the mix or remix.
    pub mix_name: DeviceSqlString,
    /// Slot 13. Unknown.
    pub unknown_string7: DeviceSqlString,
    /// Slot 14. Path to the `.DAT` analysis file, in the player's namespace.
    pub analyze_path: DeviceSqlString,
    /// Slot 15. When the analysis was performed.
    pub analyze_date: DeviceSqlString,
    /// Slot 16. The DJ's comment.
    pub comment: DeviceSqlString,
    /// Slot 17. Track title.
    pub title: DeviceSqlString,
    /// Slot 18. Unknown.
    pub unknown_string8: DeviceSqlString,
    /// Slot 19. File name, without a directory.
    pub filename: DeviceSqlString,
    /// Slot 20. Full path to the audio file, in the player's namespace.
    pub file_path: DeviceSqlString,
}

/// Number of string slots a track row carries.
pub const TRACK_STRING_SLOTS: usize = 21;

fn track_strings<R: Read + Seek>(
    reader: &mut R,
    _: Endian,
    (base,): (u64,),
) -> BinResult<TrackStrings> {
    let offsets = <[u16; TRACK_STRING_SLOTS]>::read_le(reader)?;
    let texts: Vec<DeviceSqlString> = offsets
        .iter()
        .map(|&offset| read_at(reader, base, offset))
        .collect();
    let slot = |index: usize| texts.get(index).cloned().unwrap_or_default();
    Ok(TrackStrings {
        isrc: slot(0),
        unknown_string1: slot(1),
        unknown_string2: slot(2),
        unknown_string3: slot(3),
        unknown_string4: slot(4),
        message: slot(5),
        kuvo_public: slot(6),
        autoload_hotcues: slot(7),
        unknown_string5: slot(8),
        unknown_string6: slot(9),
        date_added: slot(10),
        release_date: slot(11),
        mix_name: slot(12),
        unknown_string7: slot(13),
        analyze_path: slot(14),
        analyze_date: slot(15),
        comment: slot(16),
        title: slot(17),
        unknown_string8: slot(18),
        filename: slot(19),
        file_path: slot(20),
    })
}

/// One track: everything a browse menu shows and everything a load needs.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TrackRow {
    #[br(temp, parse_with = current_position)]
    base: u64,
    /// `0x02`. Unknown; called `index_shift`. The `0x0024` magic precedes it.
    #[br(magic = 0x0024u16)]
    pub index_shift: u16,
    /// `0x04`. Unknown; called `bitmask`.
    pub bitmask: u32,
    /// `0x08`. Sample rate in Hz.
    pub sample_rate: u32,
    /// `0x0c`. Artist row id of the composer, or zero.
    pub composer_id: u32,
    /// `0x10`. File size in bytes. A player needs this to load, and it is not
    /// on any browse menu (F31).
    pub file_size: u32,
    /// `0x14`. Unknown.
    pub unknown2: u32,
    /// `0x18`. Unknown.
    pub unknown3: u16,
    /// `0x1a`. Unknown.
    pub unknown4: u16,
    /// `0x1c`. Artwork row id, or zero. A menu item must carry this or the
    /// player never asks for the image.
    pub artwork_id: u32,
    /// `0x20`. Key row id, or zero.
    pub key_id: u32,
    /// `0x24`. Artist row id of the original performer, or zero.
    pub original_artist_id: u32,
    /// `0x28`. Label row id, or zero.
    pub label_id: u32,
    /// `0x2c`. Artist row id of the remixer, or zero.
    pub remixer_id: u32,
    /// `0x30`. Bitrate in kbps.
    pub bitrate: u32,
    /// `0x34`. Track number within its album.
    pub track_number: u32,
    /// `0x38`. Tempo in centi-BPM, so the format needs no floats.
    pub tempo: u32,
    /// `0x3c`. Genre row id, or zero.
    pub genre_id: u32,
    /// `0x40`. Album row id, or zero.
    pub album_id: u32,
    /// `0x44`. Artist row id, or zero.
    pub artist_id: u32,
    /// `0x48`. This row's id.
    pub id: u32,
    /// `0x4c`. Disc number. **Not** what `GET_TRACK_INFO` item 1 carries —
    /// serving it there broke MP3 loading (F34).
    pub disc_number: u16,
    /// `0x4e`. Times the track has been played.
    pub play_count: u16,
    /// `0x50`. Release year.
    pub year: u16,
    /// `0x52`. Bits per sample.
    pub sample_depth: u16,
    /// `0x54`. Playing time in seconds at normal pitch.
    pub duration: u16,
    /// `0x56`. Unknown; `29` on every row seen.
    pub unknown5: u16,
    /// `0x58`. Colour row id, or zero. One byte, matching the colour table.
    pub color_id: u8,
    /// `0x59`. Rating, 0–5 stars.
    pub rating: u8,
    /// `0x5a`. The container (F34). See [`Container`].
    pub container: Container,
    /// `0x5c`. Unknown; alternates between 2 and 3.
    pub unknown7: u16,
    /// `0x5e`. The 21 string slots, already dereferenced.
    #[br(args(base), parse_with = track_strings)]
    pub strings: TrackStrings,
}

impl_row!(TrackRow, PageType::TRACKS);

// -- artists, albums -------------------------------------------------------

/// Artist row subtype whose name offset is the `u8` at `0x09`.
pub const ARTIST_SUBTYPE_NEAR: u16 = 0x60;
/// Artist row subtype whose name offset is the `u16` at `0x0a`.
pub const ARTIST_SUBTYPE_FAR: u16 = 0x64;

/// One artist.
///
/// All 329 rows of `testdata/export.pdb` are the near form; the far form is
/// implemented from the published schema and is not exercised by real bytes
/// here.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ArtistRow {
    #[br(temp, parse_with = current_position)]
    base: u64,
    /// `0x00`. Which of the two name-offset widths this row uses.
    #[br(assert(
        subtype == ARTIST_SUBTYPE_NEAR || subtype == ARTIST_SUBTYPE_FAR,
        "unknown artist row subtype {:#06x}", subtype
    ))]
    pub subtype: u16,
    /// `0x02`. Unknown; called `index_shift`.
    pub index_shift: u16,
    /// `0x04`. This row's id.
    pub id: u32,
    /// `0x08`. Unknown; `3` on every row seen.
    pub unknown1: u8,
    /// `0x09`. Name offset used by the near form.
    pub ofs_name_near: u8,
    /// `0x0a`. Name offset used by the far form, which overrides the near one.
    #[br(if(subtype == ARTIST_SUBTYPE_FAR))]
    pub ofs_name_far: Option<u16>,
    /// The artist's name.
    #[br(
        args(base, ofs_name_far.unwrap_or(u16::from(ofs_name_near))),
        parse_with = string_at
    )]
    pub name: DeviceSqlString,
}

impl_row!(ArtistRow, PageType::ARTISTS);

/// Album row subtype whose name offset is the `u8` at `0x15`.
pub const ALBUM_SUBTYPE_NEAR: u16 = 0x80;
/// Album row subtype whose name offset is the `u16` at `0x16`.
pub const ALBUM_SUBTYPE_FAR: u16 = 0x84;

/// One album, with the artist it is credited to.
///
/// # The far form is an inference, and here is the evidence
///
/// The published schema knows only the `0x80` form and always reads the `u8` at
/// `0x15`. Real media carry a second form: of the 274 album rows in
/// `testdata/export.pdb`, 273 are `0x80` and one is `0x84` with `0x15` set to
/// **zero** — so a reader following the schema decodes the row's own header as
/// text, and then files the resulting mojibake under an id read from the same
/// misaligned bytes, overwriting a real album.
///
/// The bytes of that row are
///
/// ```text
/// 84 00 40 09 00 00 00 00 f5 00 00 00 a6 00 00 00 00 00 00 00 03 00 18 00 90 ...
///                                                              ^^ ^^^^^ ^^ selector
///                                                              |  far offset 0x18
///                                                              near offset 0
/// ```
///
/// and `base + 0x18` holds a well-formed UTF-16 string. So this crate reads the
/// far offset by exact analogy with [`ArtistRow`], whose `0x64` form *is*
/// documented and sits in the same relationship to its near offset. That is one
/// observation and an analogy, not a measurement, and it is recorded as such.
/// The alternative — the Mixxx port's choice — is to drop the row, which loses
/// that album's name but cannot corrupt another.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AlbumRow {
    #[br(temp, parse_with = current_position)]
    base: u64,
    /// `0x00`. Which of the two name-offset widths this row uses.
    #[br(assert(
        subtype == ALBUM_SUBTYPE_NEAR || subtype == ALBUM_SUBTYPE_FAR,
        "unknown album row subtype {:#06x}", subtype
    ))]
    pub subtype: u16,
    /// `0x02`. Unknown; called `index_shift`.
    pub index_shift: u16,
    /// `0x04`. Unknown.
    pub unknown2: u32,
    /// `0x08`. Artist row id this album is credited to.
    pub artist_id: u32,
    /// `0x0c`. This row's id.
    pub id: u32,
    /// `0x10`. Unknown.
    pub unknown3: u32,
    /// `0x14`. Unknown; `3` on every row seen.
    pub unknown4: u8,
    /// `0x15`. Name offset used by the near form.
    pub ofs_name_near: u8,
    /// `0x16`. Name offset used by the far form, which overrides the near one.
    #[br(if(subtype == ALBUM_SUBTYPE_FAR))]
    pub ofs_name_far: Option<u16>,
    /// The album's name.
    #[br(
        args(base, ofs_name_far.unwrap_or(u16::from(ofs_name_near))),
        parse_with = string_at
    )]
    pub name: DeviceSqlString,
}

impl_row!(AlbumRow, PageType::ALBUMS);

// -- the plain id/name tables ----------------------------------------------

/// One musical genre.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GenreRow {
    /// `0x00`. This row's id.
    pub id: u32,
    /// `0x04`. The genre's name.
    pub name: DeviceSqlString,
}

impl_row!(GenreRow, PageType::GENRES);

/// One record label.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LabelRow {
    /// `0x00`. This row's id.
    pub id: u32,
    /// `0x04`. The label's name.
    pub name: DeviceSqlString,
}

impl_row!(LabelRow, PageType::LABELS);

/// One musical key.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyRow {
    /// `0x00`. This row's id.
    pub id: u32,
    /// `0x04`. A second copy of the id, always equal to it.
    pub id2: u32,
    /// `0x08`. The key's name, in whichever notation the collection uses.
    pub name: DeviceSqlString,
}

impl_row!(KeyRow, PageType::KEYS);

/// One colour label.
///
/// The id is one byte, matching the track row's one-byte `color_id`, and it sits
/// at `0x05` after a byte that duplicates it.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ColorRow {
    /// `0x00`. Unknown.
    pub unknown1: u32,
    /// `0x04`. Unknown.
    pub unknown2: u8,
    /// `0x05`. This row's id.
    pub id: u8,
    /// `0x06`. Unknown.
    pub unknown3: u16,
    /// `0x08`. The colour's name, as the DJ set it.
    pub name: DeviceSqlString,
}

impl_row!(ColorRow, PageType::COLORS);

/// One artwork image.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ArtworkRow {
    /// `0x00`. This row's id, as referenced by [`TrackRow::artwork_id`].
    pub id: u32,
    /// `0x04`. Path to the image, in the player's namespace.
    pub path: DeviceSqlString,
}

impl_row!(ArtworkRow, PageType::ARTWORK);

// -- playlists -------------------------------------------------------------

/// One node of the playlist tree: a folder or a playlist.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlaylistTreeRow {
    /// `0x00`. Id of the folder this node sits in, or zero at the root.
    pub parent_id: u32,
    /// `0x04`. Unknown.
    pub unknown1: u32,
    /// `0x08`. Where this node sorts among its siblings.
    pub sort_order: u32,
    /// `0x0c`. This row's id.
    pub id: u32,
    /// `0x10`. Non-zero when this node is a folder rather than a playlist.
    pub node_is_folder: u32,
    /// `0x14`. The name shown when navigating the menu.
    pub name: DeviceSqlString,
}

impl PlaylistTreeRow {
    /// Whether this node is a folder.
    pub fn is_folder(&self) -> bool {
        self.node_is_folder != 0
    }
}

impl_row!(PlaylistTreeRow, PageType::PLAYLIST_TREE);

/// One track's membership of one playlist.
///
/// Note the field order: `entry_index` comes **first** here and **last** in
/// [`HistoryEntryRow`], which is otherwise the same three `u32`s. Reading one
/// with the other's layout produces a playlist that is plausible and wrong.
#[binread]
#[br(little)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlaylistEntryRow {
    /// `0x00`. Position within the playlist. The on-disk row order is not the
    /// playlist order.
    pub entry_index: u32,
    /// `0x04`. The track at this position.
    pub track_id: u32,
    /// `0x08`. The playlist this entry belongs to.
    pub playlist_id: u32,
}

impl_row!(PlaylistEntryRow, PageType::PLAYLIST_ENTRIES);

/// One history playlist, recorded when a player mounts the medium.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HistoryPlaylistRow {
    /// `0x00`. This row's id.
    pub id: u32,
    /// `0x04`. The name, `HISTORY 001` and so on.
    pub name: DeviceSqlString,
}

impl_row!(HistoryPlaylistRow, PageType::HISTORY_PLAYLISTS);

/// One track's membership of one history playlist.
///
/// The three fields are in a different order from [`PlaylistEntryRow`].
#[binread]
#[br(little)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HistoryEntryRow {
    /// `0x00`. The track played.
    pub track_id: u32,
    /// `0x04`. The history playlist this entry belongs to.
    pub playlist_id: u32,
    /// `0x08`. Position within the playlist.
    pub entry_index: u32,
}

impl_row!(HistoryEntryRow, PageType::HISTORY_ENTRIES);

// -- columns ---------------------------------------------------------------

/// Interlinear annotation anchor, which wraps every column name.
const ANNOTATION_OPEN: char = '\u{fffa}';
/// Interlinear annotation terminator, which wraps every column name.
const ANNOTATION_CLOSE: char = '\u{fffb}';

/// One of the categories a CDJ offers in its browse menu.
///
/// The names are stored wrapped in the Unicode interlinear-annotation
/// characters `U+FFFA` and `U+FFFB` — `\u{fffa}GENRE\u{fffb}` — for reasons
/// nobody has explained, and always in the long form even though they are pure
/// ASCII. [`ColumnRow::label`] gives the text without them.
#[binread]
#[br(little)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ColumnRow {
    /// `0x00`. This row's id, `1` upwards in menu order.
    pub id: u16,
    /// `0x02`. The menu item type a CDJ uses for this category, `0x80` upwards.
    pub menu_item_type: u16,
    /// `0x04`. The name, annotation characters included.
    pub name: DeviceSqlString,
}

impl ColumnRow {
    /// The category name with its annotation characters stripped.
    pub fn label(&self) -> &str {
        self.name
            .as_str()
            .trim_matches(|c| c == ANNOTATION_OPEN || c == ANNOTATION_CLOSE)
    }
}

impl_row!(ColumnRow, PageType::COLUMNS);
