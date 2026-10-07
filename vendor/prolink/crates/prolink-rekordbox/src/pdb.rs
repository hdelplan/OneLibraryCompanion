// SPDX-License-Identifier: GPL-3.0-only

//! `export.pdb` — the DeviceSQL database rekordbox writes to a medium.
//!
//! A fixed-page store: 4096-byte pages, one chain of pages per table, rows
//! written forward from the page header while their offsets live in a **reverse
//! index** growing backwards from the end of the page. Everything is
//! little-endian, which is the opposite of the ANLZ files it points at.
//!
//! Random access is required throughout — page indices are absolute and rows
//! point at their own strings by relative offset — so the whole file has to be
//! resident. [`Pdb`] therefore borrows a slice rather than reading a stream.
//!
//! # The awkward parts, all of which are real and all of which bite
//!
//! **The reverse index is doubly reversed.** Row offsets live in groups of 16
//! at the end of the page, each group 36 bytes: 32 bytes of offsets, a 16-bit
//! presence bitmask, and two more bytes. Within a group the offsets run
//! backwards, so row 0 occupies the *last* slot.
//!
//! **The row-slot count is packed into 13 bits at 0x18.** The low byte
//! alone wraps at 256. The field historically called `num_rows_large` at
//! 0x22 is separate bookkeeping, not the high part of this count. Using it
//! as a fallback truncates valid row-index groups on real USB exports.
//! The slot count bounds the reverse-index group walk; each presence bitmask
//! then decides which rows are live, including holes left by deleted rows.
//!
//! **Every table chain starts with a "strange" page** — flag `0x40` — that
//! holds no rows and only links onward.
//!
//! # The header's sequence counter is volatile (F13)
//!
//! Pulling a 1,077,248-byte `export.pdb` off a deck over NFS and then reading
//! the same file off the ejected stick produced two files whose SHA-256s
//! differ. They differ in **exactly two fields**, both in the file header, and
//! nowhere else in a megabyte: `unknown1` at `0x10` (4 → 5) and the global
//! write counter `sequence` at `0x14` (20585 → 20586). The deck wrote to its
//! own database between the reads — a play count, a history entry — and the
//! library content was bit-identical.
//!
//! So any cache keyed on a whole-file hash invalidates spuriously and
//! re-downloads a library that has not changed by one track. Use
//! [`stable_digest`], which zeroes [`VOLATILE_HEADER`] first.

pub mod row;

use std::collections::HashSet;
use std::fmt;
use std::io::{Cursor, Seek, SeekFrom};
use std::ops::Range;

use binrw::{BinRead, binrw};

use crate::error::{Error, Result};

pub use row::{
    ALBUM_SUBTYPE_FAR, ALBUM_SUBTYPE_NEAR, ARTIST_SUBTYPE_FAR, ARTIST_SUBTYPE_NEAR, AlbumRow,
    ArtistRow, ArtworkRow, ColorRow, ColumnRow, Container, GenreRow, HistoryEntryRow,
    HistoryPlaylistRow, KeyRow, LabelRow, PlaylistEntryRow, PlaylistTreeRow, Row,
    TRACK_STRING_SLOTS, TrackRow, TrackStrings,
};

/// Every page is this many bytes, on every medium ever seen.
pub const PAGE_SIZE: u64 = 4096;

/// Length of a page header, and the offset the first row starts at.
pub const PAGE_HEADER_LEN: u64 = 0x28;

/// Bytes one reverse-index group occupies: 16 offsets, the presence bitmask,
/// and two bytes whose meaning is unknown.
pub const ROW_GROUP_LEN: u64 = 0x24;

/// Row offsets per reverse-index group.
pub const ROWS_PER_GROUP: u64 = 16;

/// Sentinel observed in the legacy 0x22 bookkeeping field.
pub const NUM_ROWS_SENTINEL: u16 = 0x1fff;

/// Page-flag bit marking a chain-head page that carries no rows.
pub const PAGE_FLAG_STRANGE: u8 = 0x40;

/// Bytes of the file header a player rewrites as it operates (F13).
///
/// `unknown1` at `0x10` and the global write counter `sequence` at `0x14`.
/// Excluded from [`stable_digest`].
pub const VOLATILE_HEADER: Range<usize> = 0x10..0x18;

/// Which table a page belongs to.
///
/// A newtype rather than an enum: a real medium carries page types this crate
/// has no decoder for — `testdata/export.pdb` has rows under types 17 and 18,
/// which nothing in the published analysis names — and a reader that refused an
/// unknown type would take out the tables it does understand.
#[binrw]
#[brw(little)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageType(pub u32);

impl PageType {
    /// Track metadata: title, artist id, tempo, container, paths.
    pub const TRACKS: Self = Self(0);
    /// Musical genres, referenced by tracks.
    pub const GENRES: Self = Self(1);
    /// Artists, referenced by tracks as artist, composer, remixer or original.
    pub const ARTISTS: Self = Self(2);
    /// Albums, each with its own album-artist reference.
    pub const ALBUMS: Self = Self(3);
    /// Record labels.
    pub const LABELS: Self = Self(4);
    /// Musical keys, used for browsing and key matching.
    pub const KEYS: Self = Self(5);
    /// Colour labels.
    pub const COLORS: Self = Self(6);
    /// The playlist tree: folders and playlists.
    pub const PLAYLIST_TREE: Self = Self(7);
    /// Playlist membership, one row per track per playlist.
    pub const PLAYLIST_ENTRIES: Self = Self(8);
    /// History playlists, recorded each time a player mounts the medium.
    pub const HISTORY_PLAYLISTS: Self = Self(11);
    /// Membership of the history playlists.
    pub const HISTORY_ENTRIES: Self = Self(12);
    /// Album artwork paths.
    pub const ARTWORK: Self = Self(13);
    /// The categories a CDJ offers in its root menu.
    pub const COLUMNS: Self = Self(16);
    /// History synchronisation data; not decoded here.
    pub const HISTORY: Self = Self(19);

    /// A name for logs, or `None` for a type this crate does not model.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::TRACKS => "tracks",
            Self::GENRES => "genres",
            Self::ARTISTS => "artists",
            Self::ALBUMS => "albums",
            Self::LABELS => "labels",
            Self::KEYS => "keys",
            Self::COLORS => "colors",
            Self::PLAYLIST_TREE => "playlist_tree",
            Self::PLAYLIST_ENTRIES => "playlist_entries",
            Self::HISTORY_PLAYLISTS => "history_playlists",
            Self::HISTORY_ENTRIES => "history_entries",
            Self::ARTWORK => "artwork",
            Self::COLUMNS => "columns",
            Self::HISTORY => "history",
            _ => return None,
        })
    }
}

impl fmt::Debug for PageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "PageType({})", self.0),
        }
    }
}

/// One entry of the file header's table directory.
#[binrw]
#[brw(little)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TableRef {
    /// Which table this chain holds.
    pub page_type: PageType,
    /// Unknown; possibly a chain of pages freed by garbage collection.
    pub empty_candidate: u32,
    /// First page of the chain — the "strange" one, which holds no rows.
    pub first_page: u32,
    /// Last page of the chain.
    pub last_page: u32,
}

/// The 40-byte header every page starts with.
///
/// Fields whose meaning is unknown are kept and named for their offset, because
/// two of them ([`PageHeader::num_rows`], [`PageHeader::is_strange`]) turned out
/// to be load-bearing after being dismissed as padding.
#[binrw]
#[brw(little)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PageHeader {
    /// Always zero.
    pub gap: u32,
    /// This page's own index, which should match where it was read from.
    pub page_index: u32,
    /// The table this page belongs to.
    pub page_type: PageType,
    /// Index of the next page in the chain.
    pub next_page: u32,
    /// Unknown.
    pub unknown1: u32,
    /// Unknown.
    pub unknown2: u32,
    /// Low eight bits of the packed 13-bit row-slot count at 0x18.
    pub num_rows_small: u8,
    /// Low five bits complete the row-slot count; upper bits belong to another packed field.
    pub unknown3: u8,
    /// Unknown.
    pub unknown4: u8,
    /// Bit `0x40` marks a strange page.
    pub page_flags: u8,
    /// Free bytes on the page.
    pub free_size: u16,
    /// Bytes of row data in use.
    pub used_size: u16,
    /// Unknown; `0x0001` on every page seen.
    pub unknown5: u16,
    /// Legacy field name for bookkeeping at 0x22; not the row-slot count.
    pub num_rows_large: u16,
    /// Unknown; `1004` on the strange pages of `testdata/export.pdb`.
    pub unknown6: u16,
    /// Unknown.
    pub unknown7: u16,
}

impl PageHeader {
    /// A chain-head page that links onward but holds no rows.
    pub fn is_strange(&self) -> bool {
        self.page_flags & PAGE_FLAG_STRANGE != 0
    }

    /// Whether this page's reverse index should be walked at all.
    pub fn holds_rows(&self) -> bool {
        !self.is_strange()
    }

    /// How many row slots the reverse index covers (13 little-endian bits at 0x18).
    /// Presence masks decide which of those indexed rows remain live.
    pub fn num_rows(&self) -> u16 {
        u16::from(self.num_rows_small) | (u16::from(self.unknown3 & 0x1f) << 8)
    }
}

/// A parsed `export.pdb`, borrowing the file's bytes.
///
/// Rows are decoded on demand, so opening a database costs only the header walk
/// and only the tables actually asked for are read.
#[derive(Clone, Debug)]
pub struct Pdb<'a> {
    data: &'a [u8],
    page_size: u32,
    tables: Vec<TableRef>,
}

impl<'a> Pdb<'a> {
    /// Parse the file header and table directory.
    ///
    /// Fails on a file shorter than one page, a page size other than 4096, or a
    /// table directory with no entries. That last check is not pedantry: a
    /// buffer of zeroes otherwise parses as a perfectly valid database with no
    /// tracks, and for a file that arrived over a network a truncated download
    /// would then be indistinguishable from a stick with nothing on it.
    pub fn new(data: &'a [u8]) -> Result<Self> {
        let file_len = u64::try_from(data.len()).unwrap_or(u64::MAX);
        if file_len < PAGE_SIZE {
            return Err(Error::truncated(0, PAGE_SIZE, file_len));
        }
        let page_size = u32_at(data, 0x04).unwrap_or(0);
        if u64::from(page_size) != PAGE_SIZE {
            return Err(Error::malformed(
                0x04,
                format!("page size {page_size}, expected {PAGE_SIZE}"),
            ));
        }
        let num_tables = u32_at(data, 0x08).unwrap_or(0);
        if num_tables == 0 {
            return Err(Error::malformed(
                0x08,
                "no tables; this is not a rekordbox database",
            ));
        }

        let mut tables = Vec::new();
        let mut cursor = Cursor::new(data);
        for index in 0..u64::from(num_tables) {
            let at = 0x1c + index * 16;
            if cursor.seek(SeekFrom::Start(at)).is_err() {
                break;
            }
            let Ok(table) = TableRef::read(&mut cursor) else {
                break;
            };
            tables.push(table);
        }

        Ok(Self {
            data,
            page_size,
            tables,
        })
    }

    /// The bytes this database was parsed from.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Page size from the header; 4096 on everything observed.
    pub fn page_size(&self) -> u32 {
        self.page_size
    }

    /// The table directory, in file order.
    pub fn tables(&self) -> &[TableRef] {
        &self.tables
    }

    /// The first table of the given type, or `None`.
    ///
    /// Later duplicates are ignored, matching what a player appears to do.
    pub fn table(&self, page_type: PageType) -> Option<&TableRef> {
        self.tables.iter().find(|t| t.page_type == page_type)
    }

    /// A content hash that survives the player rewriting its own bookkeeping.
    ///
    /// See [`stable_digest`] and F13.
    pub fn stable_digest(&self) -> StableDigest {
        stable_digest(self.data)
    }

    /// The pages of one table's chain that carry rows.
    ///
    /// Strange pages are stepped over rather than read, and so is page 0: it is
    /// the file header, whose bytes at a page header's offsets are table
    /// directory entries, and reading it as a page yields plausible nonsense
    /// rather than an error.
    ///
    /// The walk stops when the chain points at itself, at page 0, at a page
    /// already visited, or past the end of the file. All four are how a chain
    /// normally terminates rather than signs of corruption — a real medium
    /// declares more pages in its header than the file actually holds.
    pub fn pages(&self, page_type: PageType) -> Vec<(u32, PageHeader)> {
        let mut out = Vec::new();
        let Some(table) = self.table(page_type) else {
            return out;
        };
        let mut seen = HashSet::new();
        let mut index = table.first_page;
        while index != 0 && seen.insert(index) {
            let Some(header) = self.page_header(index) else {
                break;
            };
            if header.holds_rows() {
                out.push((index, header));
            }
            let next = header.next_page;
            if next == index || u64::from(next) * PAGE_SIZE >= self.file_len() {
                break;
            }
            index = next;
        }
        out
    }

    /// Read one page's header, or `None` if the page lies past the file.
    pub fn page_header(&self, index: u32) -> Option<PageHeader> {
        let start = u64::from(index) * PAGE_SIZE;
        if start.checked_add(PAGE_HEADER_LEN)? > self.file_len() {
            return None;
        }
        let mut cursor = Cursor::new(self.data);
        cursor.seek(SeekFrom::Start(start)).ok()?;
        PageHeader::read(&mut cursor).ok()
    }

    /// Absolute file offsets of the live rows on one page.
    ///
    /// The presence bitmask decides which slots are live (F47); a cleared bit is
    /// a deleted row, whose bytes are stale, and that is entirely normal.
    pub fn row_offsets(&self, page_index: u32, header: &PageHeader) -> Vec<u64> {
        let page_start = u64::from(page_index) * PAGE_SIZE;
        let page_end = page_start + PAGE_SIZE;
        let heap_start = page_start + PAGE_HEADER_LEN;
        let mut out = Vec::new();
        if page_end > self.file_len() {
            return out;
        }

        let groups = u64::from(header.num_rows()).div_ceil(ROWS_PER_GROUP);
        for group in 0..groups {
            let base = page_end - group * ROW_GROUP_LEN;
            let Some(block) = base.checked_sub(ROW_GROUP_LEN) else {
                break;
            };
            if block < heap_start {
                break;
            }
            let present = u16_at(self.data, base - 4).unwrap_or(0);
            for slot in 0..ROWS_PER_GROUP {
                if present >> slot & 1 == 0 {
                    continue;
                }
                // Slots run backwards within the group: row 0 is the last one.
                let position = block + (ROWS_PER_GROUP - 1 - slot) * 2;
                let Some(relative) = u16_at(self.data, position) else {
                    continue;
                };
                let absolute = heap_start + u64::from(relative);
                if absolute < page_end {
                    out.push(absolute);
                }
            }
        }
        out
    }

    /// Every live row of one table, decoded.
    ///
    /// A row that fails to decode is dropped and the rest of the table is kept:
    /// real media carry rows whose subtype nothing documents, and losing one
    /// album's name is a better outcome than losing the album table.
    pub fn rows<R: Row>(&self) -> Vec<R> {
        let mut out = Vec::new();
        let mut cursor = Cursor::new(self.data);
        for (index, header) in self.pages(R::PAGE_TYPE) {
            for offset in self.row_offsets(index, &header) {
                if cursor.seek(SeekFrom::Start(offset)).is_err() {
                    continue;
                }
                if let Ok(row) = R::read_row(&mut cursor) {
                    out.push(row);
                }
            }
        }
        out
    }

    /// Live row counts per table, for a dump command.
    ///
    /// Counts slots the presence bitmask marks live, without decoding them, so
    /// it also covers the tables this crate has no row type for.
    pub fn row_counts(&self) -> Vec<(PageType, usize)> {
        let mut out = Vec::new();
        for table in &self.tables {
            let count = self
                .pages(table.page_type)
                .iter()
                .map(|(index, header)| self.row_offsets(*index, header).len())
                .sum();
            out.push((table.page_type, count));
        }
        out
    }

    fn file_len(&self) -> u64 {
        u64::try_from(self.data.len()).unwrap_or(u64::MAX)
    }
}

/// A content hash of an `export.pdb` that ignores the player's write counter.
///
/// Not a cryptographic hash — FNV-1a over 128 bits, which is a cache key rather
/// than a signature. A caller that wants SHA-256 should zero
/// [`VOLATILE_HEADER`] and hash the result itself; that window is the whole of
/// the finding (F13) and the choice of hash is not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct StableDigest(pub u128);

impl fmt::Debug for StableDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl fmt::Display for StableDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// FNV-1a offset basis for 128 bits.
const FNV_OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
/// FNV-1a prime for 128 bits.
const FNV_PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

/// Hash a database's content, treating [`VOLATILE_HEADER`] as zero (F13).
pub fn stable_digest(data: &[u8]) -> StableDigest {
    let mut hash = FNV_OFFSET;
    for (index, byte) in data.iter().enumerate() {
        let byte = if VOLATILE_HEADER.contains(&index) {
            0
        } else {
            *byte
        };
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    StableDigest(hash)
}

fn u16_at(data: &[u8], at: u64) -> Option<u16> {
    let at = usize::try_from(at).ok()?;
    let raw: [u8; 2] = data.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_le_bytes(raw))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let raw: [u8; 4] = data.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
}
