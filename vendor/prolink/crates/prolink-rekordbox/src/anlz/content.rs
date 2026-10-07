// SPDX-License-Identifier: GPL-3.0-only

//! Decoded ANLZ tag payloads — the *consume* view of an analysis file.
//!
//! Everything here is big-endian. Every type is reached from
//! [`Content::parse`], which takes a tag's bytes **after the twelve-byte common
//! header** — not after the declared header length, because rekordbox puts a
//! tag's fixed fields inside its header and only the variable-length array
//! after it. So a `PWAV` declares a 20-byte header: twelve common bytes, a
//! length and an unknown word. [`super::Tag::payload`] gives the array alone,
//! which is what the wire transform wants; [`super::Tag::body`] gives both,
//! which is what these decoders want.
//!
//! # What is measured and what is not
//!
//! The container, the beat grid, the cue lists and the monochrome waveform
//! packing are all pinned by the protocol work. Two things are not, and are
//! marked in place:
//!
//! - Which nibble of a `PWV2` byte holds the height. Nothing in this project
//!   needed it, and the schema does not say.
//! - The `PSSI` phrase-kind numbering, which is taken from the published
//!   analysis rather than observed here.
//!
//! The `PWV5` bit layout *was* measured, and against the interpretation in
//! common circulation — see [`ColorDetailColumn`].

use std::io::Cursor;

use binrw::{BinRead, binread, helpers::until_eof};

use super::{FourCc, decode};

/// A decoded tag payload.
///
/// Only the tags with a known meaning appear here. Anything else leaves
/// [`super::Tag::content`] as `None` with its bytes intact.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Content {
    /// `PPTH` — the audio file's path.
    Path(AudioPath),
    /// `PVBR` — the variable-bitrate seek index.
    VbrIndex(VbrIndex),
    /// `PQTZ` — the beat grid.
    BeatGrid(BeatGrid),
    /// `PWAV` — the monochrome waveform preview.
    WaveformPreview(WaveformPreview),
    /// `PWV2` — the smaller monochrome preview.
    TinyWaveformPreview(TinyWaveformPreview),
    /// `PWV3` — the monochrome scrolling waveform.
    WaveformDetail(WaveformDetail),
    /// `PWV4` — the colour waveform preview.
    WaveformColorPreview(WaveformColorPreview),
    /// `PWV5` — the colour scrolling waveform.
    WaveformColorDetail(WaveformColorDetail),
    /// `PCOB` — memory cues and loops, or hot cues and loops.
    CueList(CueList),
    /// `PCO2` — the nexus-2 cue list, with names and colours.
    ExtendedCueList(ExtendedCueList),
    /// `PSSI` — the song structure.
    SongStructure(SongStructure),
}

impl Content {
    /// Decode `body`, the tag's bytes after the twelve-byte common header.
    ///
    /// `None` for a tag this crate does not model and for one whose payload
    /// does not match its schema — a length that runs off the end, a count that
    /// cannot be satisfied. Either way the caller keeps the bytes.
    pub fn parse(fourcc: FourCc, body: &[u8]) -> Option<Self> {
        Some(match fourcc {
            FourCc::PPTH => Self::Path(AudioPath::parse(body)?),
            FourCc::PVBR => Self::VbrIndex(decode(body)?),
            FourCc::PQTZ => Self::BeatGrid(decode(body)?),
            FourCc::PWAV => Self::WaveformPreview(decode(body)?),
            FourCc::PWV2 => Self::TinyWaveformPreview(decode(body)?),
            FourCc::PWV3 => Self::WaveformDetail(decode(body)?),
            FourCc::PWV4 => Self::WaveformColorPreview(decode(body)?),
            FourCc::PWV5 => Self::WaveformColorDetail(decode(body)?),
            FourCc::PCOB => Self::CueList(decode(body)?),
            FourCc::PCO2 => Self::ExtendedCueList(decode(body)?),
            FourCc::PSSI => Self::SongStructure(SongStructure::parse(body)?),
            _ => return None,
        })
    }
}

// -- PPTH ------------------------------------------------------------------

/// `PPTH` — where the audio file lives, in the player's namespace.
///
/// UTF-16 **big**-endian with a trailing NUL, and the declared length counts
/// that NUL. Note the endianness: the same path travels UTF-16 *little*-endian
/// over NFS, and the two must never share a helper.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct AudioPath {
    /// The path.
    pub path: String,
}

impl AudioPath {
    /// The path.
    pub fn as_str(&self) -> &str {
        &self.path
    }

    fn parse(body: &[u8]) -> Option<Self> {
        let len: u32 = decode(body)?;
        let text = body.get(4..)?;
        let declared = usize::try_from(len).ok()?;
        // The declared length includes the trailing NUL; a shorter buffer than
        // it claims is a truncated file, not a path we should half-report.
        if declared > text.len() {
            return None;
        }
        Some(Self {
            path: utf16be(text.get(..declared).unwrap_or_default()),
        })
    }
}

/// Decode UTF-16BE, dropping a trailing NUL and replacing anything undecodable.
fn utf16be(raw: &[u8]) -> String {
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .filter_map(|pair| pair.try_into().ok())
        .map(u16::from_be_bytes)
        .collect();
    let mut text = String::from_utf16_lossy(&units);
    while text.ends_with('\0') {
        text.pop();
    }
    text
}

// -- PVBR ------------------------------------------------------------------

/// `PVBR` — the seek index that makes variable-bitrate playback possible.
///
/// Without a table mapping playing time to byte offset a player cannot begin
/// streaming an MP3, so it never issues a single read: the path resolves, the
/// size is right, and nothing happens. That is exactly what an unanswered
/// `0x2504` looked like (F30). A real deck answers with 1604 bytes — one
/// unknown word and 400 entries — and the size is fixed, so it matched across
/// two different media.
#[binread]
#[br(big)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VbrIndex {
    /// Unknown leading word.
    pub unknown1: u32,
    /// The index. 400 entries on every file observed, but read to the end of
    /// the tag rather than to a fixed count.
    #[br(parse_with = until_eof)]
    pub entries: Vec<u32>,
}

// -- PQTZ ------------------------------------------------------------------

/// One beat of the grid.
#[binread]
#[br(big)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Beat {
    /// Position within the bar; 1 is the downbeat.
    pub beat_number: u16,
    /// Tempo at this beat in centi-BPM.
    pub tempo: u16,
    /// When the beat occurs, in milliseconds at normal pitch.
    pub time: u32,
}

/// `PQTZ` — every beat rekordbox found.
#[binread]
#[br(big)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BeatGrid {
    /// Unknown.
    pub unknown1: u32,
    /// Unknown; reported as always `0x00080000`.
    pub unknown2: u32,
    /// The beats.
    #[br(temp)]
    len_beats: u32,
    /// The beats, in time order.
    #[br(count = usize::try_from(len_beats).unwrap_or(0))]
    pub beats: Vec<Beat>,
}

// -- waveforms -------------------------------------------------------------

/// One column of a monochrome waveform, `PWAV` or `PWV3`.
///
/// The packing is confirmed by the wire transform: serving a preview splits
/// each byte into `(height = b & 0x1f, whiteness = b >> 5)` (F30).
#[derive(Clone, Copy, PartialEq, Eq, Debug, BinRead)]
#[br(big)]
pub struct PreviewColumn(pub u8);

impl PreviewColumn {
    /// Bar height, 0–31.
    pub fn height(self) -> u8 {
        self.0 & 0x1f
    }

    /// Shade, 0–7.
    pub fn whiteness(self) -> u8 {
        self.0 >> 5
    }
}

/// One column of the small `PWV2` preview.
///
/// Four bits of height per byte. **Which nibble is not settled here**: the
/// published schema stores the bytes without interpreting them, no `.DAT` file
/// was available to check, and nothing in the Pro DJ Link serve path needs it —
/// the whole 100-byte payload is appended to the preview reply verbatim (F30).
/// [`TinyColumn::height`] takes the low nibble, which is what the prose
/// analysis describes; the raw byte is public so a caller who knows better can
/// disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug, BinRead)]
#[br(big)]
pub struct TinyColumn(pub u8);

impl TinyColumn {
    /// Bar height, 0–15.
    pub fn height(self) -> u8 {
        self.0 & 0x0f
    }
}

/// `PWAV` — the fixed-width monochrome preview above the touch strip.
#[binread]
#[br(big)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WaveformPreview {
    #[br(temp)]
    len_data: u32,
    /// Unknown; reported as always `0x00100000`.
    pub unknown: u32,
    /// The columns.
    #[br(count = usize::try_from(len_data).unwrap_or(0))]
    pub columns: Vec<PreviewColumn>,
}

/// `PWV2` — the smaller monochrome preview, for the CDJ-900.
#[binread]
#[br(big)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TinyWaveformPreview {
    #[br(temp)]
    len_data: u32,
    /// Unknown.
    pub unknown: u32,
    /// The columns; 100 on every file reported.
    #[br(count = usize::try_from(len_data).unwrap_or(0))]
    pub columns: Vec<TinyColumn>,
}

/// `PWV3` — the monochrome waveform that scrolls as the track plays.
///
/// 150 entries per second of audio, which is two per 75-fps frame.
#[binread]
#[br(big)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WaveformDetail {
    /// Bytes per entry; 1.
    pub len_entry_bytes: u32,
    #[br(temp)]
    len_entries: u32,
    /// Unknown; reported as always `0x00960000`.
    pub unknown: u32,
    /// The columns.
    #[br(count = usize::try_from(len_entries).unwrap_or(0))]
    pub columns: Vec<PreviewColumn>,
}

/// One column of the `PWV4` colour preview: six bytes of band energy.
#[binread]
#[br(big)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ColorPreviewColumn {
    /// Unknown; contributes to the shade.
    pub unknown1: u8,
    /// Unknown; contributes to the shade.
    pub unknown2: u8,
    /// Energy in the bottom half of the frequency range.
    pub energy_bottom_half: u8,
    /// Energy in the bottom third.
    pub energy_bottom_third: u8,
    /// Energy in the middle third.
    pub energy_mid_third: u8,
    /// Energy in the top third.
    pub energy_top_third: u8,
}

/// `PWV4` — the colour preview above the touch strip on newer players.
#[binread]
#[br(big)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WaveformColorPreview {
    /// Bytes per entry; 6.
    pub len_entry_bytes: u32,
    #[br(temp)]
    len_entries: u32,
    /// Unknown.
    pub unknown: u32,
    /// The columns.
    #[br(count = usize::try_from(len_entries).unwrap_or(0))]
    pub columns: Vec<ColorPreviewColumn>,
}

/// One column of the `PWV5` colour scrolling waveform: a big-endian `u16`.
///
/// # The bit layout, and why not the published one
///
/// The interpretation in common circulation — `blue = w >> 2`, `green = w >> 5`,
/// `red = w >> 8` — does not fit the data. Measured against the `PWV3`
/// monochrome waveform of the same track, its implied magnitude correlates at
/// **0.13**; the field at bits 6–2 correlates at **0.99**. So the layout, from
/// most significant bit:
///
/// ```text
/// bits 15-13  a band, mean level 5.70, slowest-varying   -> bass
/// bits 12-10  a band, mean level 2.12, spikiest          -> treble
/// bits  9-7   a band, mean level 3.40                    -> mid
/// bits  6-2   5-bit overall height, 0.99 vs PWV3
/// bits  1-0   zero on every entry of every file checked
/// ```
///
/// The bit *positions* are measured. The band *assignment* is inferred from two
/// signals that agree — bass is both the loudest and the slowest-varying,
/// treble the quietest and the spikiest — over six tracks off a real stick, and
/// it is inference rather than measurement.
///
/// The three bands are a **hue, not three magnitudes**: they sit near
/// saturation whatever the track is doing and correlate with the height field
/// at only 0.0–0.4. A renderer that derives height from them draws the spectral
/// balance instead of the envelope.
///
/// None of this is on the Pro DJ Link wire — `PWV5` is not one of the five
/// blobs a player asks for — so getting it wrong costs a waveform's colour, not
/// a load.
#[derive(Clone, Copy, PartialEq, Eq, Debug, BinRead)]
#[br(big)]
pub struct ColorDetailColumn(pub u16);

impl ColorDetailColumn {
    /// Bits 15–13. Rendered as red.
    pub fn bass(self) -> u8 {
        u8::try_from(self.0 >> 13 & 0x7).unwrap_or(0)
    }

    /// Bits 12–10. Rendered as **blue**.
    ///
    /// The band order and the colour order are not the same order, and these
    /// three doc lines used to say they were — treble was written down as green
    /// and mid as blue, because the colours were attached in the order the
    /// fields are declared rather than the order they are drawn. A renderer
    /// that believed them swapped two thirds of the picture.
    pub fn treble(self) -> u8 {
        u8::try_from(self.0 >> 10 & 0x7).unwrap_or(0)
    }

    /// Bits 9–7. Rendered as green.
    pub fn mid(self) -> u8 {
        u8::try_from(self.0 >> 7 & 0x7).unwrap_or(0)
    }

    /// Bits 6–2. The envelope, 0–31, matching `PWV3`'s height.
    pub fn height(self) -> u8 {
        u8::try_from(self.0 >> 2 & 0x1f).unwrap_or(0)
    }
}

/// `PWV5` — the colour waveform that scrolls as the track plays.
#[binread]
#[br(big)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WaveformColorDetail {
    /// Bytes per entry; 2.
    pub len_entry_bytes: u32,
    #[br(temp)]
    len_entries: u32,
    /// Unknown.
    pub unknown: u32,
    /// The columns.
    #[br(count = usize::try_from(len_entries).unwrap_or(0))]
    pub columns: Vec<ColorDetailColumn>,
}

// -- cues ------------------------------------------------------------------

/// Whether a cue list holds memory cues or hot cues.
///
/// A file carries one `PCOB` of each, so a reader that takes the first tag gets
/// half the cues and no error.
#[derive(Clone, Copy, PartialEq, Eq, Hash, BinRead)]
#[br(big)]
pub struct CueListType(pub u32);

impl CueListType {
    /// Memory cues and loops.
    pub const MEMORY: Self = Self(0);
    /// Hot cues and hot loops.
    pub const HOT: Self = Self(1);
}

impl std::fmt::Debug for CueListType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::MEMORY => f.write_str("memory"),
            Self::HOT => f.write_str("hot"),
            Self(raw) => write!(f, "CueListType({raw})"),
        }
    }
}

/// Whether an entry is a point or a loop.
///
/// `1` is a cue point and `2` a loop, per the crate-digger schema. Note that
/// `rekordcrate` names `0` as the point value instead; nothing here has a file
/// to arbitrate with, and the schema is the older and more widely exercised of
/// the two.
#[derive(Clone, Copy, PartialEq, Eq, Hash, BinRead)]
#[br(big)]
pub struct CueType(pub u8);

impl CueType {
    /// A single point.
    pub const POINT: Self = Self(1);
    /// A loop, whose end is `loop_time`.
    pub const LOOP: Self = Self(2);
}

impl std::fmt::Debug for CueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::POINT => f.write_str("point"),
            Self::LOOP => f.write_str("loop"),
            Self(raw) => write!(f, "CueType({raw})"),
        }
    }
}

/// Whether a cue is active.
#[derive(Clone, Copy, PartialEq, Eq, Hash, BinRead)]
#[br(big)]
pub struct CueStatus(pub u32);

impl CueStatus {
    /// Not in use.
    pub const DISABLED: Self = Self(0);
    /// In use.
    pub const ENABLED: Self = Self(1);
    /// An active loop.
    pub const ACTIVE_LOOP: Self = Self(4);
}

impl std::fmt::Debug for CueStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::DISABLED => f.write_str("disabled"),
            Self::ENABLED => f.write_str("enabled"),
            Self::ACTIVE_LOOP => f.write_str("active_loop"),
            Self(raw) => write!(f, "CueStatus({raw})"),
        }
    }
}

/// Bytes of a `PCPT` entry before its trailing unknown block.
const CUE_FIXED_LEN: u32 = 40;

/// One `PCPT` entry inside a `PCOB` list.
///
/// Positions are milliseconds here. They travel over the wire as a **frame
/// index at 150 fps, truncated not rounded** — 271 ms becomes 40 — so the
/// conversion belongs to the wire layer and not to this struct (F30).
#[binread]
#[br(big, magic = b"PCPT")]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cue {
    /// Declared header length of this entry.
    pub header_len: u32,
    /// Declared total length of this entry.
    pub entry_len: u32,
    /// Hot cue number: 0 is a memory cue, 1 is A, 2 is B, and so on.
    pub hot_cue: u32,
    /// Whether the cue is active.
    pub status: CueStatus,
    /// Unknown; reported as always `0x00100000`.
    pub unknown1: u32,
    /// Sort key; `0xffff` on the first cue.
    pub order_first: u16,
    /// Sort key; `0xffff` on the last cue.
    pub order_last: u16,
    /// Point or loop.
    pub cue_type: CueType,
    /// Unknown; reported as always `0x00 0x03 0xe8`.
    pub unknown2: [u8; 3],
    /// Where the cue sits, in milliseconds at normal pitch.
    pub time: u32,
    /// Where a loop jumps back from, in milliseconds.
    pub loop_time: u32,
    /// Trailing bytes whose meaning is unknown; 16 on every entry reported,
    /// read from the declared length so a longer entry survives.
    #[br(count = usize::try_from(entry_len.saturating_sub(CUE_FIXED_LEN)).unwrap_or(0))]
    pub trailing: Vec<u8>,
}

/// `PCOB` — one list of cues and loops, either memory or hot.
#[binread]
#[br(big)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CueList {
    /// Memory or hot.
    pub list_type: CueListType,
    /// Unknown.
    pub unknown: u16,
    #[br(temp)]
    len_cues: u16,
    /// Unknown.
    pub memory_count: u32,
    /// The entries, in file order rather than time order.
    #[br(count = usize::from(len_cues))]
    pub cues: Vec<Cue>,
}

/// Bytes of a `PCP2` entry before its optional comment and colour fields.
const EXTENDED_CUE_FIXED_LEN: u32 = 40;

/// One `PCP2` entry inside a `PCO2` list.
///
/// The trailing fields are optional and gated on the entry's declared length,
/// because rekordbox grew them one release at a time: an entry longer than 43
/// bytes carries a comment, one longer than 44 past the comment carries a
/// colour index, and each colour component adds one more byte.
#[binread]
#[br(big, magic = b"PCP2")]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExtendedCue {
    /// Declared header length of this entry.
    pub header_len: u32,
    /// Declared total length of this entry.
    pub entry_len: u32,
    /// Hot cue number: 0 is a memory cue, 1 is A, 2 is B, and so on.
    pub hot_cue: u32,
    /// Point or loop.
    pub cue_type: CueType,
    /// Unknown; reported as always `0x00 0x03 0xe8`.
    pub unknown1: [u8; 3],
    /// Where the cue sits, in milliseconds at normal pitch.
    pub time: u32,
    /// Where a loop jumps back from, in milliseconds.
    pub loop_time: u32,
    /// Colour row id, for a memory cue that was given one.
    pub color_id: u8,
    /// Unknown.
    pub unknown2: [u8; 7],
    /// Loop length numerator in beats, or zero when the loop is not quantised.
    pub loop_numerator: u16,
    /// Loop length denominator in beats, or zero.
    pub loop_denominator: u16,
    #[br(temp, if(entry_len > 43, 0))]
    len_comment: u32,
    /// The DJ's comment, UTF-16BE with a trailing NUL in the file.
    #[br(count = usize::try_from(len_comment).unwrap_or(0))]
    #[br(map = |raw: Vec<u8>| utf16be(&raw))]
    pub comment: String,
    /// Index into rekordbox's hot-cue colour table.
    #[br(if(entry_len.saturating_sub(len_comment) > 44, 0))]
    pub hot_cue_color_index: u8,
    /// Red component of the colour the player lights its pad with.
    #[br(if(entry_len.saturating_sub(len_comment) > 45, 0))]
    pub hot_cue_color_red: u8,
    /// Green component.
    #[br(if(entry_len.saturating_sub(len_comment) > 46, 0))]
    pub hot_cue_color_green: u8,
    /// Blue component.
    #[br(if(entry_len.saturating_sub(len_comment) > 47, 0))]
    pub hot_cue_color_blue: u8,
    /// Anything past the fields this version declares.
    #[br(count = usize::try_from(
        entry_len
            .saturating_sub(len_comment)
            .saturating_sub(EXTENDED_CUE_FIXED_LEN + 8)
    ).unwrap_or(0))]
    pub trailing: Vec<u8>,
}

/// `PCO2` — the nexus-2 cue list, with comments and colours.
#[binread]
#[br(big)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExtendedCueList {
    /// Memory or hot.
    pub list_type: CueListType,
    #[br(temp)]
    len_cues: u16,
    /// Unknown.
    pub unknown: u16,
    /// The entries.
    #[br(count = usize::from(len_cues))]
    pub cues: Vec<ExtendedCue>,
}

// -- PSSI ------------------------------------------------------------------

/// How rekordbox classified the track as a whole, which decides how phrase
/// numbers are labelled.
#[derive(Clone, Copy, PartialEq, Eq, Hash, BinRead)]
#[br(big)]
pub struct TrackMood(pub u16);

impl TrackMood {
    /// Phrases are intro, up, down, chorus, outro, with numbered variants.
    pub const HIGH: Self = Self(1);
    /// Phrases are intro, verse 1–6, chorus, bridge, outro.
    pub const MID: Self = Self(2);
    /// Phrases are intro, verse 1–2, chorus, bridge, outro.
    pub const LOW: Self = Self(3);
}

impl std::fmt::Debug for TrackMood {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::HIGH => f.write_str("high"),
            Self::MID => f.write_str("mid"),
            Self::LOW => f.write_str("low"),
            Self(raw) => write!(f, "TrackMood({raw})"),
        }
    }
}

/// The stylistic bank a track is assigned in rekordbox Lighting mode.
#[derive(Clone, Copy, PartialEq, Eq, Hash, BinRead)]
#[br(big)]
pub struct TrackBank(pub u8);

impl TrackBank {
    /// Unset, treated as `COOL`.
    pub const DEFAULT: Self = Self(0);
    /// "Cool".
    pub const COOL: Self = Self(1);
    /// "Natural".
    pub const NATURAL: Self = Self(2);
    /// "Hot".
    pub const HOT: Self = Self(3);
    /// "Subtle".
    pub const SUBTLE: Self = Self(4);
    /// "Warm".
    pub const WARM: Self = Self(5);
    /// "Vivid".
    pub const VIVID: Self = Self(6);
    /// "Club 1".
    pub const CLUB1: Self = Self(7);
    /// "Club 2".
    pub const CLUB2: Self = Self(8);

    /// The name rekordbox shows, or `None` for a value never observed.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::DEFAULT => "default",
            Self::COOL => "cool",
            Self::NATURAL => "natural",
            Self::HOT => "hot",
            Self::SUBTLE => "subtle",
            Self::WARM => "warm",
            Self::VIVID => "vivid",
            Self::CLUB1 => "club 1",
            Self::CLUB2 => "club 2",
            _ => return None,
        })
    }
}

impl std::fmt::Debug for TrackBank {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "TrackBank({})", self.0),
        }
    }
}

/// One phrase of the song structure.
///
/// The three `k` flags and `b` select among the numbered variants a high-mood
/// track uses ("Up 1", "Up 2", "Up 3"); the published analysis has the table.
#[binread]
#[br(big)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Phrase {
    /// Phrase number, from 1.
    pub index: u16,
    /// Beat at which the phrase starts.
    pub beat: u16,
    /// Which phrase this is. The numbering depends on the track's mood; see
    /// [`Phrase::label`].
    pub kind: u16,
    /// Unknown.
    pub unknown1: u8,
    /// Variant flag, high mood only.
    pub k1: u8,
    /// Unknown.
    pub unknown2: u8,
    /// Variant flag, high mood only.
    pub k2: u8,
    /// Unknown.
    pub unknown3: u8,
    /// How many of `beat2`–`beat4` are in use.
    pub b: u8,
    /// Extra beat number within the phrase.
    pub beat2: u16,
    /// Extra beat number, used when `b` is 1.
    pub beat3: u16,
    /// Extra beat number, used when `b` is 1.
    pub beat4: u16,
    /// Unknown.
    pub unknown4: u8,
    /// Variant flag, high mood only.
    pub k3: u8,
    /// Unknown.
    pub unknown5: u8,
    /// Non-zero when the phrase ends with fill-in beats.
    pub fill: u8,
    /// Beat at which the fill-in starts.
    pub beat_fill: u16,
}

impl Phrase {
    /// The name rekordbox shows for this phrase under `mood`.
    ///
    /// From the published analysis rather than from a file read here.
    pub fn label(&self, mood: TrackMood) -> Option<&'static str> {
        // One table per mood, because the numbering is per mood: kind 2 is
        // "up" in a high-mood track and "verse 1" in a low-mood one.
        match mood {
            TrackMood::HIGH => Some(match self.kind {
                1 => "intro",
                2 => "up",
                3 => "down",
                5 => "chorus",
                6 => "outro",
                _ => return None,
            }),
            TrackMood::MID => Some(match self.kind {
                1 => "intro",
                2..=7 => "verse",
                8 => "bridge",
                9 => "chorus",
                10 => "outro",
                _ => return None,
            }),
            TrackMood::LOW => Some(match self.kind {
                1 => "intro",
                2..=4 => "verse 1",
                5..=7 => "verse 2",
                8 => "bridge",
                9 => "chorus",
                10 => "outro",
                _ => return None,
            }),
            _ => None,
        }
    }
}

/// The obfuscation key rekordbox 6 masks `PSSI` with.
///
/// Each byte has the phrase count added to it before use, so the key depends on
/// the tag it protects.
const PSSI_KEY: [u8; 19] = [
    0xcb, 0xe1, 0xee, 0xfa, 0xe5, 0xee, 0xad, 0xee, 0xe9, 0xd2, 0xe9, 0xeb, 0xe1, 0xe9, 0xf3, 0xe8,
    0xe9, 0xf4, 0xe1,
];

/// Largest plausible unmasked mood value; anything above means the tag is
/// masked.
const PSSI_MOOD_CEILING: u16 = 20;

#[binread]
#[br(big, import(phrase_count: u16))]
#[derive(Clone, PartialEq, Eq, Debug)]
struct SongStructureBody {
    mood: TrackMood,
    unknown1: [u8; 6],
    end_beat: u16,
    unknown2: [u8; 2],
    bank: TrackBank,
    unknown3: u8,
    #[br(count = usize::from(phrase_count))]
    phrases: Vec<Phrase>,
}

/// `PSSI` — intro, verse, bridge, chorus, outro.
///
/// rekordbox 6 masks everything after the phrase count with a repeating XOR
/// key, which is obfuscation rather than encryption. Whether a given file is
/// masked is decided the way the published schema decides it: the mood field is
/// supposed to be 1, 2 or 3, so a value above 20 means the bytes are masked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SongStructure {
    /// Bytes per phrase entry; 24.
    pub len_entry_bytes: u32,
    /// Whether the file was masked and had to be unmasked to read.
    pub was_masked: bool,
    /// The mood, which decides how [`Phrase::kind`] is labelled.
    pub mood: TrackMood,
    /// Unknown.
    pub unknown1: [u8; 6],
    /// Beat at which the last recognised phrase ends. The track may continue.
    pub end_beat: u16,
    /// Unknown.
    pub unknown2: [u8; 2],
    /// The Lighting-mode bank.
    pub bank: TrackBank,
    /// Unknown.
    pub unknown3: u8,
    /// The phrases, in order.
    pub phrases: Vec<Phrase>,
}

impl SongStructure {
    fn parse(body: &[u8]) -> Option<Self> {
        let len_entry_bytes = u32::from_be_bytes(body.get(..4)?.try_into().ok()?);
        let phrase_count = u16::from_be_bytes(body.get(4..6)?.try_into().ok()?);
        let rest = body.get(6..)?;

        let raw_mood = u16::from_be_bytes(rest.get(..2)?.try_into().ok()?);
        let was_masked = raw_mood > PSSI_MOOD_CEILING;
        let plain = if was_masked {
            unmask(rest, phrase_count)
        } else {
            rest.to_vec()
        };

        let decoded =
            SongStructureBody::read_be_args(&mut Cursor::new(&plain), (phrase_count,)).ok()?;
        Some(Self {
            len_entry_bytes,
            was_masked,
            mood: decoded.mood,
            unknown1: decoded.unknown1,
            end_beat: decoded.end_beat,
            unknown2: decoded.unknown2,
            bank: decoded.bank,
            unknown3: decoded.unknown3,
            phrases: decoded.phrases,
        })
    }
}

fn unmask(data: &[u8], phrase_count: u16) -> Vec<u8> {
    let offset = u8::try_from(phrase_count % 256).unwrap_or(0);
    data.iter()
        .enumerate()
        .map(|(index, byte)| {
            let key = PSSI_KEY
                .get(index % PSSI_KEY.len())
                .copied()
                .unwrap_or(0)
                .wrapping_add(offset);
            byte ^ key
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_column_splits_into_height_and_whiteness() {
        // The split the wire transform uses (F30).
        let column = PreviewColumn(0b101_10001);
        assert_eq!(column.height(), 0b10001);
        assert_eq!(column.whiteness(), 0b101);
    }

    #[test]
    fn a_colour_detail_column_reads_its_height_from_bits_6_to_2() {
        // bass 5, treble 2, mid 3, height 27, low bits zero.
        let raw = 5 << 13 | 2 << 10 | 3 << 7 | 27 << 2;
        let column = ColorDetailColumn(raw);
        assert_eq!(column.bass(), 5);
        assert_eq!(column.treble(), 2);
        assert_eq!(column.mid(), 3);
        assert_eq!(
            column.height(),
            27,
            "the field that correlates 0.99 with PWV3"
        );
    }

    #[test]
    fn a_beat_grid_decodes_its_entries() {
        let mut body = vec![0u8; 8];
        body.extend_from_slice(&2u32.to_be_bytes());
        body.extend_from_slice(&[0, 1, 0x32, 0x00, 0, 0, 0, 0]);
        body.extend_from_slice(&[0, 2, 0x32, 0x00, 0, 0, 0x01, 0xd5]);
        let Some(Content::BeatGrid(grid)) = Content::parse(FourCc::PQTZ, &body) else {
            panic!("expected a beat grid");
        };
        assert_eq!(grid.beats.len(), 2);
        assert_eq!(grid.beats.first().unwrap().beat_number, 1);
        assert_eq!(grid.beats.first().unwrap().tempo, 12800);
        assert_eq!(grid.beats.get(1).unwrap().time, 469);
    }

    #[test]
    fn a_beat_count_that_cannot_be_satisfied_yields_no_content() {
        let mut body = vec![0u8; 8];
        body.extend_from_slice(&99u32.to_be_bytes());
        assert!(Content::parse(FourCc::PQTZ, &body).is_none());
    }

    #[test]
    fn a_path_is_utf16_big_endian_with_its_nul_inside_the_length() {
        let mut body = Vec::new();
        let text: Vec<u8> = "/a✧"
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_be_bytes)
            .collect();
        body.extend_from_slice(&u32::try_from(text.len()).unwrap().to_be_bytes());
        body.extend_from_slice(&text);
        let Some(Content::Path(path)) = Content::parse(FourCc::PPTH, &body) else {
            panic!("expected a path");
        };
        assert_eq!(
            path.as_str(),
            "/a✧",
            "big-endian here, little-endian on NFS"
        );
    }

    #[test]
    fn a_path_longer_than_its_buffer_yields_no_content() {
        let mut body = 1000u32.to_be_bytes().to_vec();
        body.extend_from_slice(b"\0/\0a");
        assert!(Content::parse(FourCc::PPTH, &body).is_none());
    }

    /// One `PCPT` entry with the trailing block the schema describes.
    fn cue_entry(hot_cue: u32, time: u32, loop_time: u32, cue_type: u8) -> Vec<u8> {
        let mut out = b"PCPT".to_vec();
        out.extend_from_slice(&12u32.to_be_bytes());
        out.extend_from_slice(&56u32.to_be_bytes());
        out.extend_from_slice(&hot_cue.to_be_bytes());
        out.extend_from_slice(&1u32.to_be_bytes()); // status
        out.extend_from_slice(&0x0010_0000u32.to_be_bytes());
        out.extend_from_slice(&0xffffu16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.push(cue_type);
        out.extend_from_slice(&[0x00, 0x03, 0xe8]);
        out.extend_from_slice(&time.to_be_bytes());
        out.extend_from_slice(&loop_time.to_be_bytes());
        out.extend_from_slice(&[0u8; 16]);
        out
    }

    #[test]
    fn a_cue_list_decodes_points_and_loops() {
        let mut body = CueListType::HOT.0.to_be_bytes().to_vec();
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&2u32.to_be_bytes());
        body.extend_from_slice(&cue_entry(1, 271, 0, CueType::POINT.0));
        body.extend_from_slice(&cue_entry(2, 1000, 2000, CueType::LOOP.0));
        let Some(Content::CueList(list)) = Content::parse(FourCc::PCOB, &body) else {
            panic!("expected a cue list");
        };
        assert_eq!(list.list_type, CueListType::HOT);
        assert_eq!(list.cues.len(), 2);
        let first = list.cues.first().unwrap();
        assert_eq!(first.cue_type, CueType::POINT);
        assert_eq!(
            first.time, 271,
            "milliseconds here, 150-fps frames on the wire"
        );
        assert_eq!(first.trailing.len(), 16);
        let second = list.cues.get(1).unwrap();
        assert_eq!(second.cue_type, CueType::LOOP);
        assert_eq!(second.loop_time, 2000);
    }

    /// A song-structure body, optionally masked the way rekordbox 6 masks it.
    fn song_structure_body(phrases: usize, masked: bool) -> Vec<u8> {
        let count = u16::try_from(phrases).unwrap();
        let mut plain = TrackMood::MID.0.to_be_bytes().to_vec();
        plain.extend_from_slice(&[0u8; 6]);
        plain.extend_from_slice(&512u16.to_be_bytes());
        plain.extend_from_slice(&[0u8; 2]);
        plain.push(TrackBank::VIVID.0);
        plain.push(0);
        for index in 0..phrases {
            let mut phrase = vec![0u8; 24];
            let number = u16::try_from(index + 1).unwrap();
            phrase.splice(0..2, number.to_be_bytes());
            phrase.splice(2..4, (number * 16).to_be_bytes());
            phrase.splice(4..6, 9u16.to_be_bytes()); // chorus, in mid mood
            plain.extend_from_slice(&phrase);
        }
        let payload = if masked { unmask(&plain, count) } else { plain };
        let mut body = 24u32.to_be_bytes().to_vec();
        body.extend_from_slice(&count.to_be_bytes());
        body.extend_from_slice(&payload);
        body
    }

    #[test]
    fn an_unmasked_song_structure_decodes() {
        let Some(Content::SongStructure(structure)) =
            Content::parse(FourCc::PSSI, &song_structure_body(3, false))
        else {
            panic!("expected a song structure");
        };
        assert!(!structure.was_masked);
        assert_eq!(structure.mood, TrackMood::MID);
        assert_eq!(structure.bank, TrackBank::VIVID);
        assert_eq!(structure.phrases.len(), 3);
        assert_eq!(
            structure.phrases.first().unwrap().label(structure.mood),
            Some("chorus")
        );
    }

    #[test]
    fn a_masked_song_structure_is_unmasked_before_decoding() {
        let masked = song_structure_body(3, true);
        // The masked bytes must not read as a valid mood, or the heuristic is
        // not being exercised.
        assert!(u16::from_be_bytes([masked[6], masked[7]]) > PSSI_MOOD_CEILING);
        let Some(Content::SongStructure(structure)) = Content::parse(FourCc::PSSI, &masked) else {
            panic!("expected a song structure");
        };
        assert!(structure.was_masked);
        assert_eq!(structure.mood, TrackMood::MID);
        assert_eq!(structure.phrases.len(), 3);
        assert_eq!(structure.end_beat, 512);
    }

    #[test]
    fn the_mask_depends_on_the_phrase_count() {
        // The key byte is the constant plus the phrase count, so two tags with
        // different counts mask the same plaintext differently.
        assert_ne!(unmask(&[0, 0], 1), unmask(&[0, 0], 2));
    }

    /// One `PCP2` entry. `entry_len` gates the trailing fields, so the caller
    /// chooses how much of the format's history this entry comes from.
    fn extended_cue_entry(hot_cue: u32, comment: &str, with_color: bool) -> Vec<u8> {
        let comment_bytes: Vec<u8> = comment
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_be_bytes)
            .collect();
        let len_comment = u32::try_from(comment_bytes.len()).unwrap();
        // 40 fixed + 4 length + comment, plus four colour bytes if present.
        let entry_len = 44 + len_comment + if with_color { 4 } else { 0 };

        let mut out = b"PCP2".to_vec();
        out.extend_from_slice(&12u32.to_be_bytes());
        out.extend_from_slice(&entry_len.to_be_bytes());
        out.extend_from_slice(&hot_cue.to_be_bytes());
        out.push(CueType::LOOP.0);
        out.extend_from_slice(&[0x00, 0x03, 0xe8]);
        out.extend_from_slice(&1000u32.to_be_bytes()); // time
        out.extend_from_slice(&3000u32.to_be_bytes()); // loop_time
        out.push(4); // color_id
        out.extend_from_slice(&[0u8; 7]);
        out.extend_from_slice(&4u16.to_be_bytes()); // loop numerator
        out.extend_from_slice(&1u16.to_be_bytes()); // loop denominator
        out.extend_from_slice(&len_comment.to_be_bytes());
        out.extend_from_slice(&comment_bytes);
        if with_color {
            out.extend_from_slice(&[0x22, 0xff, 0xa0, 0x00]);
        }
        out
    }

    #[test]
    fn an_extended_cue_reads_its_comment_and_colour() {
        let mut body = CueListType::HOT.0.to_be_bytes().to_vec();
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&extended_cue_entry(3, "drop ✧", true));
        let Some(Content::ExtendedCueList(list)) = Content::parse(FourCc::PCO2, &body) else {
            panic!("expected an extended cue list");
        };
        let cue = list.cues.first().unwrap();
        assert_eq!(cue.hot_cue, 3);
        assert_eq!(cue.cue_type, CueType::LOOP);
        assert_eq!(cue.time, 1000);
        assert_eq!(cue.loop_time, 3000);
        assert_eq!(cue.color_id, 4);
        assert_eq!((cue.loop_numerator, cue.loop_denominator), (4, 1));
        assert_eq!(cue.comment, "drop ✧", "UTF-16BE with a trailing NUL");
        assert_eq!(cue.hot_cue_color_index, 0x22);
        assert_eq!(
            (
                cue.hot_cue_color_red,
                cue.hot_cue_color_green,
                cue.hot_cue_color_blue
            ),
            (0xff, 0xa0, 0x00)
        );
        assert!(cue.trailing.is_empty());
    }

    #[test]
    fn an_extended_cue_from_an_older_rekordbox_has_no_colour_fields() {
        // The trailing fields arrived one release at a time and are gated on
        // the entry's own declared length, so an older entry must not have four
        // bytes of the next entry read into it as a colour.
        let mut body = CueListType::MEMORY.0.to_be_bytes().to_vec();
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&extended_cue_entry(0, "", false));
        body.extend_from_slice(&extended_cue_entry(1, "", false));
        let Some(Content::ExtendedCueList(list)) = Content::parse(FourCc::PCO2, &body) else {
            panic!("expected an extended cue list");
        };
        assert_eq!(
            list.cues.len(),
            2,
            "the second entry must start in the right place"
        );
        assert_eq!(list.cues[0].hot_cue, 0);
        assert_eq!(list.cues[1].hot_cue, 1);
        assert_eq!(list.cues[0].hot_cue_color_index, 0);
        assert_eq!(list.cues[0].comment, "");
    }

    /// A waveform tag body: entry size, count, an unknown word, then the data.
    fn sized_body(entry_bytes: u32, entries: &[u8], entry_len: usize) -> Vec<u8> {
        let mut body = entry_bytes.to_be_bytes().to_vec();
        let count = u32::try_from(entries.len() / entry_len).unwrap();
        body.extend_from_slice(&count.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(entries);
        body
    }

    #[test]
    fn the_waveform_tags_decode_to_their_column_counts() {
        // PWAV and PWV2 declare a byte count; PWV3, PWV4 and PWV5 declare an
        // entry size and a count, and the entry sizes differ (1, 6, 2).
        let mut preview = 4u32.to_be_bytes().to_vec();
        preview.extend_from_slice(&0u32.to_be_bytes());
        preview.extend_from_slice(&[0b101_10001, 0, 0, 0]);
        let Some(Content::WaveformPreview(wave)) = Content::parse(FourCc::PWAV, &preview) else {
            panic!("expected a waveform preview");
        };
        assert_eq!(wave.columns.len(), 4);
        assert_eq!(wave.columns[0].height(), 0b10001);

        let Some(Content::TinyWaveformPreview(tiny)) = Content::parse(FourCc::PWV2, &preview)
        else {
            panic!("expected a tiny preview");
        };
        assert_eq!(tiny.columns.len(), 4);

        let detail = sized_body(1, &[7; 6], 1);
        let Some(Content::WaveformDetail(wave)) = Content::parse(FourCc::PWV3, &detail) else {
            panic!("expected a waveform detail");
        };
        assert_eq!(wave.len_entry_bytes, 1);
        assert_eq!(wave.columns.len(), 6);

        let color_preview = sized_body(6, &[1; 12], 6);
        let Some(Content::WaveformColorPreview(wave)) =
            Content::parse(FourCc::PWV4, &color_preview)
        else {
            panic!("expected a colour preview");
        };
        assert_eq!(wave.columns.len(), 2);
        assert_eq!(wave.columns[0].energy_top_third, 1);

        let color_detail = sized_body(2, &[0xa1, 0x6c, 0x00, 0x00], 2);
        let Some(Content::WaveformColorDetail(wave)) = Content::parse(FourCc::PWV5, &color_detail)
        else {
            panic!("expected a colour detail");
        };
        assert_eq!(wave.columns.len(), 2);
        assert_eq!(wave.columns[0].0, 0xa16c, "a big-endian u16");
    }

    #[test]
    fn a_vbr_index_reads_to_the_end_of_its_tag() {
        let mut body = 0u32.to_be_bytes().to_vec();
        for index in 0..400u32 {
            body.extend_from_slice(&index.to_be_bytes());
        }
        assert_eq!(body.len(), 1604, "the size a real deck answers with (F30)");
        let Some(Content::VbrIndex(index)) = Content::parse(FourCc::PVBR, &body) else {
            panic!("expected a VBR index");
        };
        assert_eq!(index.entries.len(), 400);
        assert_eq!(index.entries.get(399), Some(&399));
    }
}
