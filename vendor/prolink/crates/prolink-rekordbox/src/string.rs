// SPDX-License-Identifier: GPL-3.0-only

//! The variable-length string `export.pdb` stores every name in.
//!
//! Called a *DeviceSQL string* in the published analysis and a *PioString* in
//! this project's research notes; the same thing. It is a one-byte selector, an
//! optional framing header, and then text — and it is the single place in this
//! crate where getting the details wrong produces something that looks correct.
//!
//! # The four forms
//!
//! ```text
//! selector      layout
//! odd byte      short ASCII: the byte itself is 2 * (len + 1) + 1, text follows inline
//! 0x40          long ASCII:  u16 length (whole string, header included), pad byte, text
//! 0x90          UTF-16:      u16 length (likewise), pad byte, text as UTF-16 LITTLE-endian
//! 0x90 + 0x03   ISRC:        the same framing, then a 0x03 magic byte and NUL-terminated ASCII
//! ```
//!
//! The short form is discriminated by its low bit, which is why the two framed
//! selectors are even. Both framed forms have a **four-byte header** and store
//! the length of the whole string including that header.
//!
//! # The trap (O6)
//!
//! The UTF-16 form is **little-endian and starts at `offset + 4`**. The
//! pre-hardware literature says big-endian from `offset + 3`, and so did the
//! reference implementation. The raw bytes settle it:
//!
//! ```text
//! 0x2f6d4  90        selector, UTF-16
//! 0x2f6d5  20 00     stored length 32  ->  28 bytes of text
//! 0x2f6d7  00        padding byte      <-  the byte that was not being skipped
//! 0x2f6d8  27 27 42 00 52 00 ...       <-  UTF-16 LITTLE-endian, not big
//!          ✧     B     R
//! ```
//!
//! **The two errors cancel exactly for ASCII.** Reading big-endian one byte
//! early is byte-for-byte identical to reading little-endian from the right
//! offset whenever every character has a zero high byte. So encoder and decoder
//! agreed with each other perfectly, a 692-track library parsed cleanly, and
//! only non-ASCII names came out as mojibake — which on the serve side became
//! `NFSERR_NOENT` on 24 of 692 tracks, i.e. a load that fails on a path the
//! player was handed by us.
//!
//! A round-trip test cannot catch this class of bug. The tests below therefore
//! pin **literal bytes lifted from a real `export.pdb`** against the names as
//! they appear on the medium's own filesystem, which is an independent source
//! and cannot round-trip into agreement.
//!
//! # The ISRC form
//!
//! 245 of the 651 track rows in `testdata/export.pdb` carry one. rekordbox
//! stores a track's International Standard Recording Code in what claims to be
//! the UTF-16 form but is really a `0x03` magic byte followed by NUL-terminated
//! ASCII. Decoding it as UTF-16 yields two CJK characters and no error. The
//! `rekordcrate` crate calls this "a bug/flaw in Pioneer's implementation" and
//! handles it; the Python reference does not, and returns mojibake.

use std::io::{Read, Seek, SeekFrom};

use binrw::{BinRead, BinResult, BinWrite, Endian};

/// Selector byte of the long-ASCII form.
pub const SELECTOR_LONG_ASCII: u8 = 0x40;

/// Selector byte of the UTF-16 form, which the ISRC form also uses.
pub const SELECTOR_UTF16: u8 = 0x90;

/// First payload byte that marks a `0x90`-framed string as an ISRC.
pub const ISRC_MAGIC: u8 = 0x03;

/// Bytes of framing before the text in either framed form: selector, `u16`
/// length, and the padding byte the literature omits (O6).
pub const FRAMED_HEADER_LEN: u16 = 4;

/// Longest string the short form can hold, since it encodes `2 * (len + 1) + 1`
/// in one byte.
pub const MAX_SHORT_LEN: usize = 126;

/// Which of the four encodings a string was stored in.
///
/// Kept alongside the text so a string round-trips to the same bytes rekordbox
/// wrote. rekordbox picks the narrowest form that fits, so re-deriving the form
/// from the text alone is right for everything except the ISRC — which is
/// exactly the case that has to be preserved rather than guessed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StringForm {
    /// Length packed into the selector byte; text inline. The default, because
    /// it is what rekordbox writes for an empty string.
    #[default]
    ShortAscii,
    /// Selector `0x40`, four-byte header.
    LongAscii,
    /// Selector `0x90`, four-byte header, UTF-16 **little**-endian text (O6).
    Utf16Le,
    /// Selector `0x90`, four-byte header, `0x03` magic, NUL-terminated ASCII.
    Isrc,
}

/// One decoded string, with the form it was stored in.
///
/// Decoding is strict about framing and lenient about text: a length that runs
/// past the end of the buffer is an error, while an undecodable character
/// becomes `U+FFFD`. A corrupt title should cost that title, not the database.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DeviceSqlString {
    /// The text, with any trailing NUL removed.
    pub text: String,
    /// How it was stored.
    pub form: StringForm,
}

impl DeviceSqlString {
    /// The empty string, as rekordbox writes it: a bare `0x03` selector.
    pub fn empty() -> Self {
        Self {
            text: String::new(),
            form: StringForm::ShortAscii,
        }
    }

    /// Build a string in the narrowest form that fits, the way rekordbox does.
    ///
    /// ASCII short enough for the inline form uses it, longer ASCII takes the
    /// `0x40` form, and anything else goes to UTF-16LE. This is *not* how you
    /// make an ISRC; see [`DeviceSqlString::isrc`].
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let form = if text.is_ascii() {
            if text.len() <= MAX_SHORT_LEN {
                StringForm::ShortAscii
            } else {
                StringForm::LongAscii
            }
        } else {
            StringForm::Utf16Le
        };
        Self { text, form }
    }

    /// Build the mangled form rekordbox uses for an ISRC.
    pub fn isrc(text: impl Into<String>) -> Self {
        let text = text.into();
        if text.is_empty() {
            return Self::empty();
        }
        Self {
            text,
            form: StringForm::Isrc,
        }
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Encode into the bytes rekordbox would have written.
    pub fn encode(&self) -> Vec<u8> {
        match self.form {
            StringForm::ShortAscii => encode_short(&self.text),
            StringForm::LongAscii => framed(SELECTOR_LONG_ASCII, self.text.as_bytes()),
            StringForm::Utf16Le => {
                let mut body = Vec::with_capacity(self.text.len() * 2);
                for unit in self.text.encode_utf16() {
                    body.extend_from_slice(&unit.to_le_bytes());
                }
                framed(SELECTOR_UTF16, &body)
            }
            StringForm::Isrc => {
                let mut body = Vec::with_capacity(self.text.len() + 2);
                body.push(ISRC_MAGIC);
                body.extend_from_slice(self.text.as_bytes());
                body.push(0);
                framed(SELECTOR_UTF16, &body)
            }
        }
    }
}

fn encode_short(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let len = bytes.len().min(MAX_SHORT_LEN);
    // The selector packs the length as `2 * (len + 1) + 1`; the low bit is what
    // distinguishes this form from the two framed ones.
    let selector = u8::try_from((len + 1) * 2 + 1).unwrap_or(u8::MAX);
    let mut out = Vec::with_capacity(len + 1);
    out.push(selector);
    out.extend_from_slice(bytes.get(..len).unwrap_or_default());
    out
}

fn framed(selector: u8, body: &[u8]) -> Vec<u8> {
    let stored = u16::try_from(body.len())
        .unwrap_or(u16::MAX)
        .saturating_add(FRAMED_HEADER_LEN);
    let mut out = Vec::with_capacity(body.len() + usize::from(FRAMED_HEADER_LEN));
    out.push(selector);
    out.extend_from_slice(&stored.to_le_bytes());
    out.push(0); // The padding byte. See the module docs, and O6.
    out.extend_from_slice(body);
    out
}

/// The string carries its own byte order — the length is little-endian and the
/// text's encoding is decided by the selector — so it ignores whatever endian
/// the surrounding structure was read with. Declaring that lets it be read
/// directly, without a caller having to pick an endianness it does not use.
impl binrw::meta::ReadEndian for DeviceSqlString {
    const ENDIAN: binrw::meta::EndianKind = binrw::meta::EndianKind::None;
}

impl binrw::meta::WriteEndian for DeviceSqlString {
    const ENDIAN: binrw::meta::EndianKind = binrw::meta::EndianKind::None;
}

impl BinRead for DeviceSqlString {
    type Args<'a> = ();

    fn read_options<R: Read + Seek>(
        reader: &mut R,
        _endian: Endian,
        (): Self::Args<'_>,
    ) -> BinResult<Self> {
        let selector = u8::read_le(reader)?;
        if selector & 1 == 1 {
            return read_short(reader, selector);
        }
        let stored = u16::read_le(reader)?;
        let body_len = stored
            .checked_sub(FRAMED_HEADER_LEN)
            .ok_or_else(|| framing_error(reader, format!("stored length {stored} < 4")))?;
        // Skip the padding byte, which is what the literature gets wrong.
        u8::read_le(reader)?;

        let mut body = vec![0u8; usize::from(body_len)];
        reader.read_exact(&mut body)?;

        match selector {
            SELECTOR_LONG_ASCII => Ok(Self {
                text: String::from_utf8_lossy(&body).into_owned(),
                form: StringForm::LongAscii,
            }),
            SELECTOR_UTF16 => Ok(decode_utf16_or_isrc(&body)),
            other => Err(framing_error(
                reader,
                format!("unknown string selector {other:#04x}"),
            )),
        }
    }
}

impl BinWrite for DeviceSqlString {
    type Args<'a> = ();

    fn write_options<W: std::io::Write + Seek>(
        &self,
        writer: &mut W,
        _endian: Endian,
        (): Self::Args<'_>,
    ) -> BinResult<()> {
        writer.write_all(&self.encode())?;
        Ok(())
    }
}

fn read_short<R: Read + Seek>(reader: &mut R, selector: u8) -> BinResult<DeviceSqlString> {
    // `(selector - 1) / 2 - 1`. Selectors 0x01 and 0x03 both mean "empty";
    // anything smaller cannot occur because the low bit is set.
    let len = usize::from(selector >> 1).saturating_sub(1);
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(DeviceSqlString {
        text: String::from_utf8_lossy(&body).into_owned(),
        form: StringForm::ShortAscii,
    })
}

fn decode_utf16_or_isrc(body: &[u8]) -> DeviceSqlString {
    if body.first() == Some(&ISRC_MAGIC) {
        let ascii = body.get(1..).unwrap_or_default();
        let end = ascii.iter().position(|&b| b == 0).unwrap_or(ascii.len());
        return DeviceSqlString {
            text: String::from_utf8_lossy(ascii.get(..end).unwrap_or_default()).into_owned(),
            form: StringForm::Isrc,
        };
    }
    let units: Vec<u16> = body
        .chunks_exact(2)
        .filter_map(|pair| pair.try_into().ok())
        .map(u16::from_le_bytes)
        .collect();
    let mut text = String::from_utf16_lossy(&units);
    // Some strings carry a trailing NUL inside their declared length. Nothing a
    // CDJ displays should end in U+0000, so trim rather than pass it on.
    while text.ends_with('\0') {
        text.pop();
    }
    DeviceSqlString {
        text,
        form: StringForm::Utf16Le,
    }
}

fn framing_error<R: Seek>(reader: &mut R, message: String) -> binrw::Error {
    binrw::Error::AssertFail {
        pos: reader.stream_position().unwrap_or(0),
        message,
    }
}

/// Read the string at `base + offset`, leaving the reader where it was.
///
/// A relative offset of zero points at the row's own header bytes, which can
/// never be a string, so it means "this slot is unused" — decoding it would
/// yield convincing garbage rather than an error, and that surfaced as a
/// mangled `comment` field only once the data reached a browse screen. A slot
/// whose string fails to decode is likewise reported empty: one bad title must
/// not cost the row.
pub(crate) fn read_at<R: Read + Seek>(reader: &mut R, base: u64, offset: u16) -> DeviceSqlString {
    if offset == 0 {
        return DeviceSqlString::empty();
    }
    let Ok(here) = reader.stream_position() else {
        return DeviceSqlString::empty();
    };
    let parsed = reader
        .seek(SeekFrom::Start(base.saturating_add(u64::from(offset))))
        .ok()
        .and_then(|_| DeviceSqlString::read(reader).ok())
        .unwrap_or_default();
    let _ = reader.seek(SeekFrom::Start(here));
    parsed
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn decode(raw: &[u8]) -> DeviceSqlString {
        DeviceSqlString::read(&mut Cursor::new(raw)).unwrap()
    }

    /// Bytes lifted verbatim from a real `export.pdb`, with the expected text
    /// taken from that same medium's *filesystem* — an independent source,
    /// which is the whole point (O6).
    const REAL_SPARKLES: &[u8] = &[
        0x90, 0x20, 0x00, 0x00, 0x27, 0x27, 0x42, 0x00, 0x52, 0x00, 0x41, 0x00, 0x49, 0x00, 0x4e,
        0x00, 0x44, 0x00, 0x41, 0x00, 0x41, 0x00, 0x4d, 0x00, 0x41, 0x00, 0x47, 0x00, 0x45, 0x00,
        0x27, 0x27,
    ];

    /// From `testdata/export.pdb`, track 3's string slot 0.
    const REAL_ISRC: &[u8] = &[
        0x90, 0x12, 0x00, 0x00, 0x03, 0x47, 0x42, 0x55, 0x4d, 0x43, 0x32, 0x34, 0x30, 0x30, 0x30,
        0x30, 0x31, 0x00,
    ];

    #[test]
    fn a_real_utf16_string_decodes_to_the_name_on_disk() {
        // Decoded big-endian from offset+3 this reads "✧B\0R\0A\0..." mojibake,
        // and the deck is then asked for a path that does not exist.
        assert_eq!(decode(REAL_SPARKLES).text, "✧BRAINDAAMAGE✧");
        assert_eq!(decode(REAL_SPARKLES).form, StringForm::Utf16Le);
    }

    #[test]
    fn our_encoder_reproduces_a_real_pdbs_utf16_bytes() {
        assert_eq!(
            DeviceSqlString::new("✧BRAINDAAMAGE✧").encode(),
            REAL_SPARKLES,
            "what we write must match what rekordbox writes"
        );
    }

    #[test]
    fn the_padding_byte_is_skipped_not_read_as_text() {
        // The byte at offset 3 is the pad. If it were text the first character
        // would be U+2700 rather than U+2727.
        assert_eq!(REAL_SPARKLES.get(3), Some(&0x00));
        assert_eq!(decode(REAL_SPARKLES).text.chars().next(), Some('✧'));
    }

    #[test]
    fn a_real_isrc_decodes_as_ascii_not_as_two_cjk_characters() {
        let parsed = decode(REAL_ISRC);
        assert_eq!(parsed.text, "GBUMC2400001");
        assert_eq!(parsed.form, StringForm::Isrc);
        assert_eq!(
            parsed.encode(),
            REAL_ISRC,
            "the mangled form must survive a round trip"
        );
    }

    #[test]
    fn the_short_form_packs_its_length_into_the_selector() {
        let encoded = DeviceSqlString::new("abc").encode();
        assert_eq!(encoded.first(), Some(&9), "2 * (3 + 1) + 1");
        assert_eq!(encoded.get(1..), Some(b"abc".as_slice()));
        assert_eq!(decode(&encoded).text, "abc");
    }

    #[test]
    fn the_empty_string_is_a_bare_selector() {
        assert_eq!(DeviceSqlString::empty().encode(), vec![3]);
        assert_eq!(decode(&[3]).text, "");
        // 0x01 also means empty; a real medium carries both.
        assert_eq!(decode(&[1]).text, "");
    }

    #[test]
    fn long_ascii_uses_the_0x40_selector_and_a_plus_four_length() {
        let text = "y".repeat(200);
        let encoded = DeviceSqlString::new(text.clone()).encode();
        assert_eq!(encoded.first(), Some(&SELECTOR_LONG_ASCII));
        assert_eq!(encoded.get(1..3), Some([204, 0].as_slice()), "200 + 4");
        assert_eq!(encoded.get(3), Some(&0), "padding byte");
        assert_eq!(decode(&encoded).text, text);
    }

    #[test]
    fn the_short_form_is_chosen_up_to_its_ceiling_and_not_past_it() {
        assert_eq!(
            DeviceSqlString::new("x".repeat(MAX_SHORT_LEN)).form,
            StringForm::ShortAscii
        );
        assert_eq!(
            DeviceSqlString::new("x".repeat(MAX_SHORT_LEN + 1)).form,
            StringForm::LongAscii
        );
    }

    #[test]
    fn round_trips_a_range_of_texts() {
        for text in [
            "",
            "a",
            "Blue Monday",
            &"x".repeat(200),
            "Étude",
            "夜のテーマ",
            "Rene Wise & Rødhåd",
        ] {
            let original = DeviceSqlString::new(text);
            assert_eq!(decode(&original.encode()), original, "round trip {text:?}");
        }
    }

    #[test]
    fn a_length_running_past_the_buffer_is_an_error_not_a_truncation() {
        let mut raw = vec![SELECTOR_LONG_ASCII, 0xe8, 0x03, 0x00];
        raw.extend_from_slice(b"short");
        assert!(DeviceSqlString::read(&mut Cursor::new(raw)).is_err());
    }

    #[test]
    fn a_stored_length_below_the_header_is_rejected() {
        let raw = [SELECTOR_UTF16, 0x02, 0x00, 0x00];
        assert!(DeviceSqlString::read(&mut Cursor::new(raw)).is_err());
    }

    #[test]
    fn an_unknown_even_selector_is_rejected_rather_than_guessed() {
        let raw = [0x50, 0x08, 0x00, 0x00, b'a', b'b', b'c', b'd'];
        assert!(DeviceSqlString::read(&mut Cursor::new(raw)).is_err());
    }

    #[test]
    fn a_zero_offset_slot_reads_as_absent() {
        let mut cursor = Cursor::new(REAL_SPARKLES);
        assert_eq!(read_at(&mut cursor, 0, 0).text, "");
        assert_eq!(
            cursor.position(),
            0,
            "reading a slot must not move the row cursor"
        );
    }
}
