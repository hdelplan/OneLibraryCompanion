// SPDX-License-Identifier: GPL-3.0-only

//! `export.pdb` reader tests, in two halves.
//!
//! The second half asserts against **a real 675 KB rekordbox export** —
//! `testdata/export.pdb`, 165 pages off the author's own USB stick, the same
//! medium the container finding (F34) and the presence-bitmask finding (F47)
//! came from. That is the only thing that can catch the class of bug this
//! format specialises in: a reader that is self-consistently wrong. The
//! reference implementation had an encoder and a decoder that agreed perfectly
//! on a UTF-16 bug, parsed a 692-track library cleanly, and mangled every
//! non-ASCII name (O6).
//!
//! The first half is the **fixture floor**: a synthetic database built here,
//! exercising the structural cases a single real file may not contain — a
//! deleted row, a row index spanning several groups, a malformed row, the packed
//! row-slot count, the far name-offset forms. Those run whether or
//! not the real export is present, so a missing file cannot hide a regression.

// An integration test is its own crate, so the crate root's test exemptions do
// not reach it. Tests are allowed to panic: an assertion *is* the failure mode,
// and a test that carefully propagated errors would report them as passes. The
// pdb writer below indexes and slices freely for the same reason — it is
// building a fixture, not reading one from a device.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::float_cmp
)]

use std::collections::BTreeMap;

use prolink_rekordbox::pdb::{
    ALBUM_SUBTYPE_FAR, ALBUM_SUBTYPE_NEAR, ARTIST_SUBTYPE_FAR, ARTIST_SUBTYPE_NEAR, AlbumRow,
    ArtistRow, ArtworkRow, ColorRow, ColumnRow, Container, GenreRow, HistoryEntryRow,
    HistoryPlaylistRow, KeyRow, LabelRow, PAGE_SIZE, PageType, Pdb, PlaylistEntryRow,
    PlaylistTreeRow, ROW_GROUP_LEN, TrackRow, stable_digest,
};
use prolink_rekordbox::string::DeviceSqlString;
use prolink_rekordbox::{Error, Library};

// -- a minimal pdb writer, for the structural cases --------------------------

const PAGE_LEN: usize = 4096;
const HEAP_START: usize = 0x28;
const GROUP_LEN: usize = 0x24;

/// Builds a structurally valid `export.pdb`.
///
/// Page 0 is the file header; each table then gets a "strange" chain-head page
/// followed by however many data pages its rows need, mirroring how real files
/// are arranged so chain walking and strange-page skipping are exercised rather
/// than bypassed.
#[derive(Default)]
struct PdbBuilder {
    tables: BTreeMap<u32, Vec<Vec<u8>>>,
    /// Row count to write into a page header instead of the true one, to
    /// exercise the disagreement between the header and the presence bitmask.
    understated_count: Option<u8>,
}

impl PdbBuilder {
    fn add(&mut self, page_type: PageType, row: Vec<u8>) -> &mut Self {
        self.tables.entry(page_type.0).or_default().push(row);
        self
    }

    fn build(&self) -> Vec<u8> {
        let layout: BTreeMap<u32, Vec<Vec<Vec<u8>>>> = self
            .tables
            .iter()
            .map(|(page_type, rows)| (*page_type, paginate(rows)))
            .collect();

        let mut first_pages: BTreeMap<u32, usize> = BTreeMap::new();
        let mut next_index = 1;
        for (page_type, pages) in &layout {
            first_pages.insert(*page_type, next_index);
            next_index += 1 + pages.len();
        }
        let total_pages = next_index;

        let mut out = vec![0u8; PAGE_LEN * total_pages];
        put_u32(&mut out, 0x04, u32::try_from(PAGE_LEN).unwrap());
        put_u32(&mut out, 0x08, u32::try_from(layout.len()).unwrap());
        for (slot, (page_type, pages)) in layout.iter().enumerate() {
            let first = first_pages[page_type];
            let at = 0x1c + slot * 16;
            put_u32(&mut out, at, *page_type);
            put_u32(&mut out, at + 8, u32::try_from(first).unwrap());
            put_u32(
                &mut out,
                at + 12,
                u32::try_from(first + pages.len()).unwrap(),
            );
        }

        for (page_type, pages) in &layout {
            let strange = first_pages[page_type];
            // Strange page: flag 0x40, no rows, links to the first data page.
            write_header(&mut out, strange, *page_type, strange + 1, 0, 0x40);
            for (offset, rows) in pages.iter().enumerate() {
                let index = strange + 1 + offset;
                let last = offset == pages.len() - 1;
                let next = if last { total_pages } else { index + 1 };
                self.write_data_page(&mut out, index, *page_type, rows, next);
            }
        }
        out
    }

    fn write_data_page(
        &self,
        out: &mut [u8],
        index: usize,
        page_type: u32,
        rows: &[Vec<u8>],
        next: usize,
    ) {
        let declared = self
            .understated_count
            .unwrap_or_else(|| u8::try_from(rows.len()).unwrap());
        write_header(out, index, page_type, next, declared, 0);

        let base = index * PAGE_LEN;
        let mut cursor = HEAP_START;
        let mut offsets = Vec::new();
        for row in rows {
            offsets.push(u16::try_from(cursor - HEAP_START).unwrap());
            out[base + cursor..base + cursor + row.len()].copy_from_slice(row);
            cursor += row.len();
        }

        let page_end = base + PAGE_LEN;
        for group in 0..rows.len().div_ceil(16) {
            let group_base = page_end - group * GROUP_LEN;
            let block = group_base - GROUP_LEN;
            let mut present = 0u16;
            for slot in 0..16 {
                let Some(offset) = offsets.get(group * 16 + slot) else {
                    break;
                };
                present |= 1 << slot;
                // Slots run backwards within the group.
                let at = block + (15 - slot) * 2;
                out[at..at + 2].copy_from_slice(&offset.to_le_bytes());
            }
            out[group_base - 4..group_base - 2].copy_from_slice(&present.to_le_bytes());
        }
    }
}

/// Split rows into page-sized runs, accounting for the reverse index.
fn paginate(rows: &[Vec<u8>]) -> Vec<Vec<Vec<u8>>> {
    let mut pages = Vec::new();
    let mut current: Vec<Vec<u8>> = Vec::new();
    let mut used = 0;
    for row in rows {
        let index_cost = (current.len() + 1).div_ceil(16) * GROUP_LEN;
        if !current.is_empty() && HEAP_START + used + row.len() + index_cost > PAGE_LEN {
            pages.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(row.clone());
        used += row.len();
    }
    pages.push(current);
    pages
}

fn write_header(
    out: &mut [u8],
    index: usize,
    page_type: u32,
    next: usize,
    num_rows: u8,
    flags: u8,
) {
    let base = index * PAGE_LEN;
    put_u32(out, base + 0x04, u32::try_from(index).unwrap());
    put_u32(out, base + 0x08, page_type);
    put_u32(out, base + 0x0c, u32::try_from(next).unwrap());
    out[base + 0x18] = num_rows;
    out[base + 0x1b] = flags;
}

fn put_u32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn text(value: &str) -> Vec<u8> {
    DeviceSqlString::new(value).encode()
}

/// The fields of a synthetic track row worth varying. A row is a schema, so
/// this is a struct rather than a dozen positional arguments.
#[derive(Default)]
struct TrackFields<'a> {
    id: u32,
    title: &'a str,
    artist_id: u32,
    album_id: u32,
    genre_id: u32,
    key_id: u32,
    color_id: u8,
    artwork_id: u32,
    tempo: u32,
    path: &'a str,
    analyze_path: &'a str,
    container: u16,
    disc_number: u16,
}

/// A track row: fixed part, the 21-slot offset table, then the strings.
fn track_row(fields: &TrackFields<'_>) -> Vec<u8> {
    let mut row = vec![0u8; 0x5e + 21 * 2];
    row[0x00..0x02].copy_from_slice(&0x0024u16.to_le_bytes());
    put_u32(&mut row, 0x08, 44100);
    put_u32(&mut row, 0x1c, fields.artwork_id);
    put_u32(&mut row, 0x20, fields.key_id);
    put_u32(&mut row, 0x30, 320);
    put_u32(&mut row, 0x38, fields.tempo);
    put_u32(&mut row, 0x3c, fields.genre_id);
    put_u32(&mut row, 0x40, fields.album_id);
    put_u32(&mut row, 0x44, fields.artist_id);
    put_u32(&mut row, 0x48, fields.id);
    row[0x4c..0x4e].copy_from_slice(&fields.disc_number.to_le_bytes());
    row[0x54..0x56].copy_from_slice(&245u16.to_le_bytes());
    row[0x58] = fields.color_id;
    row[0x59] = 4;
    row[0x5a..0x5c].copy_from_slice(&fields.container.to_le_bytes());

    let filename = fields
        .path
        .rsplit_once('/')
        .map_or(fields.path, |(_, name)| name);
    let strings: BTreeMap<usize, &str> = [
        (14, fields.analyze_path),
        (17, fields.title),
        (19, filename),
        (20, fields.path),
    ]
    .into_iter()
    .collect();

    let mut blob = Vec::new();
    for slot in 0..21 {
        let offset = match strings.get(&slot).filter(|value| !value.is_empty()) {
            Some(value) => {
                let at = u16::try_from(row.len() + blob.len()).unwrap();
                blob.extend_from_slice(&text(value));
                at
            }
            None => 0,
        };
        let at = 0x5e + slot * 2;
        row[at..at + 2].copy_from_slice(&offset.to_le_bytes());
    }
    row.extend_from_slice(&blob);
    row
}

fn artist_row(id: u32, name: &str, far: bool) -> Vec<u8> {
    let subtype = if far {
        ARTIST_SUBTYPE_FAR
    } else {
        ARTIST_SUBTYPE_NEAR
    };
    let fixed = if far { 12 } else { 10 };
    let mut row = vec![0u8; fixed];
    row[0..2].copy_from_slice(&subtype.to_le_bytes());
    put_u32(&mut row, 4, id);
    row[8] = 3;
    if far {
        // The near byte is left at zero, exactly as the real far-form rows
        // leave it, so a reader that ignores the far offset decodes the row's
        // own header as text.
        row[10..12].copy_from_slice(&u16::try_from(fixed).unwrap().to_le_bytes());
    } else {
        row[9] = u8::try_from(fixed).unwrap();
    }
    row.extend_from_slice(&text(name));
    row
}

fn album_row(id: u32, name: &str, artist_id: u32, far: bool) -> Vec<u8> {
    let subtype = if far {
        ALBUM_SUBTYPE_FAR
    } else {
        ALBUM_SUBTYPE_NEAR
    };
    let fixed = if far { 0x18 } else { 0x16 };
    let mut row = vec![0u8; fixed];
    row[0..2].copy_from_slice(&subtype.to_le_bytes());
    put_u32(&mut row, 0x08, artist_id);
    put_u32(&mut row, 0x0c, id);
    row[0x14] = 3;
    if far {
        row[0x16..0x18].copy_from_slice(&u16::try_from(fixed).unwrap().to_le_bytes());
    } else {
        row[0x15] = u8::try_from(fixed).unwrap();
    }
    row.extend_from_slice(&text(name));
    row
}

fn id_name_row(id: u32, name: &str) -> Vec<u8> {
    let mut row = id.to_le_bytes().to_vec();
    row.extend_from_slice(&text(name));
    row
}

fn key_row(id: u32, name: &str) -> Vec<u8> {
    let mut row = id.to_le_bytes().to_vec();
    row.extend_from_slice(&id.to_le_bytes());
    row.extend_from_slice(&text(name));
    row
}

fn color_row(id: u8, name: &str) -> Vec<u8> {
    let mut row = vec![0u8; 8];
    row[4] = id;
    row[5] = id;
    row.extend_from_slice(&text(name));
    row
}

fn playlist_row(id: u32, name: &str, parent_id: u32, is_folder: bool, sort_order: u32) -> Vec<u8> {
    let mut row = vec![0u8; 0x14];
    put_u32(&mut row, 0x00, parent_id);
    put_u32(&mut row, 0x08, sort_order);
    put_u32(&mut row, 0x0c, id);
    put_u32(&mut row, 0x10, u32::from(is_folder));
    row.extend_from_slice(&text(name));
    row
}

fn triple(a: u32, b: u32, c: u32) -> Vec<u8> {
    let mut row = a.to_le_bytes().to_vec();
    row.extend_from_slice(&b.to_le_bytes());
    row.extend_from_slice(&c.to_le_bytes());
    row
}

/// A small database with enough shape to exercise the reader and the join.
fn sample() -> Vec<u8> {
    let mut builder = PdbBuilder::default();
    builder
        .add(PageType::ARTISTS, artist_row(1, "New Order", false))
        .add(PageType::ARTISTS, artist_row(2, "夜のテーマ", false))
        .add(
            PageType::ALBUMS,
            album_row(1, "Power Corruption & Lies", 1, false),
        )
        .add(PageType::GENRES, id_name_row(7, "Techno"))
        .add(PageType::LABELS, id_name_row(4, "Factory"))
        .add(PageType::KEYS, key_row(3, "8A"))
        .add(PageType::COLORS, color_row(5, "Green"))
        .add(PageType::ARTWORK, id_name_row(9, "/PIONEER/ARTWORK/a.jpg"))
        .add(
            PageType::TRACKS,
            track_row(&TrackFields {
                id: 101,
                title: "Blue Monday",
                artist_id: 1,
                album_id: 1,
                key_id: 3,
                color_id: 5,
                artwork_id: 9,
                tempo: 13000,
                path: "/Contents/blue.mp3",
                analyze_path: "/PIONEER/USBANLZ/P001/00001/ANLZ0000.DAT",
                container: Container::MP3.0,
                disc_number: 1,
                genre_id: 0,
            }),
        )
        .add(
            PageType::TRACKS,
            track_row(&TrackFields {
                id: 102,
                title: "Temptation",
                artist_id: 1,
                tempo: 12800,
                path: "/Contents/temp.mp3",
                container: Container::MP3.0,
                disc_number: 2,
                ..TrackFields::default()
            }),
        )
        .add(
            PageType::TRACKS,
            track_row(&TrackFields {
                id: 103,
                title: "夜",
                artist_id: 2,
                tempo: 14000,
                path: "/Contents/yoru.flac",
                container: Container::FLAC.0,
                disc_number: 1,
                ..TrackFields::default()
            }),
        )
        .add(
            PageType::PLAYLIST_TREE,
            playlist_row(10, "Sets", 0, true, 0),
        )
        .add(
            PageType::PLAYLIST_TREE,
            playlist_row(11, "Friday", 10, false, 0),
        )
        // Written out of order, so the entry_index sort is actually tested.
        .add(PageType::PLAYLIST_ENTRIES, triple(2, 102, 11))
        .add(PageType::PLAYLIST_ENTRIES, triple(1, 101, 11))
        .add(PageType::HISTORY_PLAYLISTS, id_name_row(1, "HISTORY 001"))
        .add(PageType::HISTORY_ENTRIES, triple(103, 1, 2))
        .add(PageType::HISTORY_ENTRIES, triple(101, 1, 1));
    builder.build()
}

// -- the fixture floor -------------------------------------------------------

#[test]
fn a_strange_chain_head_page_contributes_no_rows() {
    // Every table chain begins with one. It must be walked through, not read.
    let raw = sample();
    let pdb = Pdb::new(&raw).unwrap();
    assert_eq!(pdb.rows::<TrackRow>().len(), 3);
    let pages = pdb.pages(PageType::TRACKS);
    assert_eq!(pages.len(), 1, "the strange page must not be yielded");
    assert!(!pages[0].1.is_strange());
}

#[test]
fn the_row_index_survives_more_than_one_group() {
    // Groups hold 16 rows; a 40th forces a third and exercises the
    // backwards-walking arithmetic that is easy to get subtly wrong.
    let mut builder = PdbBuilder::default();
    for index in 1..=40u32 {
        builder.add(
            PageType::GENRES,
            id_name_row(index, &format!("Genre {index}")),
        );
    }
    let raw = builder.build();
    let rows = Pdb::new(&raw).unwrap().rows::<GenreRow>();
    assert_eq!(rows.len(), 40);
    assert_eq!(
        rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        (1..=40).collect::<Vec<_>>()
    );
    assert_eq!(rows[39].name.as_str(), "Genre 40");
}

#[test]
fn a_cleared_presence_bit_is_a_deleted_row_not_the_end_of_the_page() {
    let mut builder = PdbBuilder::default();
    for index in 1..=4u32 {
        builder.add(
            PageType::GENRES,
            id_name_row(index, &format!("Genre {index}")),
        );
    }
    let mut raw = builder.build();
    // Clear the bit for row 2 on the genres data page.
    let page = 2usize;
    let flags_at = page * PAGE_LEN + PAGE_LEN - 4;
    let mut present = u16::from_le_bytes(raw[flags_at..flags_at + 2].try_into().unwrap());
    present &= !0b0010;
    raw[flags_at..flags_at + 2].copy_from_slice(&present.to_le_bytes());

    let ids: Vec<u32> = Pdb::new(&raw)
        .unwrap()
        .rows::<GenreRow>()
        .iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(ids, vec![1, 3, 4], "rows 1, 3 and 4 survive; 2 is deleted");
}

#[test]
fn the_presence_bitmask_outranks_the_headers_row_count() {
    // F47. A real medium carried num_rows = 39 on a page whose masks marked 40
    // live, and the fortieth was a genuine track. Trusting the count drops rows
    // silently, and always from the end.
    let mut builder = PdbBuilder::default();
    for index in 1..=20u32 {
        builder.add(
            PageType::GENRES,
            id_name_row(index, &format!("Genre {index}")),
        );
    }
    builder.understated_count = Some(19);
    let raw = builder.build();
    let rows = Pdb::new(&raw).unwrap().rows::<GenreRow>();
    assert_eq!(
        rows.len(),
        20,
        "the twentieth row is live per the mask and must not be dropped"
    );
}

#[test]
fn unrelated_bookkeeping_does_not_change_the_row_slot_count() {
    let mut builder = PdbBuilder::default();
    for index in 1..=20u32 {
        builder.add(
            PageType::GENRES,
            id_name_row(index, &format!("Genre {index}")),
        );
    }
    let mut raw = builder.build();
    let at = 2 * PAGE_LEN + 0x22;
    for bookkeeping in [0u16, 5, 39, 0x1fff] {
        raw[at..at + 2].copy_from_slice(&bookkeeping.to_le_bytes());
        assert_eq!(Pdb::new(&raw).unwrap().rows::<GenreRow>().len(), 20);
    }
}

#[test]
fn a_malformed_row_costs_that_row_and_not_the_table() {
    let mut builder = PdbBuilder::default();
    builder
        .add(
            PageType::TRACKS,
            track_row(&TrackFields {
                id: 1,
                title: "Good",
                path: "/a.mp3",
                ..TrackFields::default()
            }),
        )
        .add(PageType::TRACKS, vec![0; 42]) // wrong magic
        .add(
            PageType::TRACKS,
            track_row(&TrackFields {
                id: 3,
                title: "Also good",
                path: "/b.mp3",
                ..TrackFields::default()
            }),
        );
    let raw = builder.build();
    let ids: Vec<u32> = Pdb::new(&raw)
        .unwrap()
        .rows::<TrackRow>()
        .iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(ids, vec![1, 3]);
}

#[test]
fn the_container_is_read_from_row_offset_0x5a() {
    // F34. Announcing the wrong one makes a deck fetch the file, try to decode
    // an AAC as an MP3, and put an error on its screen.
    for (raw_value, expected) in [
        (1, Container::MP3),
        (4, Container::AAC),
        (5, Container::FLAC),
        (11, Container::WAV),
        (12, Container::AIFF),
    ] {
        let mut builder = PdbBuilder::default();
        builder.add(
            PageType::TRACKS,
            track_row(&TrackFields {
                id: 1,
                title: "T",
                container: raw_value,
                disc_number: 1,
                ..TrackFields::default()
            }),
        );
        let bytes = builder.build();
        let rows = Pdb::new(&bytes).unwrap().rows::<TrackRow>();
        assert_eq!(rows[0].container, expected);
        assert!(rows[0].container.name().is_some());
        // ...and it is not the disc number, which sits at 0x4c.
        assert_eq!(rows[0].disc_number, 1);
    }
}

#[test]
fn an_unmodelled_container_value_still_decodes_the_row() {
    let mut builder = PdbBuilder::default();
    builder.add(
        PageType::TRACKS,
        track_row(&TrackFields {
            id: 1,
            title: "T",
            container: 99,
            disc_number: 1,
            ..TrackFields::default()
        }),
    );
    let raw = builder.build();
    let rows = Pdb::new(&raw).unwrap().rows::<TrackRow>();
    assert_eq!(rows[0].container, Container(99));
    assert_eq!(rows[0].container.name(), None);
    assert_eq!(format!("{:?}", rows[0].container), "Container(99)");
}

#[test]
fn a_zero_string_offset_is_an_absent_field_not_the_row_header_as_text() {
    // Offset 0 points at the row's own magic, so it cannot be a string.
    // Dereferencing it yields convincing garbage rather than an error, which is
    // the worst kind of bug: it surfaced as a mangled comment on a browse UI.
    let raw = sample();
    let rows = Pdb::new(&raw).unwrap().rows::<TrackRow>();
    let track = &rows[0];
    assert_eq!(track.strings.comment.as_str(), "");
    assert_eq!(track.strings.date_added.as_str(), "");
    assert_eq!(track.strings.isrc.as_str(), "");
    // ...while the slots that were populated still decode.
    assert_eq!(track.strings.title.as_str(), "Blue Monday");
    assert_eq!(track.strings.file_path.as_str(), "/Contents/blue.mp3");
    assert_eq!(track.strings.filename.as_str(), "blue.mp3");
}

#[test]
fn the_far_name_offset_forms_are_read_rather_than_dropped() {
    // Both far forms leave the near offset at zero, so a reader that ignores
    // them decodes the row header as text and files it under a bogus id.
    let mut builder = PdbBuilder::default();
    builder
        .add(PageType::ARTISTS, artist_row(1, "Near", false))
        .add(PageType::ARTISTS, artist_row(2, "Far", true))
        .add(PageType::ALBUMS, album_row(1, "Near album", 1, false))
        .add(PageType::ALBUMS, album_row(2, "Far album", 2, true));
    let raw = builder.build();
    let pdb = Pdb::new(&raw).unwrap();

    let artists = pdb.rows::<ArtistRow>();
    assert_eq!(artists.len(), 2);
    assert_eq!(artists[1].subtype, ARTIST_SUBTYPE_FAR);
    assert_eq!(artists[1].name.as_str(), "Far");

    let albums = pdb.rows::<AlbumRow>();
    assert_eq!(albums.len(), 2);
    assert_eq!(albums[0].subtype, ALBUM_SUBTYPE_NEAR);
    assert_eq!(albums[1].subtype, ALBUM_SUBTYPE_FAR);
    assert_eq!(albums[1].name.as_str(), "Far album");
    assert_eq!(albums[1].id, 2, "the id must come from the fixed part");
}

#[test]
fn a_row_with_an_unknown_subtype_is_dropped_rather_than_misread() {
    let mut builder = PdbBuilder::default();
    let mut bad = album_row(9, "Bogus", 1, false);
    bad[0] = 0x88;
    builder
        .add(PageType::ALBUMS, album_row(1, "Real", 1, false))
        .add(PageType::ALBUMS, bad);
    let raw = builder.build();
    let albums = Pdb::new(&raw).unwrap().rows::<AlbumRow>();
    assert_eq!(albums.len(), 1, "a guess here overwrites a real album");
    assert_eq!(albums[0].name.as_str(), "Real");
}

#[test]
fn history_entries_have_a_different_field_order_from_playlist_entries() {
    // Both are three little-endian u32s; reading one with the other's layout
    // produces a playlist that is plausible and wrong.
    let raw = sample();
    let pdb = Pdb::new(&raw).unwrap();
    let entries = pdb.rows::<PlaylistEntryRow>();
    assert_eq!(entries[0].entry_index, 2);
    assert_eq!(entries[0].track_id, 102);
    assert_eq!(entries[0].playlist_id, 11);

    let history = pdb.rows::<HistoryEntryRow>();
    assert_eq!(history[0].track_id, 103);
    assert_eq!(history[0].playlist_id, 1);
    assert_eq!(history[0].entry_index, 2);
}

#[test]
fn every_side_table_decodes() {
    let raw = sample();
    let pdb = Pdb::new(&raw).unwrap();
    assert_eq!(pdb.rows::<GenreRow>()[0].name.as_str(), "Techno");
    assert_eq!(pdb.rows::<LabelRow>()[0].name.as_str(), "Factory");
    let keys = pdb.rows::<KeyRow>();
    assert_eq!(keys[0].id, 3);
    assert_eq!(keys[0].id2, 3, "the second copy always matches");
    assert_eq!(keys[0].name.as_str(), "8A");
    let colors = pdb.rows::<ColorRow>();
    assert_eq!(colors[0].id, 5, "the colour id is one byte, at 0x05");
    assert_eq!(colors[0].name.as_str(), "Green");
    assert_eq!(
        pdb.rows::<ArtworkRow>()[0].path.as_str(),
        "/PIONEER/ARTWORK/a.jpg"
    );
    assert_eq!(
        pdb.rows::<HistoryPlaylistRow>()[0].name.as_str(),
        "HISTORY 001"
    );
}

#[test]
fn rejects_a_file_shorter_than_one_page() {
    let error = Pdb::new(b"too short").unwrap_err();
    assert!(error.is_truncated(), "got {error:?}");
}

#[test]
fn rejects_an_unexpected_page_size() {
    let mut raw = vec![0u8; PAGE_LEN];
    put_u32(&mut raw, 0x04, 2048);
    put_u32(&mut raw, 0x08, 1);
    assert!(matches!(Pdb::new(&raw), Err(Error::Malformed { .. })));
}

#[test]
fn a_buffer_of_zeroes_is_an_error_and_not_an_empty_medium() {
    // The worst outcome for a file arriving over a network: a truncated
    // download that presents as a stick with nothing on it.
    let error = Pdb::new(&vec![0u8; PAGE_LEN * 4]).unwrap_err();
    assert!(matches!(error, Error::Malformed { .. }), "got {error:?}");
    assert!(!error.is_truncated());
}

#[test]
fn the_stable_digest_ignores_the_players_write_counter() {
    // F13. Pulling the same database over NFS and then off the ejected stick
    // produced files differing in exactly two header fields and nowhere else,
    // because the deck wrote a play count between the reads.
    let original = sample();
    let mut touched = original.clone();
    put_u32(&mut touched, 0x10, 5);
    put_u32(&mut touched, 0x14, 20586);
    assert_ne!(original, touched);
    assert_eq!(stable_digest(&original), stable_digest(&touched));
}

#[test]
fn the_stable_digest_still_notices_a_real_change() {
    let original = sample();
    let mut changed = original.clone();
    // A byte just past the volatile window must still count.
    changed[0x18] ^= 0xff;
    assert_ne!(stable_digest(&original), stable_digest(&changed));

    let mut builder = PdbBuilder::default();
    builder.add(
        PageType::TRACKS,
        track_row(&TrackFields {
            id: 999,
            title: "Different",
            path: "/x.mp3",
            ..TrackFields::default()
        }),
    );
    assert_ne!(stable_digest(&original), stable_digest(&builder.build()));
}

#[test]
fn the_stable_digest_handles_a_runt_file() {
    // Must not panic on a file shorter than the volatile window.
    assert_ne!(stable_digest(b"tiny"), stable_digest(b"tiny "));
}

#[test]
fn the_library_resolves_foreign_keys_and_orders_playlists() {
    let raw = sample();
    let library = Library::parse(&raw).unwrap();

    let track = &library.tracks[&101];
    assert_eq!(track.artist, "New Order");
    assert_eq!(track.album, "Power Corruption & Lies");
    assert_eq!(track.genre, "");
    assert_eq!(track.bpm(), 130.0);
    assert_eq!(track.duration_text(), "4:05");
    assert_eq!(
        track.analyze_ext_path().as_deref(),
        Some("/PIONEER/USBANLZ/P001/00001/ANLZ0000.EXT")
    );
    assert_eq!(library.tracks[&102].analyze_ext_path(), None);
    assert_eq!(library.tracks[&103].artist, "夜のテーマ");
    assert_eq!(library.tracks[&103].container, Container::FLAC);
    // The referenced row's id travels on the wire, not the track's own.
    assert_eq!(track.artist_id, 1);

    let friday = &library.playlists[&11];
    assert_eq!(friday.name, "Friday");
    assert!(!friday.is_folder);
    assert_eq!(
        friday.track_ids,
        vec![101, 102],
        "rows were written 102 then 101; entry_index decides"
    );
    assert_eq!(
        library
            .playlist_tracks(11)
            .iter()
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Blue Monday", "Temptation"]
    );

    let folder = &library.playlists[&10];
    assert!(folder.is_folder);
    assert_eq!(folder.children, vec![11]);
    assert_eq!(
        library
            .root_playlists()
            .iter()
            .map(|p| p.id)
            .collect::<Vec<_>>(),
        vec![10]
    );

    let summary = library.summary();
    assert_eq!(summary.tracks, 3);
    assert_eq!(summary.playlists, 1);
    assert_eq!(summary.folders, 1);
    assert_eq!(
        library
            .search("monday")
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        vec![101]
    );

    let history = &library.history[&1];
    assert_eq!(history.name, "HISTORY 001");
    assert_eq!(history.track_ids, vec![101, 103], "sorted by entry index");
}

// -- the real export ---------------------------------------------------------

/// The real 675 KB export, if it is present.
fn real_export() -> Option<Vec<u8>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/export.pdb");
    std::fs::read(path).ok()
}

macro_rules! real {
    () => {
        match real_export() {
            Some(data) => data,
            None => {
                eprintln!("testdata/export.pdb absent; the fixture floor above still ran");
                return;
            }
        }
    };
}

#[test]
fn the_real_export_holds_the_counts_it_holds() {
    let raw = real!();
    assert_eq!(raw.len(), 675_840, "165 pages");
    let pdb = Pdb::new(&raw).unwrap();
    assert_eq!(u64::from(pdb.page_size()), PAGE_SIZE);
    assert_eq!(pdb.tables().len(), 20);

    assert_eq!(pdb.rows::<TrackRow>().len(), 651);
    assert_eq!(pdb.rows::<ArtistRow>().len(), 329);
    assert_eq!(pdb.rows::<AlbumRow>().len(), 274);
    assert_eq!(pdb.rows::<GenreRow>().len(), 22);
    assert_eq!(pdb.rows::<LabelRow>().len(), 50);
    assert_eq!(pdb.rows::<KeyRow>().len(), 24);
    assert_eq!(pdb.rows::<ColorRow>().len(), 8);
    assert_eq!(pdb.rows::<ArtworkRow>().len(), 576);
    assert_eq!(pdb.rows::<PlaylistTreeRow>().len(), 1);
    assert_eq!(pdb.rows::<PlaylistEntryRow>().len(), 40);
    assert_eq!(pdb.rows::<HistoryPlaylistRow>().len(), 7);
    assert_eq!(pdb.rows::<HistoryEntryRow>().len(), 83);
    assert_eq!(pdb.rows::<ColumnRow>().len(), 27);

    // Page types 17 and 18 carry rows that nothing names. The walk must reach
    // them without a decoder, or an unknown table would look like an empty one.
    let counts: BTreeMap<u32, usize> = pdb
        .row_counts()
        .into_iter()
        .map(|(page_type, count)| (page_type.0, count))
        .collect();
    assert_eq!(counts[&17], 22);
    assert_eq!(counts[&18], 17);
    assert_eq!(counts[&19], 1);
}

#[test]
fn the_real_exports_playlist_holds_all_forty_of_its_tracks() {
    // F47, on the very page the finding came from: page 18 declares
    // low count byte 28 (284 slots including its high bits), with bookkeeping
    // value 39 at 0x22. Presence masks mark 16 + 16 + 8 = 40 rows live. The fortieth is track 651, the AIFF.
    let raw = real!();
    let pdb = Pdb::new(&raw).unwrap();
    let pages = pdb.pages(PageType::PLAYLIST_ENTRIES);
    let (index, header) = pages
        .iter()
        .find(|(_, header)| !pdb.row_offsets(0, header).is_empty() || header.num_rows() > 0)
        .copied()
        .unwrap();
    assert_eq!(header.num_rows_small, 28);
    assert_eq!(header.num_rows_large, 39);
    assert_eq!(
        header.num_rows(),
        284,
        "the packed count covers every index group"
    );
    assert_eq!(
        pdb.row_offsets(index, &header).len(),
        40,
        "the masks say 40 and the masks are right"
    );

    let library = Library::parse(&raw).unwrap();
    let playlist = library.playlists.values().next().unwrap();
    assert_eq!(playlist.name, "test_formats");
    assert_eq!(playlist.track_count(), 40);
    assert_eq!(playlist.track_ids.first(), Some(&612));
    assert_eq!(
        playlist.track_ids.last(),
        Some(&651),
        "the row the header's count would have dropped"
    );
}

#[test]
fn the_real_export_decodes_its_non_ascii_names() {
    // O6, and the reason this file is committed. Decoding UTF-16 big-endian
    // from offset+3 instead of little-endian from offset+4 is byte-for-byte
    // identical for ASCII, so a round-trip suite and a 692-track parse both
    // pass while every one of these comes out as mojibake — and on the serve
    // side that became NFSERR_NOENT on 24 of 692 tracks.
    let raw = real!();
    let library = Library::parse(&raw).unwrap();

    assert_eq!(library.artists[&32], "Разные исполнители");
    assert_eq!(library.artists[&52], "Rene Wise & Rødhåd");
    assert_eq!(library.artists[&94], "Chlär");
    assert_eq!(library.artists[&112], "Félicie");
    assert_eq!(library.tracks[&48].title, "Obéissance");
    assert_eq!(library.tracks[&89].title, "人々の繋がり");
    assert_eq!(library.tracks[&289].title, "Impulskörper");
    assert_eq!(
        library.tracks[&89].file_path, "/Contents/Akiba/LOST DREAMS/03. Akiba - 人々の繋がり.mp3",
        "a path the deck asks us for by name"
    );

    let non_ascii = library
        .artists
        .values()
        .filter(|name| !name.is_ascii())
        .count();
    assert_eq!(non_ascii, 17, "17 of 329 artists; this file can catch it");
}

#[test]
fn the_real_export_decodes_its_isrcs_as_ascii() {
    // The ISRC form is a 0x90-framed string whose payload starts with 0x03 and
    // is NUL-terminated ASCII. Read as UTF-16 it becomes CJK characters, with
    // no error to notice.
    let raw = real!();
    let library = Library::parse(&raw).unwrap();
    assert_eq!(library.tracks[&3].isrc, "GBUMC2400001");
    assert_eq!(library.tracks[&8].isrc, "CH6542320468");
    let with_isrc = library
        .tracks
        .values()
        .filter(|track| !track.isrc.is_empty())
        .count();
    assert_eq!(with_isrc, 245, "245 of 651 rows carry one");
    assert!(
        library.tracks.values().all(|track| track.isrc.is_ascii()),
        "an ISRC is ASCII by definition; anything else is a decode bug"
    );
}

#[test]
fn the_real_export_holds_every_container_the_format_matrix_covers() {
    // F34, from the medium the finding was built on: one source track rendered
    // into every format a CDJ-2000NXS accepts.
    let raw = real!();
    let library = Library::parse(&raw).unwrap();
    let mut histogram: BTreeMap<u16, usize> = BTreeMap::new();
    for track in library.tracks.values() {
        *histogram.entry(track.container.0).or_default() += 1;
    }
    assert_eq!(
        histogram,
        [(1, 626), (4, 8), (5, 1), (11, 12), (12, 4)]
            .into_iter()
            .collect::<BTreeMap<u16, usize>>(),
        "mp3, m4a, flac, wav, aiff — no exceptions within a container"
    );
}

#[test]
fn the_real_exports_far_form_album_is_recovered_and_collides_with_nothing() {
    // 273 of the 274 album rows are subtype 0x80 and one is 0x84 with its near
    // offset set to zero. A reader that follows the published schema decodes
    // that row's own header as text and files the mojibake under an id read
    // from the same misaligned bytes, overwriting a real album.
    let raw = real!();
    let pdb = Pdb::new(&raw).unwrap();
    let albums = pdb.rows::<AlbumRow>();
    assert_eq!(albums.len(), 274);

    let far: Vec<&AlbumRow> = albums
        .iter()
        .filter(|row| row.subtype == ALBUM_SUBTYPE_FAR)
        .collect();
    assert_eq!(far.len(), 1);
    assert_eq!(far[0].id, 166);
    assert_eq!(far[0].ofs_name_near, 0, "the near offset is unusable");
    assert_eq!(far[0].ofs_name_far, Some(0x18));
    assert_eq!(
        far[0].name.as_str().chars().next(),
        Some('\u{285}'),
        "a well-formed UTF-16 string lives at the far offset"
    );
    assert_eq!(far[0].name.as_str().chars().count(), 131);

    let library = Library::parse(&raw).unwrap();
    assert_eq!(
        library.albums.len(),
        274,
        "every album keeps its own id; nothing is overwritten"
    );
}

#[test]
fn the_real_exports_history_is_seven_playlists_of_eighty_three_entries() {
    let raw = real!();
    let library = Library::parse(&raw).unwrap();
    assert_eq!(library.history.len(), 7);
    assert_eq!(library.history[&1].name, "HISTORY 001");
    assert_eq!(library.history[&7].name, "HISTORY 007");
    let total: usize = library
        .history
        .values()
        .map(|playlist| playlist.track_ids.len())
        .sum();
    assert_eq!(total, 83);
    // The field order differs from a playlist entry; reading it wrong would put
    // every track in playlist 1.
    assert_eq!(library.history[&4].track_ids.len(), 20);
}

#[test]
fn the_real_exports_columns_are_the_browse_categories() {
    let raw = real!();
    let columns = Pdb::new(&raw).unwrap().rows::<ColumnRow>();
    assert_eq!(columns[0].label(), "GENRE");
    assert_eq!(columns[0].menu_item_type, 0x80);
    assert_eq!(columns[1].label(), "ARTIST");
    assert!(
        columns[0].name.as_str().starts_with('\u{fffa}'),
        "the annotation characters are in the file and must not be in the label"
    );
    let labels: Vec<&str> = columns.iter().map(ColumnRow::label).collect();
    assert!(labels.contains(&"DJ PLAY COUNT"));
    assert!(labels.contains(&"MATCHING"));
}

#[test]
fn the_real_exports_first_track_joins_end_to_end() {
    let raw = real!();
    let library = Library::parse(&raw).unwrap();
    let track = &library.tracks[&1];
    assert_eq!(track.title, "Mary Lynne");
    assert_eq!(track.artist, "Anf / Priori / Dust-e-1");
    assert_eq!(track.album, "Mauna Kea");
    assert_eq!(track.genre, "IDM");
    assert_eq!(track.key, "2A");
    assert_eq!(track.label, "Pacific Rhythm");
    assert_eq!(track.bpm(), 120.0);
    assert_eq!(track.duration, 398);
    assert_eq!(track.duration_text(), "6:38");
    assert_eq!(track.bitrate, 320);
    assert_eq!(track.sample_rate, 44100);
    assert_eq!(track.track_number, 4);
    assert_eq!(track.year, 2019);
    assert_eq!(track.container, Container::MP3);
    assert_eq!(track.date_added, "2026-01-03");
    assert_eq!(
        track.analyze_path,
        "/PIONEER/USBANLZ/P02E/0002B5FE/ANLZ0000.DAT"
    );
    assert_eq!(
        track.analyze_ext_path().as_deref(),
        Some("/PIONEER/USBANLZ/P02E/0002B5FE/ANLZ0000.EXT")
    );
    assert_eq!(
        track.file_path,
        "/Contents/Anf _ Priori _ Dust-e-1/Mauna Kea/4 - Anf, Priori, Dust-e-1 - Mary Lynne.mp3"
    );
    assert_eq!(track.file_size, 16_400_232);
    // The referenced rows' ids, which is what a menu item carries.
    assert_eq!(
        (track.artist_id, track.album_id, track.artwork_id),
        (1, 1, 1)
    );
}

#[test]
fn the_real_exports_summary_is_what_a_media_query_must_report() {
    // A deck will not list a medium whose counts are wrong (F24).
    let raw = real!();
    let summary = Library::parse(&raw).unwrap().summary();
    assert_eq!(summary.tracks, 651);
    assert_eq!(summary.artists, 329);
    assert_eq!(summary.albums, 274);
    assert_eq!(summary.genres, 22);
    assert_eq!(summary.keys, 24);
    assert_eq!(summary.playlists, 1);
    assert_eq!(summary.folders, 0);
}

#[test]
fn the_real_exports_digest_survives_a_simulated_deck_write() {
    // F13: the two fields that changed between an NFS pull and a direct read of
    // the same 1 MB database, and nothing else in a megabyte.
    let raw = real!();
    let mut touched = raw.clone();
    // unknown1 4 -> 5, sequence n -> n + 1.
    touched[0x10..0x14].copy_from_slice(&6u32.to_le_bytes());
    let sequence = u32::from_le_bytes(touched[0x14..0x18].try_into().unwrap());
    touched[0x14..0x18].copy_from_slice(&(sequence + 1).to_le_bytes());
    assert_ne!(raw, touched);
    assert_eq!(stable_digest(&raw), stable_digest(&touched));

    let mut retitled = raw.clone();
    let at = ROW_GROUP_LEN as usize; // any byte outside the volatile window
    retitled[PAGE_LEN + at] ^= 0xff;
    assert_ne!(stable_digest(&raw), stable_digest(&retitled));
}
