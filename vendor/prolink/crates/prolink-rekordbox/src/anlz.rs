// SPDX-License-Identifier: GPL-3.0-only

//! `ANLZ####.DAT` / `.EXT` — what rekordbox learned about one track.
//!
//! Beat grid, cue points, waveforms and, for MP3s, the seek index that makes
//! variable-bitrate playback possible. A player fetches these from the medium's
//! owner when it loads a track and refuses the load without them — browsing
//! works fine, pressing LOAD does not.
//!
//! The container is trivially simple: a `PMAI` header followed by a flat
//! sequence of tags, each a four-character identifier, a header size, a total
//! size, and a payload. Everything is **big-endian**, unlike the little-endian
//! `export.pdb` that points at these files.
//!
//! `.DAT` carries `PPTH PVBR PQTZ PWAV PWV2 PCOB`; `.EXT` adds
//! `PWV3 PCO2 PQT2 PWV5 PWV4 PSSI`; the CDJ-3000's `.2EX` adds `PWV6 PWV7`.
//! Which tags a file has depends on the rekordbox version that wrote it, so a
//! missing tag is normal rather than an error.
//!
//! # Two views, because there are two jobs
//!
//! **Serving** a track's analysis to a CDJ means *transforming* it: the file is
//! big-endian and the wire little-endian, and three of the five blobs change
//! layout as well (F30). That transform lives in `prolink-proto::analysis` and
//! takes raw payload bytes, so [`Tag::payload`] hands over the tag's bytes
//! exactly as rekordbox wrote them, byte for byte.
//!
//! **Consuming** it means interpreting it, so [`Tag::content`] carries a decode
//! of the tags that have a known meaning.
//!
//! Both come from one parse. A tag whose payload does not match its schema
//! leaves `content` as `None` and keeps its bytes: an unknown or malformed tag
//! costs that tag, not the file.
//!
//! # Two `PCOB` tags, not one
//!
//! Cues arrive in two tags of the same type — one holding memory cues and loops,
//! one holding hot cues — distinguished by the `list_type` word inside. The same
//! is true of `PCO2`. So [`AnlzFile::tag`] returning the first is usually wrong;
//! use [`AnlzFile::tags`] or [`AnlzFile::cue_lists`].
//!
//! # No captured file was available
//!
//! Every other format in this workspace is pinned against bytes off real
//! hardware. This one is not: there was no `.DAT` or `.EXT` on the machine this
//! was written on. The layouts come from the crate-digger Kaitai schema and the
//! project's own notes, and the tests below build synthetic tags. Where a field
//! is genuinely unsettled — which nibble of a `PWV2` byte holds the height — the
//! doc comment says so instead of picking one silently.

pub mod content;

use std::fmt;
use std::io::Cursor;

use binrw::BinRead;

use crate::error::{Error, Result};

pub use content::{
    AudioPath, Beat, BeatGrid, ColorDetailColumn, ColorPreviewColumn, Content, Cue, CueList,
    CueListType, CueStatus, CueType, ExtendedCue, ExtendedCueList, Phrase, PreviewColumn,
    SongStructure, TinyColumn, TinyWaveformPreview, TrackBank, TrackMood, VbrIndex,
    WaveformColorDetail, WaveformColorPreview, WaveformDetail, WaveformPreview,
};

/// A tag's four-character identifier.
///
/// A newtype rather than an enum, for the usual reason: rekordbox keeps
/// inventing tags — `PWV6` and `PWV7` arrived with the CDJ-3000 — and a reader
/// that refused an unfamiliar one would take out the tags it does understand.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FourCc(pub [u8; 4]);

impl FourCc {
    /// The file header itself.
    pub const PMAI: Self = Self(*b"PMAI");
    /// Path of the audio file this analysis belongs to.
    pub const PPTH: Self = Self(*b"PPTH");
    /// Seek index for variable-bitrate MP3s. **Gates playback** (F30).
    pub const PVBR: Self = Self(*b"PVBR");
    /// Beat grid.
    pub const PQTZ: Self = Self(*b"PQTZ");
    /// Extended beat grid, `.EXT` only. Layout not modelled here.
    pub const PQT2: Self = Self(*b"PQT2");
    /// Monochrome waveform preview, for the strip above the jog wheel.
    pub const PWAV: Self = Self(*b"PWAV");
    /// Smaller monochrome preview, for the CDJ-900.
    pub const PWV2: Self = Self(*b"PWV2");
    /// Monochrome scrolling waveform, `.EXT`.
    pub const PWV3: Self = Self(*b"PWV3");
    /// Colour waveform preview, `.EXT`.
    pub const PWV4: Self = Self(*b"PWV4");
    /// Colour scrolling waveform, `.EXT`.
    pub const PWV5: Self = Self(*b"PWV5");
    /// Three-band preview, CDJ-3000, `.2EX`. Layout not modelled here.
    pub const PWV6: Self = Self(*b"PWV6");
    /// Three-band scrolling waveform, CDJ-3000, `.2EX`. Not modelled here.
    pub const PWV7: Self = Self(*b"PWV7");
    /// Memory cues and loops, or hot cues and loops.
    pub const PCOB: Self = Self(*b"PCOB");
    /// One entry inside a `PCOB`.
    pub const PCPT: Self = Self(*b"PCPT");
    /// The nexus-2 cue list, with names and colours.
    pub const PCO2: Self = Self(*b"PCO2");
    /// One entry inside a `PCO2`.
    pub const PCP2: Self = Self(*b"PCP2");
    /// Song structure: intro, verse, chorus, outro.
    pub const PSSI: Self = Self(*b"PSSI");

    /// The identifier as text, or `None` if it is not printable ASCII.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0)
            .ok()
            .filter(|s| s.chars().all(|c| c.is_ascii_graphic()))
    }
}

impl fmt::Debug for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_str() {
            Some(text) => f.write_str(text),
            None => write!(f, "FourCc({:02x?})", self.0),
        }
    }
}

/// Bytes of a tag header before its payload: identifier, header size, total
/// size.
pub const TAG_HEADER_LEN: u32 = 12;

/// One tag: its identifier, its bytes, and a decode of them if we have one.
///
/// # Four views of the same bytes, and why each exists
///
/// A tag's fixed fields live *inside its declared header*: a `PWAV` header is
/// 20 bytes — the twelve common ones, a length and an unknown word — and only
/// the column data follows. So:
///
/// | | | |
/// |---|---|---|
/// | [`Tag::raw`] | the whole tag | archival |
/// | [`Tag::payload`] | from `header_len` | what a dbserver reply transforms |
/// | [`Tag::header_extra`] | bytes 12..`header_len` | the tag's own fixed fields |
/// | [`Tag::body`] | from byte 12 | the last two together, what [`Content`] decodes |
///
/// Conflating `payload` and `body` is easy and silent: on a tag whose header
/// really is twelve bytes they are identical, and `PVBR` is one such.
///
/// The split is not arbitrary — it is what `prolink-proto::analysis` asks for.
/// Its `beat_grid` takes a `PQTZ` payload meaning "the entries alone, with the
/// tag's own header already stripped", its `waveform_preview` takes the packed
/// `PWAV` columns with no length word in front, and its `waveform_detail` takes
/// the `PWV3` entries *plus* the entry width, which is the first word of
/// [`Tag::header_extra`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Tag {
    /// The four-character identifier.
    pub fourcc: FourCc,
    /// Declared header length, twelve or more.
    pub header_len: u32,
    /// Declared total length, header included.
    pub total_len: u32,
    /// The whole tag, header included, exactly as written.
    pub raw: Vec<u8>,
    /// A decode of [`Tag::body`], or `None` for a tag this crate does not model
    /// and for one whose payload did not match its schema. The bytes are still
    /// in [`Tag::raw`] either way.
    pub content: Option<Content>,
}

impl Tag {
    /// The variable-length data, from the declared header length onwards.
    ///
    /// Byte-exact, because serving a track's analysis is a *transform* of
    /// exactly these bytes and `prolink-proto::analysis` takes them as an
    /// argument.
    pub fn payload(&self) -> &[u8] {
        let start = usize::try_from(self.header_len).unwrap_or(usize::MAX);
        self.raw.get(start..).unwrap_or_default()
    }

    /// Everything after the twelve-byte common header: the tag's own fixed
    /// fields followed by its payload.
    pub fn body(&self) -> &[u8] {
        let start = usize::try_from(TAG_HEADER_LEN).unwrap_or(usize::MAX);
        self.raw.get(start..).unwrap_or_default()
    }

    /// The fixed fields between the common header and the payload.
    pub fn header_extra(&self) -> &[u8] {
        let start = usize::try_from(TAG_HEADER_LEN).unwrap_or(usize::MAX);
        let end = usize::try_from(self.header_len).unwrap_or(usize::MAX);
        self.raw.get(start..end.max(start)).unwrap_or_default()
    }
}

/// A parsed ANLZ file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AnlzFile {
    /// Declared header length; the first tag starts here.
    pub header_len: u32,
    /// Declared file length. Tags are read up to this, not to the end of the
    /// buffer, because some files carry trailing slack that is not a tag.
    pub file_len: u32,
    /// Header bytes past the three declared fields.
    pub header_extra: Vec<u8>,
    /// The tags, in file order.
    pub tags: Vec<Tag>,
}

impl AnlzFile {
    /// Parse a `.DAT`, `.EXT` or `.2EX`.
    ///
    /// Fails only when the file is not an ANLZ file at all — wrong magic, or
    /// too short to hold a header. Everything past that point degrades: a tag
    /// whose length runs off the end stops the walk and keeps what came before,
    /// because half a waveform is better than no cue points.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let buffer_len = u64::try_from(data.len()).unwrap_or(u64::MAX);
        if buffer_len < u64::from(TAG_HEADER_LEN) {
            return Err(Error::truncated(0, u64::from(TAG_HEADER_LEN), buffer_len));
        }
        let magic = data.get(..4).unwrap_or_default();
        if magic != FourCc::PMAI.0 {
            return Err(Error::bad_magic(0, &FourCc::PMAI.0, magic));
        }
        let header_len = u32_at(data, 4).unwrap_or(0);
        let file_len = u32_at(data, 8).unwrap_or(0);

        let header_end = usize::try_from(header_len).unwrap_or(usize::MAX);
        let header_extra = data
            .get(TAG_HEADER_LEN.try_into().unwrap_or(usize::MAX)..header_end)
            .unwrap_or_default()
            .to_vec();

        // Stop at the declared file size where it is plausible, at the buffer
        // otherwise: a file truncated in transit declares more than it holds.
        let declared = usize::try_from(file_len).unwrap_or(usize::MAX);
        let end = if file_len == 0 {
            data.len()
        } else {
            declared.min(data.len())
        };

        let mut tags = Vec::new();
        let mut offset = header_end;
        while let Some(tag) = read_tag(data, offset, end) {
            offset = offset.saturating_add(usize::try_from(tag.total_len).unwrap_or(usize::MAX));
            tags.push(tag);
        }

        Ok(Self {
            header_len,
            file_len,
            header_extra,
            tags,
        })
    }

    /// The first tag with this identifier.
    ///
    /// Wrong for cues, which come in pairs; see [`AnlzFile::tags`].
    pub fn tag(&self, fourcc: FourCc) -> Option<&Tag> {
        self.tags.iter().find(|tag| tag.fourcc == fourcc)
    }

    /// Every tag with this identifier, in file order.
    pub fn tags(&self, fourcc: FourCc) -> impl Iterator<Item = &Tag> {
        self.tags.iter().filter(move |tag| tag.fourcc == fourcc)
    }

    /// The payload of the first tag with this identifier, byte-exact.
    ///
    /// `None` rather than empty when the tag is absent: a track analysed by an
    /// older rekordbox legitimately lacks the newer tags, and that is not the
    /// same as a tag that is present and empty.
    pub fn payload(&self, fourcc: FourCc) -> Option<&[u8]> {
        self.tag(fourcc).map(Tag::payload)
    }

    /// Every identifier in the file, for logs.
    pub fn fourccs(&self) -> Vec<FourCc> {
        self.tags.iter().map(|tag| tag.fourcc).collect()
    }

    /// The audio file's path, from `PPTH`.
    pub fn path(&self) -> Option<&str> {
        match self.tag(FourCc::PPTH)?.content.as_ref()? {
            Content::Path(path) => Some(path.as_str()),
            _ => None,
        }
    }

    /// The beat grid, from `PQTZ`.
    pub fn beat_grid(&self) -> Option<&BeatGrid> {
        match self.tag(FourCc::PQTZ)?.content.as_ref()? {
            Content::BeatGrid(grid) => Some(grid),
            _ => None,
        }
    }

    /// The variable-bitrate seek index, from `PVBR`.
    ///
    /// Without this a player cannot map playing time to a byte offset, so it
    /// never issues a single read — a load that resolves the path perfectly and
    /// then does nothing (F30).
    pub fn vbr_index(&self) -> Option<&VbrIndex> {
        match self.tag(FourCc::PVBR)?.content.as_ref()? {
            Content::VbrIndex(index) => Some(index),
            _ => None,
        }
    }

    /// Both `PCOB` cue lists, memory and hot.
    pub fn cue_lists(&self) -> impl Iterator<Item = &CueList> {
        self.tags(FourCc::PCOB)
            .filter_map(|tag| match tag.content.as_ref()? {
                Content::CueList(list) => Some(list),
                _ => None,
            })
    }

    /// Both `PCO2` cue lists, memory and hot.
    pub fn extended_cue_lists(&self) -> impl Iterator<Item = &ExtendedCueList> {
        self.tags(FourCc::PCO2)
            .filter_map(|tag| match tag.content.as_ref()? {
                Content::ExtendedCueList(list) => Some(list),
                _ => None,
            })
    }

    /// The song structure, from `PSSI`.
    pub fn song_structure(&self) -> Option<&SongStructure> {
        match self.tag(FourCc::PSSI)?.content.as_ref()? {
            Content::SongStructure(structure) => Some(structure),
            _ => None,
        }
    }
}

/// Read one tag at `offset`, or `None` when the walk should stop.
fn read_tag(data: &[u8], offset: usize, end: usize) -> Option<Tag> {
    let header_end = offset.checked_add(usize::try_from(TAG_HEADER_LEN).ok()?)?;
    if header_end > end {
        return None;
    }
    let fourcc = FourCc(data.get(offset..offset + 4)?.try_into().ok()?);
    let header_len = u32_at(data, offset + 4)?;
    let total_len = u32_at(data, offset + 8)?;
    if total_len < TAG_HEADER_LEN || header_len < TAG_HEADER_LEN || header_len > total_len {
        return None;
    }
    let tag_end = offset.checked_add(usize::try_from(total_len).ok()?)?;
    if tag_end > end {
        return None;
    }
    let raw = data.get(offset..tag_end)?.to_vec();
    // Decode from after the *common* header, not the declared one: a tag's own
    // fixed fields live inside its header. See `Tag`.
    let body_start = usize::try_from(TAG_HEADER_LEN).ok()?;
    let content = raw
        .get(body_start..)
        .and_then(|body| Content::parse(fourcc, body));
    Some(Tag {
        fourcc,
        header_len,
        total_len,
        raw,
        content,
    })
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let raw: [u8; 4] = data.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(raw))
}

/// Read a value from a payload, big-endian, requiring nothing of what follows.
pub(crate) fn decode<T: for<'a> BinRead<Args<'a> = ()>>(payload: &[u8]) -> Option<T> {
    T::read_be(&mut Cursor::new(payload)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tag with its fixed fields inside the header, as rekordbox does.
    fn tag(fourcc: [u8; 4], header_extra: &[u8], payload: &[u8]) -> Vec<u8> {
        let header_len = TAG_HEADER_LEN + u32::try_from(header_extra.len()).unwrap();
        let total = header_len + u32::try_from(payload.len()).unwrap();
        let mut out = fourcc.to_vec();
        out.extend_from_slice(&header_len.to_be_bytes());
        out.extend_from_slice(&total.to_be_bytes());
        out.extend_from_slice(header_extra);
        out.extend_from_slice(payload);
        out
    }

    /// Wrap tags in a `PMAI` header, as rekordbox does.
    fn file(tags: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = tags.concat();
        let total = TAG_HEADER_LEN + u32::try_from(body.len()).unwrap();
        let mut out = b"PMAI".to_vec();
        out.extend_from_slice(&TAG_HEADER_LEN.to_be_bytes());
        out.extend_from_slice(&total.to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// A `PPTH`: a 16-byte header holding the length, then the UTF-16BE path.
    fn path_tag(path: &str) -> Vec<u8> {
        let mut text: Vec<u8> = path.encode_utf16().flat_map(u16::to_be_bytes).collect();
        text.extend_from_slice(&[0, 0]);
        let len = u32::try_from(text.len()).unwrap().to_be_bytes();
        tag(*b"PPTH", &len, &text)
    }

    /// A `PQTZ`: a 24-byte header holding two unknowns and the count.
    fn beat_grid_tag(beats: &[(u16, u16, u32)]) -> Vec<u8> {
        let mut header = vec![0u8; 8];
        header.extend_from_slice(&u32::try_from(beats.len()).unwrap().to_be_bytes());
        let mut payload = Vec::new();
        for (number, tempo, time) in beats {
            payload.extend_from_slice(&number.to_be_bytes());
            payload.extend_from_slice(&tempo.to_be_bytes());
            payload.extend_from_slice(&time.to_be_bytes());
        }
        tag(*b"PQTZ", &header, &payload)
    }

    #[test]
    fn rejects_a_file_without_the_pmai_magic() {
        assert!(matches!(
            AnlzFile::parse(b"NOPE\0\0\0\x0c\0\0\0\x0c"),
            Err(Error::BadMagic { .. })
        ));
    }

    #[test]
    fn rejects_a_file_too_short_to_hold_a_header() {
        let error = AnlzFile::parse(b"PMAI").unwrap_err();
        assert!(error.is_truncated(), "got {error:?}");
    }

    #[test]
    fn the_payload_starts_at_the_declared_header_and_the_body_at_byte_twelve() {
        // The distinction that is easy to lose: a PPTH declares a 16-byte
        // header, so its payload is the text alone and its body is the length
        // word plus the text. Serving wants the first, decoding the second.
        let raw = file(&[path_tag("/Contents/a.mp3")]);
        let parsed = AnlzFile::parse(&raw).unwrap();
        let tag = parsed.tag(FourCc::PPTH).unwrap();
        assert_eq!(tag.header_len, 16);
        assert_eq!(tag.payload().len(), tag.body().len() - 4);
        assert_eq!(tag.header_extra().len(), 4);
        assert_eq!(
            tag.payload(),
            raw.get(28..).unwrap(),
            "the bytes a transform receives must be exactly the file's"
        );
        assert_eq!(parsed.path(), Some("/Contents/a.mp3"));
    }

    #[test]
    fn an_unknown_tag_costs_that_tag_and_not_the_file() {
        let raw = file(&[
            tag(*b"XXXX", &[], &[1, 2, 3, 4]),
            beat_grid_tag(&[(1, 12800, 0), (2, 12800, 469)]),
        ]);
        let parsed = AnlzFile::parse(&raw).unwrap();
        assert_eq!(parsed.fourccs(), vec![FourCc(*b"XXXX"), FourCc::PQTZ]);
        assert!(parsed.tags.first().unwrap().content.is_none());
        assert_eq!(parsed.beat_grid().unwrap().beats.len(), 2);
    }

    #[test]
    fn a_malformed_payload_costs_the_decode_but_not_the_bytes() {
        // A beat grid claiming 1000 beats it does not carry.
        let mut header = vec![0u8; 8];
        header.extend_from_slice(&1000u32.to_be_bytes());
        let raw = file(&[tag(*b"PQTZ", &header, &[0; 8])]);
        let parsed = AnlzFile::parse(&raw).unwrap();
        let grid = parsed.tag(FourCc::PQTZ).unwrap();
        assert!(grid.content.is_none(), "the decode must not be invented");
        assert_eq!(
            grid.payload(),
            [0; 8],
            "the bytes must still be there to serve"
        );
    }

    #[test]
    fn a_tag_running_past_the_file_stops_the_walk_and_keeps_the_rest() {
        let mut raw = file(&[path_tag("/a.mp3"), tag(*b"PQTZ", &[], &[0; 32])]);
        // Truncate mid-way through the second tag.
        raw.truncate(raw.len() - 8);
        let parsed = AnlzFile::parse(&raw).unwrap();
        assert_eq!(parsed.fourccs(), vec![FourCc::PPTH]);
    }

    #[test]
    fn the_declared_file_length_bounds_the_walk() {
        let mut raw = file(&[path_tag("/a.mp3")]);
        let kept = raw.len();
        raw.extend_from_slice(&tag(*b"PQTZ", &[], &[0; 12]));
        // The header still declares the shorter length, so the extra is slack.
        let parsed = AnlzFile::parse(&raw).unwrap();
        assert_eq!(parsed.file_len, u32::try_from(kept).unwrap());
        assert_eq!(parsed.fourccs(), vec![FourCc::PPTH]);
    }

    #[test]
    fn both_cue_lists_are_reachable() {
        let cue_list = |kind: u32| {
            let mut body = kind.to_be_bytes().to_vec();
            body.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
            tag(*b"PCOB", &[], &body)
        };
        let raw = file(&[cue_list(0), cue_list(1)]);
        let parsed = AnlzFile::parse(&raw).unwrap();
        let kinds: Vec<CueListType> = parsed.cue_lists().map(|list| list.list_type).collect();
        assert_eq!(
            kinds,
            vec![CueListType::MEMORY, CueListType::HOT],
            "tag() alone would have returned only the memory cues"
        );
    }

    #[test]
    fn a_fourcc_prints_as_its_identifier() {
        assert_eq!(format!("{:?}", FourCc::PQTZ), "PQTZ");
        assert_eq!(
            format!("{:?}", FourCc([0, 1, 2, 3])),
            "FourCc([00, 01, 02, 03])"
        );
    }
}
