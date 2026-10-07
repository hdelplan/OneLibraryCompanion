// SPDX-License-Identifier: GPL-3.0-only

//! Turning ANLZ tag payloads into the blobs dbserver puts on the wire.
//!
//! **A server cannot hand a player the bytes rekordbox wrote.** That was
//! assumed once and it is false: a real CDJ serving another CDJ *transforms*
//! every analysis blob, and the transformations are not cosmetic — the file is
//! big-endian and the wire little-endian, and three of the five change layout
//! as well (F30).
//!
//! Every rule here was derived by diffing a real load — deck A loading track
//! `0xc8` from deck B — against that track's own `ANLZ0000.DAT`/`.EXT` on the
//! medium that was in deck B. Having both halves is what makes these confirmed
//! rather than guessed.
//!
//! | Request | Reply | Transform |
//! |---|---|---|
//! | `0x2504` VBR index | `0x4502` | every 32-bit word byte-swapped |
//! | `GET_BEAT_GRID` | `0x4602` | 20-byte prefix, then 16-byte entries from the file's 8-byte ones |
//! | `GET_WAVEFORM_PREVIEW` | `0x4402` | each packed byte split in two, then the tiny waveform appended |
//! | `GET_WAVEFORM_DETAIL` | `0x4a02` | 20-byte prefix, then the payload verbatim |
//! | `GET_CUE_POINTS` | `0x4702` | two blobs, sorted by time |
//!
//! # Why this module takes bytes rather than a parsed file
//!
//! It would be natural for these functions to accept a parsed `AnlzFile`. They
//! do not, and the reason is layering: `prolink-proto` is the wire layer and
//! must not depend on the file layer. The caller — which has already parsed the
//! analysis file for its own reasons — passes the **raw payload of the tag**,
//! meaning the bytes after that tag's own header. That keeps these transforms
//! testable from a byte literal, which is exactly how the captured evidence is
//! written down.

use std::num::NonZeroU32;

/// The fifth prefix word of the beat grid and the detail waveform.
///
/// **We cannot derive it.** The two observed values, `0x06114a48` and
/// `0x0612e0b4`, are for the *same track in the same load*, so it is not a
/// property of the content — it is per reply. They are 2.58 s apart and differ
/// by 104,044, roughly 40,000 per second, which makes it a free-running counter
/// or an allocator address on the serving deck. Either way a client cannot
/// recompute it.
///
/// The reasoning that predicted it could therefore be zero was wrong, and
/// hardware settled it: **with zero the main waveform does not draw** (F33). A
/// receiver does not have to *validate* a field to *reject* it — zero is a
/// perfectly good sentinel for "absent", and evidently that is how it reads.
///
/// So the type is non-zero by construction. What the number means is still
/// unknown; all that is known is that it must be non-zero and must not go
/// backwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrefixWord(NonZeroU32);

impl PrefixWord {
    /// The high byte both observed values share; a plausible base for a counter
    /// of the same shape.
    pub const OBSERVED_BASE: u32 = 0x0600_0000;

    /// How fast the observed values advanced, in units per second.
    pub const OBSERVED_RATE: u32 = 40_000;

    /// Build a prefix word, rejecting zero.
    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// A counter of the same shape as the hardware's, for `elapsed` since the
    /// server started.
    ///
    /// Monotonic and non-zero for any elapsed time, including zero.
    pub fn from_elapsed(elapsed: std::time::Duration) -> Self {
        let ticks = elapsed
            .as_millis()
            .saturating_mul(u128::from(Self::OBSERVED_RATE))
            / 1000;
        let advanced = Self::OBSERVED_BASE.wrapping_add(u32::try_from(ticks).unwrap_or(u32::MAX));
        Self::new(advanced).unwrap_or(Self(NonZeroU32::MIN))
    }

    /// The value as it goes on the wire.
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// `0x2504` → `0x4502`: the MP3 variable-bitrate seek index.
///
/// The `PVBR` payload with every 32-bit word byte-swapped. Nothing else changes.
///
/// **This is the request that gates playback.** Without a table mapping playing
/// time to byte offset a player cannot seek within a VBR MP3, so it has no way
/// to begin streaming — a load that resolves the path perfectly and then never
/// issues a single `READ`.
///
/// In the reference capture only the final word is visibly reordered, because
/// every other word in that track's index happens to be zero, and zeros are the
/// same in either byte order. Swapping all of them is what the one
/// non-palindromic word tells us to do.
pub fn vbr_index(pvbr_payload: &[u8]) -> Vec<u8> {
    swap_u32_words(pvbr_payload)
}

/// Bytes the file uses per beat-grid entry.
const FILE_BEAT_LEN: usize = 8;
/// Bytes the wire uses per beat-grid entry.
const WIRE_BEAT_LEN: usize = 16;

/// `GET_BEAT_GRID` → `0x4602`: the beat grid.
///
/// A 20-byte little-endian prefix followed by one 16-byte entry per beat. The
/// file stores 8-byte entries — beat number `u16`, tempo `u16`, time `u32`, big
/// endian — and the wire keeps the same three fields little-endian, then pads
/// each entry to 16 with eight `0xff` bytes. Verified against all 1038 entries
/// of the captured grid.
///
/// `pqtz_payload` is the `PQTZ` tag's payload: the entries alone, with the
/// tag's own header already stripped.
pub fn beat_grid(pqtz_payload: &[u8], prefix: PrefixWord) -> Vec<u8> {
    let count = pqtz_payload.len() / FILE_BEAT_LEN;
    let mut entries = Vec::with_capacity(count * WIRE_BEAT_LEN);
    for entry in pqtz_payload.chunks_exact(FILE_BEAT_LEN) {
        // A slice pattern rather than indexing: `chunks_exact` guarantees the
        // width, and this way the compiler knows it too.
        let [beat_hi, beat_lo, tempo_hi, tempo_lo, time @ ..] = entry else {
            continue;
        };
        let beat = u16::from_be_bytes([*beat_hi, *beat_lo]);
        let tempo = u16::from_be_bytes([*tempo_hi, *tempo_lo]);
        let time = u32::from_be_bytes(time.try_into().unwrap_or_default());
        entries.extend_from_slice(&beat.to_le_bytes());
        entries.extend_from_slice(&tempo.to_le_bytes());
        entries.extend_from_slice(&time.to_le_bytes());
        // Eight bytes of 0xff. Not padding we chose: a real deck sends these.
        entries.extend_from_slice(&[0xff; 8]);
    }

    // Word 0 is the tag's own constant; word 2 is the entry-block length.
    let mut out = Vec::with_capacity(20 + entries.len());
    for word in [
        0x0008_0000,
        u32::try_from(count).unwrap_or(u32::MAX),
        u32::try_from(entries.len()).unwrap_or(u32::MAX),
        1,
        prefix.get(),
    ] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(&entries);
    out
}

/// `GET_WAVEFORM_PREVIEW` → `0x4402`: the preview waveform, plus the tiny one.
///
/// The file packs each of the 400 columns into one byte: the low five bits are
/// the bar height, the top three a "whiteness" used for shading. The wire
/// unpacks that into two bytes per column, height first — 800 bytes — and then
/// appends the 100-byte `PWV2` tiny waveform verbatim, for **900 in all**.
///
/// That trailing 100 bytes is why a plausible-looking "widen each byte"
/// implementation still comes out the wrong length.
pub fn waveform_preview(pwav_payload: &[u8], pwv2_payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pwav_payload.len() * 2 + pwv2_payload.len());
    for &packed in pwav_payload {
        out.push(packed & 0x1f);
        out.push(packed >> 5);
    }
    out.extend_from_slice(pwv2_payload);
    out
}

/// `GET_WAVEFORM_DETAIL` → `0x4a02`: the scrolling waveform.
///
/// A 20-byte little-endian prefix and then the `PWV3` payload **verbatim** — the
/// one analysis blob the wire does not reorder, because its entries are single
/// bytes and so have no byte order to get wrong.
///
/// `entry_width` is the tag's own first header word, always 1 in every file
/// seen; `0x96` is the high half of the constant that follows it in the tag
/// header. The prefix repeats the entry count either side of the width.
pub fn waveform_detail(pwv3_payload: &[u8], entry_width: u32, prefix: PrefixWord) -> Vec<u8> {
    let len = u32::try_from(pwv3_payload.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(20 + pwv3_payload.len());
    for word in [len, entry_width.max(1), len, 0x96, prefix.get()] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(pwv3_payload);
    out
}

/// Waveform frames per second.
///
/// The detail waveform is drawn at 150 columns per second — the same 150 that
/// appears in its own reply prefix — and a cue's position travels as a *frame
/// index* rather than a time, so the player can place the marker on the
/// waveform without doing the arithmetic itself.
pub const WAVEFORM_FPS: u32 = 150;

/// Bytes per record in the first `CUE_POINTS` blob.
pub const CUE_ENTRY_LEN: usize = 0x24;

/// One memory point or hot cue, as the wire needs it.
///
/// Times are milliseconds, as the file stores them; the conversion to frames
/// happens here so no caller has to remember that it truncates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cue {
    /// The cue's ordering field, as the file records it.
    pub order: u16,
    /// `0` for a memory point, `1` for hot cue A, `2` for B, and so on.
    pub hot_cue: u16,
    /// Position in milliseconds.
    pub time_ms: u32,
    /// Where an active loop jumps back to, in milliseconds; `0` when not a loop.
    pub loop_time_ms: u32,
}

impl Cue {
    /// The cue's position as a waveform frame index.
    ///
    /// **Truncated, not rounded**: 271 ms becomes 40, not 41 — confirmed
    /// against all three cues in the reference capture, which is what rules out
    /// rounding.
    pub const fn frame(self) -> u32 {
        self.time_ms.saturating_mul(WAVEFORM_FPS) / 1000
    }
}

/// The two blobs a `GET_CUE_POINTS` reply carries.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CueBlobs {
    /// `count` fixed-size records of [`CUE_ENTRY_LEN`] bytes.
    pub records: Vec<u8>,
    /// How many cues the records describe.
    pub count: u32,
    /// One little-endian `(time, loop_time)` pair per cue.
    pub times: Vec<u8>,
}

/// `GET_CUE_POINTS` → `0x4702`: memory points and hot cues.
///
/// A record is `[u16 order][u16 hot cue][u32 0][u32 0][u32 frame]` followed by
/// twenty zero bytes.
///
/// Cues go out **sorted by time**, not in the order the file stores them —
/// rekordbox had written the reference track's three cues newest-first.
pub fn cue_points(cues: &[Cue]) -> CueBlobs {
    let mut sorted = cues.to_vec();
    sorted.sort_by_key(|cue| cue.time_ms);

    let mut records = Vec::with_capacity(sorted.len() * CUE_ENTRY_LEN);
    let mut times = Vec::with_capacity(sorted.len() * 8);
    for cue in &sorted {
        records.extend_from_slice(&cue.order.to_le_bytes());
        records.extend_from_slice(&cue.hot_cue.to_le_bytes());
        records.extend_from_slice(&0u32.to_le_bytes());
        records.extend_from_slice(&0u32.to_le_bytes());
        records.extend_from_slice(&cue.frame().to_le_bytes());
        records.resize(records.len() + (CUE_ENTRY_LEN - 16), 0);

        times.extend_from_slice(&cue.time_ms.to_le_bytes());
        times.extend_from_slice(&cue.loop_time_ms.to_le_bytes());
    }
    CueBlobs {
        records,
        count: u32::try_from(sorted.len()).unwrap_or(u32::MAX),
        times,
    }
}

/// A tag payload is a run of big-endian words; the wire wants them
/// little-endian. Applied whole-payload where the layout is otherwise
/// unchanged. A trailing partial word is carried over untouched.
fn swap_u32_words(payload: &[u8]) -> Vec<u8> {
    let whole = payload.len() - payload.len() % 4;
    let mut out = Vec::with_capacity(payload.len());
    for word in payload.chunks_exact(4) {
        out.extend(word.iter().rev().copied());
    }
    out.extend_from_slice(payload.get(whole..).unwrap_or_default());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> PrefixWord {
        PrefixWord::new(0x0611_4a48).expect("the value a real deck sent")
    }

    #[test]
    fn a_zero_prefix_word_cannot_be_built() {
        // With zero here the main waveform does not draw (F33), so the type
        // refuses it rather than leaving it to a comment.
        assert!(PrefixWord::new(0).is_none());
        assert!(PrefixWord::new(1).is_some());
    }

    #[test]
    fn the_prefix_counter_never_starts_at_zero() {
        let at_start = PrefixWord::from_elapsed(std::time::Duration::ZERO);
        assert_eq!(at_start.get(), PrefixWord::OBSERVED_BASE);
    }

    #[test]
    fn the_prefix_counter_advances_like_the_hardware_did() {
        // The two observed values are 2.58 s apart and differ by 104,044.
        let early = PrefixWord::from_elapsed(std::time::Duration::ZERO);
        let late = PrefixWord::from_elapsed(std::time::Duration::from_millis(2580));
        assert_eq!(late.get() - early.get(), 103_200, "≈40,000 per second");
    }

    #[test]
    fn the_vbr_index_is_the_payload_with_every_word_swapped() {
        let payload = [0x00, 0x00, 0x00, 0x00, 0x12, 0x34, 0x56, 0x78];
        assert_eq!(
            vbr_index(&payload),
            vec![0, 0, 0, 0, 0x78, 0x56, 0x34, 0x12]
        );
    }

    #[test]
    fn a_zero_word_is_the_same_in_either_byte_order() {
        // Which is why the reference capture appeared to reorder only its last
        // word, and why swapping all of them is nonetheless right.
        assert_eq!(vbr_index(&[0; 16]), vec![0; 16]);
    }

    #[test]
    fn a_beat_grid_entry_grows_from_eight_bytes_to_sixteen() {
        // beat 1, tempo 13200 centi-BPM, time 500 ms — big-endian in the file.
        let payload = [0x00, 0x01, 0x33, 0x90, 0x00, 0x00, 0x01, 0xf4];
        let wire = beat_grid(&payload, prefix());

        assert_eq!(wire.len(), 20 + WIRE_BEAT_LEN);
        let entry = &wire[20..];
        assert_eq!(
            &entry[0..2],
            &1u16.to_le_bytes(),
            "beat number, little-endian"
        );
        assert_eq!(
            &entry[2..4],
            &13_200u16.to_le_bytes(),
            "tempo, little-endian"
        );
        assert_eq!(&entry[4..8], &500u32.to_le_bytes(), "time, little-endian");
        assert_eq!(
            &entry[8..16],
            &[0xff; 8],
            "padded with 0xff, not with zeros"
        );
    }

    #[test]
    fn the_beat_grid_prefix_describes_the_entries_that_follow() {
        let payload = [0u8; FILE_BEAT_LEN * 3];
        let wire = beat_grid(&payload, prefix());
        let word =
            |index: usize| u32::from_le_bytes(wire[index * 4..index * 4 + 4].try_into().unwrap());
        assert_eq!(word(0), 0x0008_0000, "the tag's own constant");
        assert_eq!(word(1), 3, "beat count");
        assert_eq!(word(2), 3 * 16, "entry-block length");
        assert_eq!(word(3), 1);
        assert_eq!(word(4), prefix().get());
    }

    #[test]
    fn the_preview_waveform_is_nine_hundred_bytes_not_eight_hundred() {
        // 400 packed columns become 800 bytes, and the 100-byte tiny waveform
        // is appended. Getting the length right is the whole test.
        let preview = [0u8; 400];
        let tiny = [0u8; 100];
        assert_eq!(waveform_preview(&preview, &tiny).len(), 900);
    }

    #[test]
    fn a_preview_column_splits_into_height_then_whiteness() {
        // 0b101_00110: whiteness 5, height 6.
        let wire = waveform_preview(&[0b1010_0110], &[]);
        assert_eq!(wire, vec![0b0000_0110, 0b0000_0101]);
    }

    #[test]
    fn the_detail_waveform_payload_is_not_reordered() {
        let payload: Vec<u8> = (0..=255u8).collect();
        let wire = waveform_detail(&payload, 1, prefix());
        assert_eq!(
            &wire[20..],
            payload.as_slice(),
            "single bytes have no byte order"
        );
        let word =
            |index: usize| u32::from_le_bytes(wire[index * 4..index * 4 + 4].try_into().unwrap());
        assert_eq!(word(0), 256);
        assert_eq!(word(1), 1, "entry width");
        assert_eq!(word(2), 256);
        assert_eq!(word(3), 0x96);
        assert_eq!(word(4), prefix().get());
    }

    #[test]
    fn a_cue_time_becomes_a_frame_index_by_truncation() {
        // 271 ms is 40.65 frames at 150 fps, and the hardware sends 40.
        assert_eq!(
            Cue {
                order: 0,
                hot_cue: 0,
                time_ms: 271,
                loop_time_ms: 0
            }
            .frame(),
            40
        );
    }

    #[test]
    fn cues_go_out_sorted_by_time_not_in_file_order() {
        // rekordbox had written the reference track's cues newest-first.
        let cues = [
            Cue {
                order: 3,
                hot_cue: 0,
                time_ms: 9000,
                loop_time_ms: 0,
            },
            Cue {
                order: 1,
                hot_cue: 1,
                time_ms: 271,
                loop_time_ms: 0,
            },
            Cue {
                order: 2,
                hot_cue: 0,
                time_ms: 4000,
                loop_time_ms: 0,
            },
        ];
        let blobs = cue_points(&cues);
        assert_eq!(blobs.count, 3);
        assert_eq!(blobs.records.len(), 3 * CUE_ENTRY_LEN);
        assert_eq!(blobs.times.len(), 3 * 8);

        let first_time = u32::from_le_bytes(blobs.times[0..4].try_into().unwrap());
        let second_time = u32::from_le_bytes(blobs.times[8..12].try_into().unwrap());
        assert_eq!((first_time, second_time), (271, 4000));

        // The first record's frame is the earliest cue's.
        let frame = u32::from_le_bytes(blobs.records[12..16].try_into().unwrap());
        assert_eq!(frame, 40);
    }

    #[test]
    fn no_cues_is_an_answer_not_an_error() {
        assert_eq!(cue_points(&[]), CueBlobs::default());
    }
}
