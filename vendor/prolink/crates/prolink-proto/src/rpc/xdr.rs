// SPDX-License-Identifier: GPL-3.0-only

//! XDR (RFC 4506), plus Pioneer's one deviation from it.
//!
//! Standard XDR is four rules: everything is big-endian, everything is padded
//! up to a four-byte boundary, variable-length data carries a 32-bit byte
//! count, and fixed-length data carries none.
//!
//! # The deviation, and why `libnfs` cannot simply be linked
//!
//! Standard NFS and MOUNT put path and file names on the wire as ASCII.
//! Pioneer puts the mount path and *every* `LOOKUP` filename on as **UTF-16
//! little-endian**, still length-prefixed — and the prefix counts **bytes**,
//! not characters, so an n-character ASCII name announces `2n`. `/C/` is three
//! characters and travels as a prefix of six followed by `2f 00 43 00 2f 00`,
//! confirmed directly off a CDJ-2000NXS (F12).
//!
//! This is the single most important non-standard fact about the file-access
//! path. Getting it wrong yields `NFSERR_NOENT` for files that are plainly
//! there and no other clue, and it is the reason a stock NFS library is no use
//! here: its wire encoder emits ASCII, so adopting one would mean patching it
//! rather than writing these few hundred lines.
//!
//! # Two string encodings that must never share a helper
//!
//! [`Writer::ascii_string`] and [`Writer::utf16le_string`] are deliberately
//! separate functions rather than one function with a flag, because within a
//! *single* MOUNT `EXPORT` reply the directory path is UTF-16LE and the group
//! names are plain ASCII (C7). Pioneer's convention is not applied uniformly
//! even within one structure; a decoder that assumed it was turned
//! `169.254.244.181/255.255.255.255` into CJK mojibake.
//!
//! And note the contrast with [`crate::dbserver`], which is a *third*
//! convention: UTF-16 **big**-endian, counted in **characters** including a
//! trailing NUL. Two endiannesses and two units inside one protocol. Nothing
//! is shared between that module and this one, and nothing should be.
//!
//! # A real CDJ does not zero its padding
//!
//! RFC 4506 says the padding bytes that round a field up to four "should be"
//! zero, and every implementation treats them as ignorable on receipt. Pioneer
//! takes the second half of that seriously and not the first: captured `MNT`,
//! `UMNT` and `LOOKUP` calls carry uninitialised bytes there —
//!
//! ```text
//! MNT    '/C/'                  00000006 2f0043002f00 0011
//! UMNT   '/C/'                  00000006 2f0043002f00 3cd2
//! LOOKUP '02. Akiba - カガミ.mp3'  00000026 …0070003300 8930
//! ```
//!
//! Two consequences. A decoder must **skip** padding rather than check it,
//! which is what [`Reader::opaque_fixed`] and [`Reader::opaque_var`] do.
//!
//! And byte-exactness has a seam here. A parsed [`crate::rpc::Call`] carries
//! its argument block as opaque bytes, so a captured datagram re-encodes
//! exactly, padding included. Re-encoding from a *decoded* argument
//! structure — a [`crate::rpc::mount::Request`], say — writes the zeroes the
//! standard requires and so differs from the capture in those one to three
//! bytes. Neither is wrong and no receiver can tell, but a round-trip test
//! needs to know which of the two it is asserting.
//!
//! # Hostile input
//!
//! Every length prefix is checked against the bytes actually remaining
//! **before** anything is allocated, so a corrupt or hostile datagram claiming
//! a four-gigabyte name costs a parse failure rather than four gigabytes. This
//! is a network input path reachable by anyone on the link; that property is
//! load-bearing and the tests pin it.

use std::fmt;

use crate::{Error, Result};

/// Round a length up to XDR's four-byte boundary.
///
/// Saturates rather than wrapping. The saturating case cannot arise from a
/// parsed length — every caller has already bounded it against the buffer —
/// but a silent wrap here would turn a length check into its own bypass.
pub const fn align4(length: usize) -> usize {
    match length.checked_add(3) {
        Some(sum) => sum & !3,
        None => usize::MAX,
    }
}

/// Bytes of padding that follow `length` bytes of payload.
pub const fn padding_for(length: usize) -> usize {
    align4(length) - length
}

/// Default ceiling on a length-prefixed string.
///
/// The longest real path on a rekordbox medium is a few hundred bytes; this is
/// the cap the Kaitai schema validated against 8415 real calls.
pub const MAX_STRING: u32 = 1024;

/// RFC 1057's own ceiling on an `opaque_auth` body.
pub const MAX_AUTH_BODY: u32 = 400;

/// A Pioneer UTF-16LE name, kept in the form it travels in.
///
/// The raw bytes are the field and the [`String`] is derived, never the other
/// way round. That ordering is the whole point: a name that is not well-formed
/// UTF-16 — an odd byte count, an unpaired surrogate — still re-encodes to the
/// bytes it arrived as, so a capture round-trips and a `LOOKUP` we forward is
/// the one the deck asked for. Deriving the bytes from a decoded string would
/// quietly normalise them, which is one of the three independent bugs that
/// made an earlier implementation serve paths that do not exist (O6).
///
/// The length prefix on the wire counts these bytes, so the type also makes
/// the byte-versus-character mistake unwritable.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct Utf16LeString(Vec<u8>);

impl Utf16LeString {
    /// Encode `text` as UTF-16LE.
    pub fn new(text: &str) -> Self {
        let mut bytes = Vec::with_capacity(text.len() * 2);
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        Self(bytes)
    }

    /// Adopt bytes that are already UTF-16LE, whether or not they are valid.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    /// The wire bytes, with no length prefix and no padding.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// How many bytes the length prefix will announce — `2n` for an
    /// n-character ASCII name.
    pub fn len_bytes(&self) -> usize {
        self.0.len()
    }

    /// Whether this name is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Decode to a [`String`], substituting `U+FFFD` for anything malformed.
    ///
    /// Lenient by design. A mangled name visible in a log is far more useful
    /// than a dropped datagram, and the raw bytes are still here for whoever
    /// needs to see what the hardware actually said — which for the `/B/` and
    /// `/C/` export names is the difference between evidence and our reading
    /// of it.
    pub fn to_string_lossy(&self) -> String {
        let mut units = Vec::with_capacity(self.0.len() / 2);
        let mut chunks = self.0.chunks_exact(2);
        for pair in chunks.by_ref() {
            let (Some(&low), Some(&high)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            units.push(u16::from_le_bytes([low, high]));
        }
        let mut text: String = char::decode_utf16(units)
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
        if !chunks.remainder().is_empty() {
            // An odd byte count cannot be UTF-16 at all. Mark the dangling
            // byte rather than dropping it, so the mismatch is visible.
            text.push(char::REPLACEMENT_CHARACTER);
        }
        text
    }
}

impl From<&str> for Utf16LeString {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl fmt::Display for Utf16LeString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

impl fmt::Debug for Utf16LeString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The decoded form for reading, the byte count because that is what
        // the wire prefix says and what gets miscounted.
        write!(f, "{:?}/{}B", self.to_string_lossy(), self.0.len())
    }
}

/// Big-endian XDR encoder.
///
/// Every method appends; nothing seeks. A `Vec` never runs out of room, so
/// encoding is infallible and no method returns a [`Result`] —
/// the one exception is the caller's job, which is to have proven that the
/// values it is writing are in range before it gets here.
#[derive(Clone, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// An empty encoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty encoder with room for `capacity` bytes.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    /// Bytes written so far.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Take the encoded bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// How many bytes have been written.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing has been written.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// One 32-bit big-endian word — XDR's only integer size that matters here.
    pub fn u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// A signed 32-bit word. Identical bytes to [`Writer::u32`]; separate so a
    /// field's signedness is visible at the call site.
    pub fn i32(&mut self, value: i32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// A 64-bit big-endian word. Unused by NFSv2, whose sizes and offsets are
    /// all 32-bit, and present because XDR defines it.
    pub fn u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// XDR's boolean: a full word, zero or one.
    pub fn bool(&mut self, value: bool) {
        self.u32(u32::from(value));
    }

    /// Bytes with no length prefix and no interpretation.
    pub fn raw(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Pad a field of `field_len` bytes up to the next four-byte boundary.
    ///
    /// Deliberately **not** "align the buffer". XDR's padding rule is a
    /// property of the field, not of where the field happens to have landed,
    /// and the two agree only while the buffer is already word-aligned.
    /// [`Writer::raw`] is public and can break that, and a `pad`-the-buffer
    /// implementation would then silently emit a field with the wrong number
    /// of padding bytes — the sort of thing that decodes fine against our own
    /// reader and desynchronises somebody else's.
    fn pad_field(&mut self, field_len: usize) {
        self.buf
            .extend(std::iter::repeat_n(0u8, padding_for(field_len)));
    }

    /// Fixed-length opaque: the bytes, then padding, and **no length prefix**.
    ///
    /// This is how the 32-byte NFS filehandle travels. A handle is an
    /// uninterpreted token: echo back exactly what the peer gave, never parse
    /// or normalise it. (A CDJ does not return the favour — see
    /// [`crate::rpc::FileHandle`].)
    pub fn opaque_fixed(&mut self, data: &[u8]) {
        self.raw(data);
        self.pad_field(data.len());
    }

    /// Variable-length opaque: a byte count, the bytes, then padding.
    ///
    /// The count is `usize`-to-`u32` saturating, which is unreachable rather
    /// than merely unlikely: the largest field this crate encodes is a `READ`
    /// payload, itself bounded by what a UDP datagram can carry. Saturating is
    /// not a fix — a saturated count would disagree with the bytes that follow
    /// it just as a truncated one would — it is only a way to write the
    /// conversion without a cast.
    pub fn opaque_var(&mut self, data: &[u8]) {
        self.u32(u32::try_from(data.len()).unwrap_or(u32::MAX));
        self.raw(data);
        self.pad_field(data.len());
    }

    /// A length-prefixed **ASCII** string.
    ///
    /// Standard XDR. Used for the `AUTH_UNIX` machine name and for the group
    /// names in a MOUNT `EXPORT` reply — and for nothing else in this
    /// protocol. Non-ASCII characters are replaced byte-for-byte with `?`,
    /// matching the reference implementation, because there is no correct
    /// answer and silently emitting UTF-8 would be a third encoding.
    pub fn ascii_string(&mut self, text: &str) {
        let ascii: Vec<u8> = text
            .chars()
            .map(|c| {
                if c.is_ascii() {
                    u8::try_from(c).unwrap_or(b'?')
                } else {
                    b'?'
                }
            })
            .collect();
        self.opaque_var(&ascii);
    }

    /// A length-prefixed **UTF-16LE** string, the count in bytes.
    ///
    /// Pioneer's convention: see the module documentation. Not to be confused
    /// with the dbserver layer's UTF-16 big-endian counted in characters.
    pub fn utf16le_string(&mut self, text: &Utf16LeString) {
        self.opaque_var(text.as_bytes());
    }

    /// A counted array of 32-bit words — the `AUTH_UNIX` supplementary gids,
    /// and nothing else here.
    pub fn u32_array(&mut self, values: &[u32]) {
        self.u32(u32::try_from(values.len()).unwrap_or(u32::MAX));
        for &value in values {
            self.u32(value);
        }
    }
}

impl fmt::Debug for Writer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "xdr::Writer({} bytes)", self.buf.len())
    }
}

/// Big-endian XDR decoder over a borrowed datagram.
///
/// Reads borrow from the datagram rather than copying it, so decoding a whole
/// RPC call allocates only for the fields that genuinely need owning.
///
/// Failure is not sticky: every method returns a [`Result`] and
/// the position does not advance on failure, so a caller that stops at the
/// first error stops at the right offset. [`Error::Truncated`] means the
/// buffer ended early and [`Error::ImplausibleLength`] means a prefix claimed
/// more than any real message carries; the two are never conflated, because on
/// a datagram protocol the first means "this peer sent a runt" and the second
/// means "this peer is not speaking XDR".
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Read from the start of `data`.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// The offset of the next unread byte, for error reporting.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// How many bytes are left.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Whether every byte has been consumed. A structure that decodes with
    /// bytes left over usually means a field was missed, not that the peer
    /// sent extra.
    pub fn at_end(&self) -> bool {
        self.remaining() == 0
    }

    /// Everything not yet read, consuming it.
    pub fn rest(&mut self) -> &'a [u8] {
        let tail = self.data.get(self.pos..).unwrap_or(&[]);
        self.pos = self.data.len();
        tail
    }

    /// Everything not yet read, without consuming it.
    pub fn peek_rest(&self) -> &'a [u8] {
        self.data.get(self.pos..).unwrap_or(&[])
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(count).ok_or(Error::Truncated {
            need: count,
            at: self.pos,
            have: self.remaining(),
        })?;
        let slice = self.data.get(self.pos..end).ok_or(Error::Truncated {
            need: count,
            at: self.pos,
            have: self.remaining(),
        })?;
        self.pos = end;
        Ok(slice)
    }

    /// Step over `count` bytes plus XDR padding, without interpreting them.
    pub fn skip(&mut self, count: usize) -> Result<()> {
        self.take(align4(count)).map(|_| ())
    }

    /// One 32-bit big-endian word.
    pub fn u32(&mut self) -> Result<u32> {
        let at = self.pos;
        let bytes = self.take(4)?;
        let array: [u8; 4] = bytes
            .try_into()
            .map_err(|_| Error::malformed(at, "a four-byte read returned a short slice"))?;
        Ok(u32::from_be_bytes(array))
    }

    /// A signed 32-bit word.
    ///
    /// The same four bytes as [`Reader::u32`]; XDR's `int` and `unsigned int`
    /// differ only in how they are read. Reinterpreted through the byte array
    /// rather than by a cast, so no conversion can silently change meaning.
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.u32()?.to_be_bytes()))
    }

    /// A 64-bit big-endian word. See [`Writer::u64`].
    pub fn u64(&mut self) -> Result<u64> {
        let at = self.pos;
        let bytes = self.take(8)?;
        let array: [u8; 8] = bytes
            .try_into()
            .map_err(|_| Error::malformed(at, "an eight-byte read returned a short slice"))?;
        Ok(u64::from_be_bytes(array))
    }

    /// XDR's boolean. Any non-zero word is true, as every implementation
    /// tolerates; refusing `2` here would drop a datagram over a field whose
    /// meaning is unambiguous anyway.
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u32()? != 0)
    }

    /// Fixed-length opaque of exactly `length` bytes, plus padding.
    pub fn opaque_fixed(&mut self, length: usize) -> Result<&'a [u8]> {
        let at = self.pos;
        let padded = align4(length);
        if padded > self.remaining() {
            return Err(Error::Truncated {
                need: padded,
                at,
                have: self.remaining(),
            });
        }
        let slice = self.take(length)?;
        self.take(padding_for(length))?;
        Ok(slice)
    }

    /// Variable-length opaque, rejecting an implausible prefix before
    /// allocating or slicing.
    ///
    /// `limit` is the largest length this field can legitimately have. A
    /// prefix above it fails with [`Error::ImplausibleLength`]; a prefix that
    /// merely exceeds the bytes present fails with [`Error::Truncated`].
    pub fn opaque_var(&mut self, limit: u32, what: &'static str) -> Result<&'a [u8]> {
        let at = self.pos;
        let length = self.u32()?;
        if length > limit {
            self.pos = at;
            return Err(Error::ImplausibleLength {
                what,
                length: u64::from(length),
                limit: u64::from(limit),
            });
        }
        let length = usize::try_from(length)
            .map_err(|_| Error::malformed(at, "length does not fit this platform's usize"))?;
        if align4(length) > self.remaining() {
            let have = self.remaining();
            self.pos = at;
            return Err(Error::Truncated {
                need: align4(length),
                at: at + 4,
                have,
            });
        }
        self.opaque_fixed(length)
    }

    /// A length-prefixed **ASCII** string. Bytes outside ASCII become `?`.
    pub fn ascii_string(&mut self, limit: u32) -> Result<String> {
        let bytes = self.opaque_var(limit, "an ASCII string")?;
        Ok(bytes
            .iter()
            .map(|&b| if b.is_ascii() { char::from(b) } else { '?' })
            .collect())
    }

    /// A length-prefixed **UTF-16LE** string, the prefix counting bytes.
    ///
    /// Returns the wire bytes wrapped in [`Utf16LeString`] rather than a
    /// decoded [`String`], because the bytes are the thing the protocol
    /// carries and the string is a reading of them.
    pub fn utf16le_string(&mut self, limit: u32) -> Result<Utf16LeString> {
        Ok(Utf16LeString::from_bytes(
            self.opaque_var(limit, "a UTF-16LE string")?,
        ))
    }

    /// A counted array of 32-bit words, capped before allocating.
    pub fn u32_array(&mut self, limit: u32) -> Result<Vec<u32>> {
        let at = self.pos;
        let count = self.u32()?;
        if count > limit {
            self.pos = at;
            return Err(Error::ImplausibleLength {
                what: "an array of 32-bit words",
                length: u64::from(count),
                limit: u64::from(limit),
            });
        }
        let count = usize::try_from(count)
            .map_err(|_| Error::malformed(at, "count does not fit this platform's usize"))?;
        // Four bytes each, checked before reserving so a large-but-under-cap
        // count cannot allocate more than the datagram could ever hold.
        if count.saturating_mul(4) > self.remaining() {
            let have = self.remaining();
            self.pos = at;
            return Err(Error::Truncated {
                need: count.saturating_mul(4),
                at: at + 4,
                have,
            });
        }
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(self.u32()?);
        }
        Ok(values)
    }
}

impl fmt::Debug for Reader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "xdr::Reader(at {} of {})", self.pos, self.data.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align4_rounds_up_to_the_next_word() {
        assert_eq!(
            [0, 1, 3, 4, 5, 8].map(align4),
            [0, 4, 4, 4, 8, 8],
            "XDR pads every field up to four bytes"
        );
    }

    #[test]
    fn integers_are_big_endian() {
        let mut writer = Writer::new();
        writer.u32(1);
        assert_eq!(writer.as_bytes(), b"\x00\x00\x00\x01");

        let mut writer = Writer::new();
        writer.u32(0xdead_beef);
        assert_eq!(writer.as_bytes(), &[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(
            Reader::new(&[0xde, 0xad, 0xbe, 0xef]).u32().unwrap(),
            0xdead_beef
        );
    }

    /// The mistake that produces `NFSERR_NOENT` and no other clue.
    ///
    /// `/C/` is three characters and six UTF-16LE bytes; the prefix must say
    /// six. These exact bytes came off a CDJ-2000NXS in a MOUNT `EXPORT`
    /// reply — `'/C/' raw=2f0043002f00` (F12).
    #[test]
    fn a_utf16le_prefix_counts_bytes_not_characters() {
        let mut writer = Writer::new();
        writer.utf16le_string(&Utf16LeString::new("/C/"));
        let encoded = writer.into_bytes();
        assert_eq!(
            encoded.get(..4),
            Some(b"\x00\x00\x00\x06".as_slice()),
            "the prefix must be the BYTE count, not the character count"
        );
        assert_eq!(encoded.get(4..10), Some(b"/\x00C\x00/\x00".as_slice()));
        // Six bytes padded to eight, plus the four-byte prefix.
        assert_eq!(encoded.len(), 12);
        assert_eq!(
            encoded,
            [
                0x00, 0x00, 0x00, 0x06, 0x2f, 0x00, 0x43, 0x00, 0x2f, 0x00, 0x00, 0x00
            ]
        );
    }

    #[test]
    fn the_sd_export_name_encodes_as_captured() {
        // `/B/`, the SD export a deck mounts (F37). Same shape as F12's `/C/`.
        let mut writer = Writer::new();
        writer.utf16le_string(&Utf16LeString::new("/B/"));
        assert_eq!(
            writer.into_bytes(),
            [
                0x00, 0x00, 0x00, 0x06, 0x2f, 0x00, 0x42, 0x00, 0x2f, 0x00, 0x00, 0x00
            ]
        );
    }

    #[test]
    fn utf16le_strings_round_trip() {
        for name in ["/B/", "/C/", "/", "PIONEER", "export.pdb", "/C/EXPORT"] {
            let mut writer = Writer::new();
            writer.utf16le_string(&Utf16LeString::new(name));
            let encoded = writer.into_bytes();
            let decoded = Reader::new(&encoded).utf16le_string(MAX_STRING).unwrap();
            assert_eq!(decoded.to_string_lossy(), name, "round trip of {name:?}");
        }
    }

    /// O6: an encoder and a decoder that agree with each other prove nothing
    /// about non-ASCII input, which is where the reference implementation's
    /// bug lived. `カガミ` is a real directory name off a rekordbox medium.
    #[test]
    fn a_non_ascii_name_encodes_to_its_utf16le_code_units() {
        let name = Utf16LeString::new("カガミ");
        assert_eq!(name.as_bytes(), &[0xab, 0x30, 0xac, 0x30, 0xdf, 0x30]);
        assert_eq!(name.len_bytes(), 6, "three characters, six bytes");
        assert_eq!(name.to_string_lossy(), "カガミ");
    }

    #[test]
    fn a_name_outside_the_basic_plane_uses_a_surrogate_pair() {
        let name = Utf16LeString::new("🎧");
        assert_eq!(name.as_bytes(), &[0x3c, 0xd8, 0xa7, 0xdf]);
        assert_eq!(
            name.len_bytes(),
            4,
            "one character, two code units, four bytes"
        );
        assert_eq!(name.to_string_lossy(), "🎧");
    }

    #[test]
    fn malformed_utf16_survives_a_round_trip_rather_than_being_normalised() {
        // A lone high surrogate, and an odd trailing byte. Neither is valid
        // UTF-16; both must come back out exactly as they went in, because a
        // LOOKUP we forward has to be the one the deck asked for.
        let raw = [0x3c, 0xd8, 0x41, 0x00, 0x7f];
        let name = Utf16LeString::from_bytes(&raw);
        assert_eq!(name.as_bytes(), raw);
        assert_eq!(name.to_string_lossy(), "\u{fffd}A\u{fffd}");

        let mut writer = Writer::new();
        writer.utf16le_string(&name);
        let encoded = writer.into_bytes();
        let decoded = Reader::new(&encoded).utf16le_string(MAX_STRING).unwrap();
        assert_eq!(decoded.as_bytes(), raw, "bytes must survive verbatim");
    }

    #[test]
    fn an_ascii_string_is_not_utf16() {
        // The other half of C7: within one EXPORT reply, the path is UTF-16LE
        // and the group is ASCII. This is the group.
        let mut writer = Writer::new();
        writer.ascii_string("169.254.0.0/255.255.0.0");
        let encoded = writer.into_bytes();
        assert_eq!(encoded.get(..4), Some(b"\x00\x00\x00\x17".as_slice()));
        assert_eq!(
            encoded.get(4..27),
            Some(b"169.254.0.0/255.255.0.0".as_slice())
        );
        assert_eq!(encoded.len(), 28, "23 bytes padded to 24, plus the prefix");
    }

    #[test]
    fn a_thirty_two_byte_filehandle_travels_bare_and_unprefixed() {
        let handle: Vec<u8> = (0..32).collect();
        let mut writer = Writer::new();
        writer.opaque_fixed(&handle);
        let encoded = writer.into_bytes();
        assert_eq!(encoded, handle, "already aligned: no prefix, no padding");
        assert_eq!(Reader::new(&encoded).opaque_fixed(32).unwrap(), handle);
    }

    /// A real CDJ leaves uninitialised bytes in the padding. These twelve are
    /// the argument block of a captured `MNT('/C/')` from a CDJ-2000NXS —
    /// note `0011` where the standard asks for `0000`.
    #[test]
    fn padding_is_skipped_rather_than_checked() {
        let captured = [
            0x00, 0x00, 0x00, 0x06, 0x2f, 0x00, 0x43, 0x00, 0x2f, 0x00, 0x00, 0x11,
        ];
        let mut reader = Reader::new(&captured);
        let path = reader.utf16le_string(MAX_STRING).unwrap();
        assert_eq!(path.to_string_lossy(), "/C/");
        assert!(
            reader.at_end(),
            "the two padding bytes must be consumed, whatever they hold"
        );
    }

    #[test]
    fn opaque_fixed_pads_an_unaligned_length() {
        let mut writer = Writer::new();
        writer.opaque_fixed(b"abc");
        assert_eq!(writer.into_bytes(), b"abc\x00");
    }

    /// Padding belongs to the field, not to the buffer. `raw` is public, so a
    /// caller can leave the buffer unaligned; a field written after that must
    /// still carry its own padding, or a reader counting fields desynchronises
    /// even though ours — which counts from the same misaligned place — would
    /// not notice.
    #[test]
    fn a_field_is_padded_by_its_own_length_not_by_the_buffers() {
        let mut writer = Writer::new();
        writer.raw(&[0xaa]);
        writer.opaque_fixed(b"abc");
        assert_eq!(
            writer.into_bytes(),
            b"\xaaabc\x00",
            "three bytes still take one byte of padding"
        );

        let mut writer = Writer::new();
        writer.raw(&[0xaa, 0xbb, 0xcc]);
        writer.opaque_var(b"ab");
        assert_eq!(
            writer.into_bytes(),
            b"\xaa\xbb\xcc\x00\x00\x00\x02ab\x00\x00",
            "two bytes still take two of padding"
        );
    }

    #[test]
    fn opaque_var_pads_to_four() {
        let mut writer = Writer::new();
        writer.opaque_var(b"hello");
        let encoded = writer.into_bytes();
        assert_eq!(encoded, b"\x00\x00\x00\x05hello\x00\x00\x00");
        assert_eq!(
            Reader::new(&encoded)
                .opaque_var(MAX_STRING, "test")
                .unwrap(),
            b"hello"
        );
    }

    /// The property that has to survive every port of this code: a reply
    /// claiming four gigabytes costs a parse failure, not four gigabytes.
    #[test]
    fn an_absurd_length_is_rejected_without_allocating() {
        let hostile = b"\xff\xff\xff\xfftiny";
        let error = Reader::new(hostile)
            .opaque_var(MAX_STRING, "a UTF-16LE string")
            .unwrap_err();
        assert!(
            matches!(error, Error::ImplausibleLength { length, .. } if length == 0xffff_ffff),
            "expected ImplausibleLength, got {error:?}"
        );
        assert!(
            !error.is_truncated(),
            "a hostile prefix is not a short datagram; more bytes will not help"
        );
    }

    #[test]
    fn a_length_beyond_the_datagram_is_truncation_not_garbage() {
        let short = b"\x00\x00\x01\x00only eight";
        let error = Reader::new(short)
            .opaque_var(MAX_STRING, "a UTF-16LE string")
            .unwrap_err();
        assert!(
            error.is_truncated(),
            "a plausible length with too few bytes means the datagram was cut short: {error:?}"
        );
    }

    #[test]
    fn a_failed_read_leaves_the_position_where_it_was() {
        let mut reader = Reader::new(b"\xff\xff\xff\xfftiny");
        assert!(reader.opaque_var(MAX_STRING, "test").is_err());
        assert_eq!(
            reader.position(),
            0,
            "a caller that stops at the first error must stop at the right offset"
        );
    }

    #[test]
    fn reading_past_the_end_is_truncation() {
        let mut reader = Reader::new(b"\x00\x00\x00\x01");
        assert_eq!(reader.u32().unwrap(), 1);
        assert!(reader.at_end());
        let error = reader.u32().unwrap_err();
        assert!(error.is_truncated(), "got {error:?}");
    }

    #[test]
    fn an_array_count_is_capped_before_allocating() {
        let error = Reader::new(b"\xff\xff\xff\xff").u32_array(16).unwrap_err();
        assert!(
            matches!(error, Error::ImplausibleLength { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn an_empty_gid_array_is_a_bare_zero() {
        let mut writer = Writer::new();
        writer.u32_array(&[]);
        assert_eq!(writer.as_bytes(), b"\x00\x00\x00\x00");
        assert_eq!(
            Reader::new(b"\x00\x00\x00\x00").u32_array(16).unwrap(),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn booleans_round_trip() {
        for value in [true, false] {
            let mut writer = Writer::new();
            writer.bool(value);
            assert_eq!(Reader::new(writer.as_bytes()).bool().unwrap(), value);
        }
        assert!(
            Reader::new(b"\x00\x00\x00\x02").bool().unwrap(),
            "any non-zero word is true"
        );
    }

    #[test]
    fn a_signed_word_reads_back_signed() {
        let mut writer = Writer::new();
        writer.i32(-1);
        assert_eq!(writer.as_bytes(), b"\xff\xff\xff\xff");
        assert_eq!(Reader::new(b"\xff\xff\xff\xff").i32().unwrap(), -1);
    }
}
