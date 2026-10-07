// SPDX-License-Identifier: GPL-3.0-only

//! The metadata protocol the LINK button drives — TCP 1051, "remotedb".
//!
//! Everything a CDJ shows about another player's media travels here: the root
//! categories, every list behind them, a track's metadata, its file path and
//! size, and its artwork. NFS makes a medium's *files* readable; this makes the
//! medium *browsable*, and the two are independent — a deck that can read every
//! byte over NFS will still show an empty menu if this is missing or wrong.
//!
//! Written from `docs/PROTOCOL.md` §5 and validated against **11 809 messages**
//! reassembled from every dbserver stream in the capture corpus — 30 streams
//! across 45 sessions plus dysentery's two `LinkInfo` files. Every stream is
//! framed by this decoder alone and consumed end to end with nothing left over,
//! and every message re-encodes byte for byte (F7). Framing the whole stream is
//! the check that matters, because a single misstep on the omitted-blob rule
//! below desynchronises everything after it silently.
//!
//! # Finding the port, and the preamble
//!
//! The port is nominally dynamic. A client asks TCP [`PORT_QUERY_PORT`] with
//! the fixed 19-byte [`PORT_QUERY`] and gets a two-byte big-endian answer; both
//! reference captures answer [`PORT`], as does every deck we have seen, but the
//! query is cheap and skipping it is one more way to look unlike a CDJ.
//!
//! Then, **in both directions and before any message**, each peer sends the
//! five-byte [`PREAMBLE`] — a bare `UInt32` field holding 1. A stream decoder
//! that goes looking for the first magic without stepping over it fails on the
//! first byte, so [`skip_preamble`] exists and is not optional.
//!
//! # Framing, and why truncation is not an error
//!
//! ```text
//! 11 87 23 49 ae   magic, itself a tagged UInt32
//! 11 xx xx xx xx   transaction id
//! 10 xx xx         message type
//! 0f xx            argument count, at most 12
//! 14 00 00 00 0c   the 12-byte argument-tag blob
//! …                the arguments
//! ```
//!
//! A message carries **no length prefix**. It is framed by nothing but its own
//! contents, so the only way to know whether a TCP buffer holds a whole one is
//! to try, and running off the end is the *expected* outcome of trying too
//! early. [`Message::decode`] reports how many bytes it consumed, and
//! [`Error::is_truncated`] is what separates "wait for more" from "this peer is
//! not speaking the protocol" — the second of which has no recovery, because
//! there is no frame boundary to resynchronise on.
//!
//! # Three things that are easy to get wrong
//!
//! **Two independent type numberings.** Every value on the wire is a tagged
//! field ([`FieldTag`]: `0f`/`10`/`11`/`14`/`26`) and the header *also* carries
//! a twelve-byte blob of *argument* tags ([`ArgTag`]: `02`/`03`/`06`)
//! describing the same arguments with entirely different numbers. Both must
//! agree. Here they cannot disagree: a [`Field`] yields both, from one match on
//! one enum, and the header blob is derived at encode time rather than stored.
//!
//! **A zero-length binary argument is absent from the wire.** Not sent as an
//! empty blob: gone, with the preceding `UInt32` length argument as the only
//! evidence it was ever declared. This is the rule that silently
//! desynchronises a naive parser — the reader consumes the next message's magic
//! as a field and every argument after that is one position out, with no error
//! to show for it — and it is the *common* case rather than an exotic one: a
//! player answers `GetArtwork` for a track with no art exactly this way.
//! Encoding is the exact inverse of decoding here, so a message omits an
//! argument if and only if a decoder would infer its absence.
//!
//! **Strings count characters, not bytes.** The prefix is the UTF-16 length
//! *including a terminating NUL*, so a three-character string announces 4 and
//! carries 8 bytes, and the text is UTF-16 **big**-endian. The NFS half of the
//! same protocol sends UTF-16 **little**-endian counted in **bytes**. The two
//! conventions are opposites in both axes and must never share a helper:
//! [`encode_string`] here, [`crate::rpc::xdr`] there. Within one menu item both
//! units appear at once — arguments 2 and 4 are label lengths in *bytes* while
//! the string fields beside them are counted in *characters* — which is why
//! [`string_characters`] and [`label_bytes`] are separately named functions and
//! why [`MenuItem`] derives both rather than letting a caller supply them.
//!
//! # Why this is hand-rolled and not `binrw`
//!
//! `ksy/prolink_dbserver.ksy` shows the read direction expresses fine
//! declaratively. The write direction does not: the argument-tag blob is a
//! summary of the arguments that follow, and whether an argument appears at all
//! depends on the *value* of the argument before it. Expressed in `binrw` those
//! two rules would have to be written once for reading and again for writing,
//! and duplicating the omitted-blob rule is precisely the bug it causes. A
//! cursor over `&[u8]` states each rule once, in code both directions call, and
//! gives the consumed-byte count a stream reader needs for free.
//!
//! # Vocabulary
//!
//! The second half of this module is the protocol's *nouns*: message types,
//! item types, the root categories, sort orders and the menu-item layout. They
//! are tables of observed values wherever the evidence allows only a table.
//! Five bugs in the reference implementation came from deriving a value that
//! looked derivable (F26, F40, F27/F41, F31/F35), so a derivation here is used
//! only where the *whole grid* has been observed — [`drill_kind`] — and the
//! root categories, which two separate derivations got wrong in two different
//! places, are simply listed (F43).

use std::fmt;

use crate::device::{BrowsableDeviceNumber, DeviceNumber};
use crate::{Error, Result, Slot};

// -- constants ------------------------------------------------------------

/// The `UInt32` every message starts with.
pub const MAGIC: u32 = 0x8723_49AE;

/// The port a real player serves on, and the answer both reference captures
/// give to the port query. Documented as dynamic; ask anyway.
pub const PORT: u16 = 1051;

/// The fixed port that answers "which port is the dbserver on?".
pub const PORT_QUERY_PORT: u16 = 12523;

/// The 19-byte port query: a big-endian length, the ASCII name, a NUL.
///
/// The length `0x0f` is 15 — the fourteen name bytes **plus** the NUL, counted
/// in bytes. A third length convention, in a protocol that already has two.
pub const PORT_QUERY: [u8; 19] = [
    0x00, 0x00, 0x00, 0x0f, b'R', b'e', b'm', b'o', b't', b'e', b'D', b'B', b'S', b'e', b'r', b'v',
    b'e', b'r', 0x00,
];

/// The five-byte connection preamble, exchanged in **both** directions before
/// any message: a bare `UInt32` field holding 1.
pub const PREAMBLE: [u8; 5] = [FieldTag::U32.0, 0x00, 0x00, 0x00, 0x01];

/// The transaction id a player reserves for `Introduce` and `Disconnect`.
pub const SETUP_TRANSACTION_ID: u32 = 0xFFFF_FFFE;

/// Where a real player's ordinary transaction ids start.
///
/// The pre-hardware literature says they start at 1 and count up. Every
/// conversation in every capture begins around `0x03800001` instead (C10). The
/// value is opaque — a server only echoes it — so nothing breaks either way,
/// but a client counting from 1 is one more way to look unlike a CDJ.
pub const FIRST_TRANSACTION_ID: u32 = 0x0380_0001;

/// Menu rows per render a Nexus 2 is documented to tolerate.
///
/// Thousands in one render demonstrably fail. Some hardware takes more; the
/// safe batch size is hardware-dependent and paging costs nothing.
pub const MAX_RENDER_BATCH: u32 = 64;

/// Length of the argument-tag blob. Fixed: one byte per possible argument, and
/// there are twelve.
const ARG_TAG_BLOB_LEN: usize = 12;

/// Bytes of a message before its arguments: five tagged header fields.
const HEADER_LEN: usize = 5 + 5 + 3 + 2 + 5 + ARG_TAG_BLOB_LEN;

/// Ceiling on a binary argument, enforced before allocating.
///
/// A decoder sizes the buffer from a length word it has not yet corroborated,
/// so without a cap one corrupt word asks for four gigabytes. Well above any
/// real payload: the largest thing this protocol carries is a cover image, and
/// the largest observed is under a kilobyte.
const MAX_BLOB_LEN: u32 = 16 * 1024 * 1024;

/// Ceiling on a string argument, in characters. Doubled to get bytes, so this
/// is [`MAX_BLOB_LEN`] in the same units.
const MAX_STRING_CHARS: u32 = MAX_BLOB_LEN / 2;

/// Step over the connection preamble if `data` begins with one.
///
/// Both peers send it once, before anything else, so a decoder that goes
/// straight for the first magic fails on byte zero of every connection.
pub fn skip_preamble(data: &[u8]) -> &[u8] {
    match data.get(..PREAMBLE.len()) {
        Some(head) if head == PREAMBLE => data.get(PREAMBLE.len()..).unwrap_or(&[]),
        _ => data,
    }
}

/// The two-byte answer to a [`PORT_QUERY`].
pub fn encode_port_reply(port: u16) -> [u8; 2] {
    port.to_be_bytes()
}

/// Read the two-byte answer to a [`PORT_QUERY`].
pub fn decode_port_reply(data: &[u8]) -> Result<u16> {
    let bytes: [u8; 2] = data
        .get(..2)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(Error::Truncated {
            need: 2,
            at: 0,
            have: data.len(),
        })?;
    Ok(u16::from_be_bytes(bytes))
}

// -- the two numberings ---------------------------------------------------

/// The tag byte that precedes every value on the wire.
///
/// One of the protocol's **two** numberings for the same five types. The other
/// is [`ArgTag`], and a message whose two disagree is rejected by real
/// hardware. Nothing in this module lets them drift apart: both come from
/// [`Field`].
///
/// A newtype rather than an enum because it is a wire enumeration; an unknown
/// tag is a decode failure rather than a variant, and the raw byte survives
/// into the error message.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FieldTag(pub u8);

impl FieldTag {
    /// A one-byte integer.
    pub const U8: Self = Self(0x0f);
    /// A two-byte big-endian integer.
    pub const U16: Self = Self(0x10);
    /// A four-byte big-endian integer.
    pub const U32: Self = Self(0x11);
    /// A length-prefixed blob of bytes.
    pub const BLOB: Self = Self(0x14);
    /// A UTF-16 big-endian string, counted in characters including its NUL.
    pub const TEXT: Self = Self(0x26);

    /// The same type in the *other* numbering.
    pub const fn arg_tag(self) -> Option<ArgTag> {
        Some(match self {
            Self::U8 => ArgTag::U8,
            Self::U16 => ArgTag::U16,
            Self::U32 => ArgTag::U32,
            Self::BLOB => ArgTag::BLOB,
            Self::TEXT => ArgTag::TEXT,
            _ => return None,
        })
    }
}

impl fmt::Debug for FieldTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::U8 => f.write_str("u8"),
            Self::U16 => f.write_str("u16"),
            Self::U32 => f.write_str("u32"),
            Self::BLOB => f.write_str("blob"),
            Self::TEXT => f.write_str("text"),
            Self(raw) => write!(f, "FieldTag({raw:#04x})"),
        }
    }
}

/// The *other* numbering, used only inside the header's twelve-byte blob.
///
/// Two numberings for one set of types is a wart of the protocol, not of this
/// code. Only three of the five have ever been seen on the wire — 77 119
/// `UInt32`, 12 638 string and 347 binary arguments in the corpus — and in
/// every one of them the header tag and the value's own tag agree.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArgTag(pub u8);

impl ArgTag {
    /// A UTF-16 big-endian string.
    pub const TEXT: Self = Self(0x02);
    /// A blob — and the one tag a decoder must recognise, because a zero-length
    /// blob argument is absent from the wire entirely.
    pub const BLOB: Self = Self(0x03);
    /// A one-byte integer. **Inferred, never observed.**
    pub const U8: Self = Self(0x04);
    /// A two-byte integer. **Inferred, never observed.**
    pub const U16: Self = Self(0x05);
    /// A four-byte integer.
    pub const U32: Self = Self(0x06);

    /// The same type in the numbering used in front of values.
    pub const fn field_tag(self) -> Option<FieldTag> {
        Some(match self {
            Self::U8 => FieldTag::U8,
            Self::U16 => FieldTag::U16,
            Self::U32 => FieldTag::U32,
            Self::BLOB => FieldTag::BLOB,
            Self::TEXT => FieldTag::TEXT,
            _ => return None,
        })
    }
}

impl fmt::Debug for ArgTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TEXT => f.write_str("text"),
            Self::BLOB => f.write_str("blob"),
            Self::U8 => f.write_str("u8"),
            Self::U16 => f.write_str("u16"),
            Self::U32 => f.write_str("u32"),
            Self(raw) => write!(f, "ArgTag({raw:#04x})"),
        }
    }
}

// -- strings --------------------------------------------------------------

/// The terminator a well-formed dbserver string ends with, and the one
/// character the count includes over and above the text.
pub const NUL: u16 = 0x0000;

/// Encode a dbserver string.
///
/// UTF-16 **big**-endian, length-prefixed in *characters* — UTF-16 code units —
/// **including a terminating NUL**. `"abc"` announces 4 and carries 8 bytes.
///
/// The NFS half of this protocol encodes UTF-16 **little**-endian counted in
/// **bytes**. Neither the endianness nor the unit is shared, so a value that
/// crosses between the two layers must be re-encoded, never copied.
pub fn encode_string(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + text.len() * 2 + 2);
    write_string(&mut out, text, Some(NUL));
    out
}

/// How many characters a string announces: its UTF-16 length **plus the NUL**.
///
/// Code units, not code points, which differ for anything outside the basic
/// multilingual plane. The wire counts what it carries.
pub fn string_characters(text: &str) -> u32 {
    utf16_units(text).saturating_add(1)
}

/// The UTF-16 length of `text`, before the terminator is counted.
fn utf16_units(text: &str) -> u32 {
    u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX)
}

/// Write a string body: the character count, the text, then the terminator.
fn write_string(out: &mut Vec<u8>, text: &str, terminator: Option<u16>) {
    let count = utf16_units(text).saturating_add(u32::from(terminator.is_some()));
    out.extend_from_slice(&count.to_be_bytes());
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    if let Some(terminator) = terminator {
        out.extend_from_slice(&terminator.to_be_bytes());
    }
}

/// How many **bytes** a label occupies, for a menu item's length arguments.
///
/// Twice [`string_characters`]. Arguments 2 and 4 of a [`MenuItem`] carry this
/// while the string fields beside them carry the character count, so the two
/// units sit four bytes apart on the wire. `Above & Beyond` is fourteen
/// characters and announces `0x1e` (F7).
pub fn label_bytes(text: &str) -> u32 {
    string_characters(text).saturating_mul(2)
}

/// Decode a UTF-16 big-endian string body into its text and its terminator.
///
/// The **last** code unit is the terminator whatever its value, because the
/// count says so: it is the text's length plus one. Splitting on position
/// rather than on the unit being NUL is what makes this the exact inverse of
/// [`write_string`], and it is not academic — see [`Field::Text`].
fn decode_string_body(bytes: &[u8]) -> (String, Option<u16>) {
    let mut units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            u16::from_be_bytes([
                pair.first().copied().unwrap_or(0),
                pair.get(1).copied().unwrap_or(0),
            ])
        })
        .collect();
    let terminator = units.pop();
    (String::from_utf16_lossy(&units), terminator)
}

// -- fields ---------------------------------------------------------------

/// One tagged argument.
///
/// The single point at which the protocol's two numberings are decided:
/// [`Field::tag`] gives the byte written in front of the value and
/// [`Field::arg_tag`] the byte written into the header blob, both from one
/// match on one enum. There is no way to set one and forget the other.
#[derive(Clone, PartialEq, Eq)]
pub enum Field {
    /// A one-byte integer.
    U8(u8),
    /// A two-byte big-endian integer.
    U16(u16),
    /// A four-byte big-endian integer — nearly every argument in practice.
    U32(u32),
    /// A blob of bytes. An **empty** one is not written to the wire at all;
    /// see the module documentation.
    Blob(Vec<u8>),
    /// A UTF-16 big-endian string, counted in characters including its
    /// terminator. Build one with `Field::from("…")`.
    Text {
        /// The characters, without the terminator.
        text: String,
        /// The final code unit, and `None` only for a zero-character field —
        /// which no capture contains, but which the count can express.
        ///
        /// **Stored rather than assumed, because a real CDJ does not always
        /// write a NUL** — *a new observation, not yet in the research record.*
        /// 28 of the 12 638 string fields in the corpus terminate in `0x0009`
        /// instead, and every one of them is a label of a `MENU_ITEM` title row
        /// whose byte-length argument says that label is empty. So the deck
        /// announces one character, meaning "the terminator and nothing else",
        /// and the byte pair it writes there is stale rather than zero. It
        /// occurs in four independent capture sessions — `S06-load-and-play`,
        /// `S13-format-ground-truth`, `S15a-sd-alone` and `S15b-sd-and-usb` —
        /// so it is the hardware and not the tap.
        ///
        /// Normalising it to a NUL costs one word of difference from what a
        /// real deck sent, which is one word too many. Both reference
        /// implementations do exactly that and their round-trip tests do not
        /// catch it, because the only capture those tests read is the one
        /// capture that happens not to contain the case.
        terminator: Option<u16>,
    },
}

impl Field {
    /// The tag byte written in front of this value.
    pub const fn tag(&self) -> FieldTag {
        match *self {
            Self::U8(_) => FieldTag::U8,
            Self::U16(_) => FieldTag::U16,
            Self::U32(_) => FieldTag::U32,
            Self::Blob(_) => FieldTag::BLOB,
            Self::Text { .. } => FieldTag::TEXT,
        }
    }

    /// The tag byte written into the header's twelve-byte blob for this value.
    pub const fn arg_tag(&self) -> ArgTag {
        match *self {
            Self::U8(_) => ArgTag::U8,
            Self::U16(_) => ArgTag::U16,
            Self::U32(_) => ArgTag::U32,
            Self::Blob(_) => ArgTag::BLOB,
            Self::Text { .. } => ArgTag::TEXT,
        }
    }

    /// The integer this field carries, widened, or `None` if it is not one.
    pub fn number(&self) -> Option<u32> {
        Some(match *self {
            Self::U8(value) => u32::from(value),
            Self::U16(value) => u32::from(value),
            Self::U32(value) => value,
            _ => return None,
        })
    }

    /// The text this field carries, or `None` if it is not a string.
    ///
    /// Never includes the terminator, whatever the terminator turned out to be.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            _ => None,
        }
    }

    /// The bytes this field carries, or `None` if it is not a blob.
    pub fn blob(&self) -> Option<&[u8]> {
        match self {
            Self::Blob(value) => Some(value),
            _ => None,
        }
    }

    /// What this field contributes to the *next* argument's presence test.
    ///
    /// The omitted-blob rule reads the preceding argument's integer value and
    /// treats a non-integer — or an argument that was itself omitted — as 1,
    /// meaning "not a zero length". Only a genuine numeric zero suppresses the
    /// blob that follows it.
    fn presence_value(&self) -> u32 {
        self.number().unwrap_or(1)
    }

    /// Append this field's wire form.
    fn write(&self, out: &mut Vec<u8>) {
        out.push(self.tag().0);
        match self {
            Self::U8(value) => out.push(*value),
            Self::U16(value) => out.extend_from_slice(&value.to_be_bytes()),
            Self::U32(value) => out.extend_from_slice(&value.to_be_bytes()),
            Self::Blob(value) => {
                let length = u32::try_from(value.len()).unwrap_or(u32::MAX);
                out.extend_from_slice(&length.to_be_bytes());
                out.extend_from_slice(value);
            }
            Self::Text { text, terminator } => write_string(out, text, *terminator),
        }
    }
}

impl fmt::Debug for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::U8(value) => write!(f, "{value:#04x}"),
            Self::U16(value) => write!(f, "{value:#06x}"),
            Self::U32(value) => write!(f, "{value:#010x}"),
            Self::Blob(value) => write!(f, "<{}B>", value.len()),
            Self::Text {
                text,
                terminator: Some(NUL) | None,
            } => write!(f, "{text:?}"),
            Self::Text { text, terminator } => write!(f, "{text:?}+{terminator:04x?}"),
        }
    }
}

impl From<u32> for Field {
    fn from(value: u32) -> Self {
        Self::U32(value)
    }
}

impl From<&str> for Field {
    fn from(value: &str) -> Self {
        Self::from(value.to_owned())
    }
}

impl From<String> for Field {
    fn from(text: String) -> Self {
        Self::Text {
            text,
            terminator: Some(NUL),
        }
    }
}

impl From<Vec<u8>> for Field {
    fn from(value: Vec<u8>) -> Self {
        Self::Blob(value)
    }
}

// -- arguments ------------------------------------------------------------

/// A message's arguments: at most twelve.
///
/// Twelve is structural, not a limit we chose: the header describes the
/// arguments with a twelve-byte blob, one byte each, so a thirteenth argument
/// could not be described and a message carrying one is unencodable. Making
/// that a property of the type means the encoder never has to decide what to do
/// about it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Arguments(Vec<Field>);

impl Arguments {
    /// The most arguments the header's tag blob can describe.
    pub const MAX: usize = ARG_TAG_BLOB_LEN;

    /// Collect arguments, or `None` for more than [`Arguments::MAX`].
    pub fn new(fields: impl IntoIterator<Item = Field>) -> Option<Self> {
        let fields: Vec<Field> = fields.into_iter().collect();
        (fields.len() <= Self::MAX).then_some(Self(fields))
    }

    /// Every argument, in order.
    pub fn as_slice(&self) -> &[Field] {
        &self.0
    }

    /// How many arguments there are.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none. `MENU_FOOTER` and `MENU_CLOSE` both look like
    /// this.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Argument `index`, if it exists.
    pub fn get(&self, index: usize) -> Option<&Field> {
        self.0.get(index)
    }

    /// Argument `index` as an integer, or `None` if absent or not one.
    pub fn number(&self, index: usize) -> Option<u32> {
        self.get(index).and_then(Field::number)
    }

    /// Argument `index` as text, or `None` if absent or not a string.
    pub fn text(&self, index: usize) -> Option<&str> {
        self.get(index).and_then(Field::text)
    }

    /// Argument `index` as bytes, or `None` if absent or not a blob.
    pub fn blob(&self, index: usize) -> Option<&[u8]> {
        self.get(index).and_then(Field::blob)
    }
}

impl<const N: usize> From<[Field; N]> for Arguments {
    fn from(fields: [Field; N]) -> Self {
        const {
            assert!(
                N <= Arguments::MAX,
                "a dbserver message carries at most twelve arguments"
            );
        }
        Self(fields.into())
    }
}

impl fmt::Debug for Arguments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.0).finish()
    }
}

// -- message types --------------------------------------------------------

/// The type word at offset 11 of every message.
///
/// Requests are `0x0nnn`–`0x3nnn` and replies `0x4nnn`. A newtype rather than an
/// enum: the undocumented types below were found by watching hardware, the
/// corpus holds 58 distinct types against the 51 named here, and refusing an
/// unknown one would break browsing for the types we do understand.
///
/// **Erroring on an unknown request is not free.** Answering `0x3e03` with
/// [`MessageKind::ERROR`] made a real deck fetch our root menu, render every
/// category, and then disconnect without opening any of them (F25). An error
/// and an empty folder are indistinguishable on a CDJ's screen, so the set of
/// types a server answers is a user-visible surface rather than an internal
/// detail (F40).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageKind(pub u16);

impl MessageKind {
    /// "I am device N." The first message on a connection; the reply carries
    /// the *server's* number, the one `SUCCESS` whose argument 1 is not a count.
    pub const INTRODUCE: Self = Self(0x0000);
    /// Sent 23 times in one CDJ-to-CDJ browse (99 times across the corpus),
    /// reusing the transaction id of the `RENDER_MENU` it follows, and drawing
    /// **no reply at all** — the packet accounting in that capture is exact and
    /// leaves nothing over for it (F16).
    ///
    /// "Done with that menu, release its state" is the natural reading and
    /// acting on it is a bug: a deck sends this while still paging through the
    /// list it is supposedly finished with, so honouring it destroys the result
    /// set mid-scroll (F27). Reply with nothing, discard nothing.
    pub const MENU_CLOSE: Self = Self(0x0001);
    /// "I am leaving."
    pub const DISCONNECT: Self = Self(0x0100);

    /// The root category list.
    pub const MENU_ROOT: Self = Self(0x1000);
    /// All genres.
    pub const MENU_GENRE: Self = Self(0x1001);
    /// All artists.
    pub const MENU_ARTIST: Self = Self(0x1002);
    /// All albums.
    pub const MENU_ALBUM: Self = Self(0x1003);
    /// All tracks. Argument 1 is the sort order.
    pub const MENU_TRACK: Self = Self(0x1004);
    /// All tempos.
    pub const MENU_BPM: Self = Self(0x1006);
    /// All ratings.
    pub const MENU_RATING: Self = Self(0x1007);
    /// All years.
    pub const MENU_YEAR: Self = Self(0x1008);
    /// All record labels.
    pub const MENU_LABEL: Self = Self(0x100a);
    /// All colours.
    pub const MENU_COLOR: Self = Self(0x100d);
    /// All durations.
    pub const MENU_TIME: Self = Self(0x1010);
    /// All bitrates.
    pub const MENU_BITRATE: Self = Self(0x1011);
    /// History playlists.
    pub const MENU_HISTORY: Self = Self(0x1012);
    /// Browse by filename.
    pub const MENU_FILENAME: Self = Self(0x1013);
    /// All keys. Drilling into one reaches a harmonic tolerance first (F44).
    pub const MENU_KEY: Self = Self(0x1014);
    /// "Give me the tag list" — the tracks tagged with the TAG TRACK button.
    ///
    /// Answered with ordinary track rows, not with a category listing. Named
    /// from a real deck: the only two occurrences in this corpus are in the
    /// session where tracks were being tagged, and the deck answered both with
    /// title rows for tagged tracks while `MENU_KEY` in the same corpus is
    /// answered with key names (F53).
    ///
    /// **The tag list is the server's state, not the browsing deck's.** A deck
    /// asks its source for the list rather than remembering what it tagged, so
    /// a server that acknowledges [`Self::TAG_LIST_ADD`] without storing
    /// anything leaves the menu permanently empty — which is what it does.
    pub const MENU_TAG_LIST: Self = Self(0x100f);
    /// Playlists and playlist folders. Argument 1 is the sort order (F43).
    pub const MENU_PLAYLIST: Self = Self(0x1105);
    /// Search as you type: `[descriptor, sort, byte length, text, 0]`, one
    /// request per keystroke. **The text is argument 3**, not argument 2 (F44).
    pub const MENU_SEARCH: Self = Self(0x1300);
    /// "Which sort orders does this menu offer?" Argument 2 names the menu and
    /// the reply is [`SORT_MENU`] regardless.
    pub const MENU_SORT: Self = Self(0x1400);
    /// Unanalysed files by directory, with track type 2 in the descriptor.
    pub const MENU_FOLDER: Self = Self(0x2006);

    /// A track's thirteen metadata items (§5.9).
    pub const GET_METADATA: Self = Self(0x2002);
    /// Cover art by artwork id. Answered with an **omitted** blob when the
    /// track has none.
    pub const GET_ARTWORK: Self = Self(0x2003);
    /// The preview waveform. Carries the track id at argument **2**, not 1 like
    /// its siblings.
    pub const GET_WAVEFORM_PREVIEW: Self = Self(0x2004);
    /// A track's six track-info items (§5.10) — path, size, container.
    pub const GET_TRACK_INFO: Self = Self(0x2102);
    /// Memory points and hot cues.
    pub const GET_CUE_POINTS: Self = Self(0x2104);
    /// Metadata for an unanalysed file.
    pub const GET_GENERIC_METADATA: Self = Self(0x2202);
    /// The beat grid.
    pub const GET_BEAT_GRID: Self = Self(0x2204);
    /// Undocumented, and **the gate on playback**: the MP3 variable-bitrate
    /// seek index. Without a time-to-byte-offset table a player cannot seek, so
    /// it never issues a single READ — a load that resolves the path perfectly
    /// and then does nothing.
    pub const GET_VBR_INDEX: Self = Self(0x2504);
    /// "Describe the medium in this slot."
    ///
    /// Sent during a load, on the binary descriptor. **Answered with
    /// [`Self::MEDIA_INFO`] carrying a 148-byte body, not with a bare
    /// `SUCCESS`** — *a new observation, not in the research record, which
    /// lists `0x3903` among the undecoded types seen around a loaded track.*
    ///
    /// Answering it as an unknown request costs the whole browse session: a
    /// deck that gets `SUCCESS` here loses its menus, drops the track title
    /// back to the medium's own name, and stops drawing the scrolling
    /// waveform, until the DJ leaves LINK and comes back. See
    /// [`MediaInfo`] for the body and for how it was decoded.
    pub const GET_MEDIA_INFO: Self = Self(0x3903);
    /// Sent about a minute after a load, and **answered with nothing at all**.
    ///
    /// *A new observation, not in the research record, which lists `0x3001`
    /// among the undecoded types seen around a loaded track.* Across three
    /// deck-to-deck captures a real player sends this six times and the serving
    /// deck replies to it **not once** — the same contract as
    /// [`Self::MENU_CLOSE`].
    ///
    /// **Answering it costs the session.** A reply nobody asked for is read as
    /// the answer to the *next* request, which is the `GET_METADATA` the deck
    /// sends immediately afterwards; that comes back as "zero items" and every
    /// reply from then on is one behind. On the deck: menus blank, the track
    /// title falls back to the medium's own name, the waveform stops drawing —
    /// about a minute into any track, because that is when this is sent. The
    /// byte stream stays perfectly framed throughout, which is why nothing that
    /// checks framing finds it.
    pub const UNKNOWN_3001: Self = Self(0x3001);
    /// The detailed waveform.
    pub const GET_WAVEFORM_DETAIL: Self = Self(0x2904);
    /// Extended cue points, CDJ-2000NXS2 and later.
    pub const GET_CUE_POINTS_EXT: Self = Self(0x2b04);
    /// A raw analysis tag.
    pub const GET_ANALYSIS_TAG: Self = Self(0x2c04);

    /// Page through the result set a menu request established:
    /// `[descriptor, offset, limit, 0, total, 0]`.
    pub const RENDER_MENU: Self = Self(0x3000);
    /// "Put this track in the tag list": `[descriptor, track id, 1]`.
    ///
    /// Answered with a bare `SUCCESS` carrying zero. Nine of these appear in
    /// the tagging session, one per track tagged, from both decks; argument 2
    /// is `1` in every one, so **removal has never been observed** and `0` is
    /// this library's guess at it rather than a reading (F53).
    ///
    /// The descriptor's requesting-device byte is what the list is keyed on:
    /// two decks tagging from the same medium keep separate lists, which is
    /// what the TAG LIST button means on each of them.
    pub const TAG_LIST_ADD: Self = Self(0x3002);
    /// "REMOVE ALL TRACKS": empty the tag list. Only the descriptor.
    ///
    /// Isolated by the item counts either side of it, which is the only thing
    /// that separates it from [`Self::UNKNOWN_3402`] — both carry a bare
    /// descriptor and both draw `SUCCESS[type, 0]`. With three tracks tagged,
    /// `MENU_TAG_LIST` answered `3` before this request and `0` immediately
    /// after, while `0x3402` left the count at `3` (F54).
    pub const TAG_LIST_CLEAR: Self = Self(0x3202);
    /// Undocumented, a bare descriptor, sent while the TAG LIST menu is open.
    ///
    /// **Not** the clear, though it looks exactly like it: the tag list still
    /// held three tracks across it (F54). Acknowledged and otherwise ignored.
    pub const UNKNOWN_3402: Self = Self(0x3402);
    /// Undocumented. Sent mid-load, between `GET_TRACK_INFO` and the analysis
    /// fetches; four arguments, `[descriptor, n, 0, 0]`. A real deck answers
    /// with a bare `SUCCESS` echoing the type, and so must we.
    pub const UNKNOWN_3100: Self = Self(0x3100);
    /// Undocumented, two arguments `(descriptor, track id)`, sent once during
    /// playback and only ever to a **foreign** device. **No capture shows a
    /// real reply**, so acknowledging it the way `0x3100` is acknowledged is a
    /// guess — justified only by F25, where erroring on an unknown request
    /// stopped browsing dead.
    pub const UNKNOWN_3D03: Self = Self(0x3d03);
    /// Undocumented, one argument (the descriptor), sent immediately after
    /// `INTRODUCE`. Answer it with [`MessageKind::UNKNOWN_4B02`]; answering
    /// with an error is F25.
    ///
    /// **Not a foreign-device probe**, though it was recorded here as one. A
    /// sweep of every dbserver connection in the corpus that begins with
    /// `INTRODUCE` finds it sent deck-to-deck in four sessions — S13, S25, S26
    /// and S4b — and absent from two others between the same two decks. It
    /// tracks something about the session, not about who is being asked, and
    /// the earlier reading came from a sample that happened to split the same
    /// way (F56).
    pub const UNKNOWN_3E03: Self = Self(0x3e03);

    /// "Understood": `[request type, count]`, or `[0, our device number]` in
    /// answer to `INTRODUCE`.
    pub const SUCCESS: Self = Self(0x4000);
    /// Opens a render: `[1, 0]`.
    pub const MENU_HEADER: Self = Self(0x4001);
    /// Cover art: `[0x2003, 0, byte length, image]`.
    pub const ARTWORK: Self = Self(0x4002);
    /// A refusal. **Never send this in reply to a request you do not
    /// recognise** (F25).
    pub const ERROR: Self = Self(0x4003);
    /// One row of a menu: always twelve arguments. See [`MenuItem`].
    pub const MENU_ITEM: Self = Self(0x4101);
    /// Closes a render. No arguments.
    pub const MENU_FOOTER: Self = Self(0x4201);
    /// The preview waveform.
    pub const WAVEFORM_PREVIEW: Self = Self(0x4402);
    /// The MP3 variable-bitrate seek index.
    pub const VBR_INDEX: Self = Self(0x4502);
    /// The answer to [`Self::GET_MEDIA_INFO`]: a medium's description.
    pub const MEDIA_INFO: Self = Self(0x4902);
    /// The beat grid.
    pub const BEAT_GRID: Self = Self(0x4602);
    /// Memory points and hot cues.
    pub const CUE_POINTS: Self = Self(0x4702);
    /// The detailed waveform.
    pub const WAVEFORM_DETAIL: Self = Self(0x4a02);
    /// The reply to [`MessageKind::UNKNOWN_3E03`], observed as
    /// `[0x3e03, 0, <server device number>, ""]`. Meaning unknown; the empty
    /// string looks like somewhere a name belongs.
    pub const UNKNOWN_4B02: Self = Self(0x4b02);
    /// Extended cue points.
    pub const CUE_POINTS_EXT: Self = Self(0x4e02);
    /// A raw analysis tag.
    pub const ANALYSIS_TAG: Self = Self(0x4f02);

    /// A name for logs, or `None` for a type we have never observed.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::INTRODUCE => "introduce",
            Self::MENU_CLOSE => "menu_close",
            Self::DISCONNECT => "disconnect",
            Self::MENU_ROOT => "menu_root",
            Self::MENU_GENRE => "menu_genre",
            Self::MENU_ARTIST => "menu_artist",
            Self::MENU_ALBUM => "menu_album",
            Self::MENU_TRACK => "menu_track",
            Self::MENU_BPM => "menu_bpm",
            Self::MENU_RATING => "menu_rating",
            Self::MENU_YEAR => "menu_year",
            Self::MENU_LABEL => "menu_label",
            Self::MENU_COLOR => "menu_color",
            Self::MENU_TIME => "menu_time",
            Self::MENU_BITRATE => "menu_bitrate",
            Self::MENU_HISTORY => "menu_history",
            Self::MENU_FILENAME => "menu_filename",
            Self::MENU_KEY => "menu_key",
            Self::MENU_TAG_LIST => "menu_tag_list",
            Self::TAG_LIST_ADD => "tag_list_add",
            Self::TAG_LIST_CLEAR => "tag_list_clear",
            Self::MENU_PLAYLIST => "menu_playlist",
            Self::MENU_SEARCH => "menu_search",
            Self::MENU_SORT => "menu_sort",
            Self::MENU_FOLDER => "menu_folder",
            Self::GET_METADATA => "get_metadata",
            Self::GET_ARTWORK => "get_artwork",
            Self::GET_WAVEFORM_PREVIEW => "get_waveform_preview",
            Self::GET_TRACK_INFO => "get_track_info",
            Self::GET_CUE_POINTS => "get_cue_points",
            Self::GET_GENERIC_METADATA => "get_generic_metadata",
            Self::GET_BEAT_GRID => "get_beat_grid",
            Self::GET_VBR_INDEX => "get_vbr_index",
            Self::GET_MEDIA_INFO => "get_media_info",
            Self::UNKNOWN_3001 => "unknown_3001",
            Self::MEDIA_INFO => "media_info",
            Self::GET_WAVEFORM_DETAIL => "get_waveform_detail",
            Self::GET_CUE_POINTS_EXT => "get_cue_points_ext",
            Self::GET_ANALYSIS_TAG => "get_analysis_tag",
            Self::RENDER_MENU => "render_menu",
            Self::UNKNOWN_3100 => "unknown_3100",
            Self::UNKNOWN_3D03 => "unknown_3d03",
            Self::UNKNOWN_3E03 => "unknown_3e03",
            Self::SUCCESS => "success",
            Self::MENU_HEADER => "menu_header",
            Self::ARTWORK => "artwork",
            Self::ERROR => "error",
            Self::MENU_ITEM => "menu_item",
            Self::MENU_FOOTER => "menu_footer",
            Self::WAVEFORM_PREVIEW => "waveform_preview",
            Self::VBR_INDEX => "vbr_index",
            Self::BEAT_GRID => "beat_grid",
            Self::CUE_POINTS => "cue_points",
            Self::WAVEFORM_DETAIL => "waveform_detail",
            Self::UNKNOWN_4B02 => "unknown_4b02",
            Self::CUE_POINTS_EXT => "cue_points_ext",
            Self::ANALYSIS_TAG => "analysis_tag",
            _ => return None,
        })
    }

    /// Whether a peer expects no reply at all to this request.
    ///
    /// True only for [`MessageKind::MENU_CLOSE`]. Sending anything back — least
    /// of all the `0x4003` an unhandled type would otherwise produce — risks
    /// desynchronising a client that is not listening for one (F16).
    pub fn expects_no_reply(self) -> bool {
        self == Self::MENU_CLOSE
    }
}

impl fmt::Debug for MessageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "MessageKind({:#06x})", self.0),
        }
    }
}

// -- the drill-down grid --------------------------------------------------

/// A drill-down request: `depth` levels into `category`.
///
/// One systematic message type addresses every drill-down —
/// `0x1000 | depth << 8 | category`, where *category* is the **menu request**
/// type's low byte. All thirteen types seen in one exhaustive session are
/// generated by that formula (F42), so the three named messages the
/// pre-hardware literature gives — `ARTISTS_FOR_GENRE`, `ALBUMS_FOR_ARTIST`,
/// `TRACKS_FOR_ALBUM` — are three points in a grid rather than three messages.
/// Implementing it as a grid is what makes LABEL, BITRATE, HISTORY and KEY work
/// at all; before it they showed as categories that existed and were empty.
///
/// Arguments are `[descriptor, sort, filter…]`, one filter id per level, and a
/// filter of [`FILTER_ALL`] means "do not narrow here". The chains differ by
/// category: GENRE narrows to an artist, then an album, then tracks; ARTIST
/// skips straight to albums; ALBUM straight to tracks; KEY has an extra level
/// no other category has, a harmonic tolerance (F44).
///
/// **The category byte is the request numbering, not the root-item id
/// numbering**: KEY is `0x14` here and `0x0c` in [`ROOT_CATEGORIES`]. Two
/// schemes that disagree, coexisting in one protocol, is exactly how F40's bug
/// happened — a deck opening "KEY" asked for bitrates.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Drill {
    /// How many levels in. 1 is the first narrowing; 0 is the flat menu and is
    /// not a drill.
    pub depth: u8,
    /// The low byte of the flat menu's request type.
    pub category: u8,
}

impl Drill {
    /// The message type this drill is addressed with.
    pub fn kind(self) -> MessageKind {
        drill_kind(self.depth, self.category)
    }

    /// Read a drill out of a message type, or `None` if it is not one.
    pub fn parse(kind: MessageKind) -> Option<Self> {
        let raw = kind.0;
        if raw & 0xf000 != 0x1000 {
            return None;
        }
        let depth = u8::try_from((raw >> 8) & 0x0f).ok()?;
        if depth == 0 {
            return None;
        }
        Some(Self {
            depth,
            category: u8::try_from(raw & 0xff).ok()?,
        })
    }
}

/// The request type for drilling `depth` levels into `category` (F42).
pub fn drill_kind(depth: u8, category: u8) -> MessageKind {
    MessageKind(0x1000 | (u16::from(depth) << 8) | u16::from(category))
}

/// The filter id meaning "do not narrow at this level", and the id the `ALL`
/// row carries.
pub const FILTER_ALL: u32 = 0xFFFF_FFFF;

// -- the descriptor -------------------------------------------------------

/// Byte `M` of a descriptor: where the answer is destined for.
///
/// A newtype rather than an enum — a value we have not seen would otherwise
/// take out a request we could still serve.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MenuTarget(pub u8);

impl MenuTarget {
    /// The list the deck is scrolling.
    pub const MAIN: Self = Self(0x01);
    /// A transient menu dipped into — metadata for a highlighted track.
    pub const SUB: Self = Self(0x02);
    /// The preview pane.
    pub const PREVIEW: Self = Self(0x03);
    /// Binary loads: artwork, waveforms, beat grids, cues.
    pub const BINARY: Self = Self(0x08);
}

impl fmt::Debug for MenuTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::MAIN => f.write_str("main"),
            Self::SUB => f.write_str("sub"),
            Self::PREVIEW => f.write_str("preview"),
            Self::BINARY => f.write_str("binary"),
            Self(raw) => write!(f, "MenuTarget({raw:#04x})"),
        }
    }
}

/// Byte `T` of a descriptor: what kind of thing is being browsed.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackType(pub u8);

impl TrackType {
    /// A rekordbox-analysed track.
    pub const REKORDBOX: Self = Self(1);
    /// An unanalysed file, browsed by folder.
    pub const UNANALYSED: Self = Self(2);
    /// A track on an audio CD.
    pub const CD_AUDIO: Self = Self(5);
    /// A streamed track.
    pub const STREAMING: Self = Self(6);
}

impl fmt::Debug for TrackType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::REKORDBOX => f.write_str("rekordbox"),
            Self::UNANALYSED => f.write_str("unanalysed"),
            Self::CD_AUDIO => f.write_str("cd_audio"),
            Self::STREAMING => f.write_str("streaming"),
            Self(raw) => write!(f, "TrackType({raw:#04x})"),
        }
    }
}

/// Argument 0 of nearly every request: `D << 24 | M << 16 | Sr << 8 | Tr`.
///
/// Parsed into fields rather than passed around raw, because two of the four
/// bytes decide which library answers. **The slot byte is the discriminator
/// when one connection carries two media** (F37): a player browsing both a
/// deck's SD and its USB opens exactly one dbserver connection and tells them
/// apart by this byte alone, so a server resolves the medium *per message*
/// rather than per connection.
///
/// Slots are numbered as the status packets number them — [`Slot::SD`] is 2 and
/// [`Slot::USB`] is 3.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Descriptor {
    /// `D` — the **requesting** device's number, not the server's.
    pub device: DeviceNumber,
    /// `M` — where the answer is shown.
    pub menu: MenuTarget,
    /// `Sr` — which of the server's slots is being browsed.
    pub slot: Slot,
    /// `Tr` — rekordbox track, unanalysed file, CD audio.
    pub track_type: TrackType,
}

impl Descriptor {
    /// Build the descriptor a client puts in its requests.
    ///
    /// Takes a [`BrowsableDeviceNumber`] because a device outside 1–4 is never
    /// offered as a browse source and never gets this far (F45); the number an
    /// observer takes cannot be borrowed for a dbserver session, and making
    /// that a type rather than a comment means the mistake cannot be written.
    pub fn new(
        device: BrowsableDeviceNumber,
        slot: Slot,
        menu: MenuTarget,
        track_type: TrackType,
    ) -> Self {
        Self {
            device: device.number(),
            menu,
            slot,
            track_type,
        }
    }

    /// Read a descriptor off the wire.
    ///
    /// Permissive where [`Descriptor::new`] is strict: a request from a device
    /// numbered 5 is still a request, and rejecting it would lose a message we
    /// could answer. Only device 0 is refused, since no reply could be
    /// addressed to it.
    pub fn parse(raw: u32) -> Option<Self> {
        let [device, menu, slot, track_type] = raw.to_be_bytes();
        Some(Self {
            device: DeviceNumber::new(device)?,
            menu: MenuTarget(menu),
            slot: Slot(slot),
            track_type: TrackType(track_type),
        })
    }

    /// The packed `UInt32`.
    pub fn to_raw(self) -> u32 {
        u32::from_be_bytes([
            self.device.get(),
            self.menu.0,
            self.slot.0,
            self.track_type.0,
        ])
    }

    /// The same descriptor aimed at a different part of the display.
    #[must_use]
    pub fn with_menu(mut self, menu: MenuTarget) -> Self {
        self.menu = menu;
        self
    }
}

impl From<Descriptor> for Field {
    fn from(descriptor: Descriptor) -> Self {
        Self::U32(descriptor.to_raw())
    }
}

// -- messages -------------------------------------------------------------

/// One dbserver message: a five-field header and up to twelve arguments.
///
/// The argument count and the twelve-byte tag blob are **not** fields: both are
/// functions of [`Message::args`], derived on the way out. A message whose
/// header contradicts its arguments cannot be built.
#[derive(Clone, PartialEq, Eq)]
pub struct Message {
    /// Echoed in the reply, and the only way to pair one with its request.
    /// [`SETUP_TRANSACTION_ID`] for `INTRODUCE` and `DISCONNECT`, counting up
    /// from [`FIRST_TRANSACTION_ID`] for everything else (C10).
    pub transaction_id: u32,
    /// What this message is.
    pub kind: MessageKind,
    /// Its arguments.
    pub args: Arguments,
}

impl Message {
    /// Build a message.
    pub fn new(transaction_id: u32, kind: MessageKind, args: impl Into<Arguments>) -> Self {
        Self {
            transaction_id,
            kind,
            args: args.into(),
        }
    }

    /// Decode one message, returning it and the bytes it consumed.
    ///
    /// The count is what a stream reader needs: messages carry no length, so
    /// the parser's final position *is* the frame boundary.
    ///
    /// Fails with [`Error::Truncated`] when the buffer ends inside the message —
    /// the normal outcome of trying too early on a TCP stream, and a signal to
    /// wait for more bytes rather than to give up. Anything else means the peer
    /// is not speaking this protocol, and since there is no frame boundary to
    /// resynchronise on, the only remedy is to drop the connection.
    pub fn decode(data: &[u8]) -> Result<(Self, usize)> {
        let mut reader = Reader::new(data);
        reader.magic()?;
        let transaction_id = reader.tagged_u32("transaction id")?;
        let kind = MessageKind(reader.tagged_u16("message type")?);
        let count = usize::from(reader.tagged_u8("argument count")?);
        let tags = reader.arg_tag_blob()?;

        if count > Arguments::MAX {
            return Err(Error::malformed(
                14,
                format!("argument count {count} exceeds the twelve the tag blob describes"),
            ));
        }

        let mut fields = Vec::with_capacity(count);
        let mut previous = 1u32;
        for index in 0..count {
            let arg_tag = ArgTag(tags.get(index).copied().unwrap_or(0));
            let field = if arg_tag == ArgTag::BLOB && previous == 0 {
                // Absent from the wire entirely. Nothing to read: the zero we
                // just passed is the whole of the evidence.
                Field::Blob(Vec::new())
            } else {
                reader.field(arg_tag, index)?
            };
            previous = field.presence_value();
            fields.push(field);
        }

        let args = Arguments::new(fields).unwrap_or_default();
        let consumed = reader.position();
        Ok((
            Self {
                transaction_id,
                kind,
                args,
            },
            consumed,
        ))
    }

    /// Encode this message.
    ///
    /// The inverse of [`Message::decode`] in the strong sense: an argument is
    /// omitted if and only if a decoder would infer its absence, so
    /// decode-then-encode reproduces a captured message byte for byte and
    /// encode-then-decode reproduces the message.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + 8 * self.args.len());
        Field::U32(MAGIC).write(&mut out);
        Field::U32(self.transaction_id).write(&mut out);
        Field::U16(self.kind.0).write(&mut out);
        let count = u8::try_from(self.args.len()).unwrap_or(0);
        Field::U8(count).write(&mut out);

        let mut tags = [0u8; ARG_TAG_BLOB_LEN];
        for (slot, field) in tags.iter_mut().zip(self.args.as_slice()) {
            *slot = field.arg_tag().0;
        }
        Field::Blob(tags.to_vec()).write(&mut out);

        let mut previous = 1u32;
        for field in self.args.as_slice() {
            let omitted = previous == 0 && matches!(field, Field::Blob(blob) if blob.is_empty());
            if !omitted {
                field.write(&mut out);
            }
            previous = field.presence_value();
        }
        out
    }

    /// Argument `index` as an integer.
    pub fn number(&self, index: usize) -> Option<u32> {
        self.args.number(index)
    }

    /// Argument `index` as text.
    pub fn text(&self, index: usize) -> Option<&str> {
        self.args.text(index)
    }

    /// Argument `index` as bytes.
    pub fn blob(&self, index: usize) -> Option<&[u8]> {
        self.args.blob(index)
    }

    /// Argument 0 read as a descriptor, which is what it is on nearly every
    /// request.
    pub fn descriptor(&self) -> Option<Descriptor> {
        self.number(0).and_then(Descriptor::parse)
    }

    // -- request builders --

    /// `INTRODUCE`: "I am device N."
    pub fn introduce(device: BrowsableDeviceNumber) -> Self {
        Self::new(
            SETUP_TRANSACTION_ID,
            MessageKind::INTRODUCE,
            [Field::U32(u32::from(device.get()))],
        )
    }

    /// `DISCONNECT`.
    pub fn disconnect() -> Self {
        Self::new(SETUP_TRANSACTION_ID, MessageKind::DISCONNECT, [])
    }

    /// `RENDER_MENU`: page `limit` rows from `offset` of a `limit`-row result.
    pub fn render(transaction_id: u32, descriptor: Descriptor, offset: u32, limit: u32) -> Self {
        Self::render_of(transaction_id, descriptor, offset, limit, limit)
    }

    /// `RENDER_MENU` with an explicit result-set size.
    ///
    /// The follow-up to every menu request. `total` echoes the size the menu
    /// request was answered with, and together with the descriptor it names
    /// *which* pending result set is being paged: a deck interleaves a metadata
    /// lookup with a track list and then resumes the list at the next offset
    /// without re-issuing its request, so a server must key its result sets on
    /// `(descriptor, count)` and hold several at once. Keying on the count
    /// alone worked until metadata became thirteen items and collided with a
    /// thirteen-track album (F27, F41).
    pub fn render_of(
        transaction_id: u32,
        descriptor: Descriptor,
        offset: u32,
        limit: u32,
        total: u32,
    ) -> Self {
        Self::new(
            transaction_id,
            MessageKind::RENDER_MENU,
            [
                descriptor.into(),
                Field::U32(offset),
                Field::U32(limit),
                Field::U32(0),
                Field::U32(total),
                Field::U32(0),
            ],
        )
    }

    /// Any menu request: the descriptor, then the type-specific arguments.
    ///
    /// `None` when `extra` would push the message past twelve arguments.
    pub fn menu_request(
        transaction_id: u32,
        kind: MessageKind,
        descriptor: Descriptor,
        extra: &[u32],
    ) -> Option<Self> {
        let args = Arguments::new(
            std::iter::once(Field::from(descriptor)).chain(extra.iter().copied().map(Field::U32)),
        )?;
        Some(Self {
            transaction_id,
            kind,
            args,
        })
    }

    /// `MENU_SEARCH`: `[descriptor, sort, byte length, text, 0]`.
    ///
    /// **Argument 3 is the text** and argument 2 its UTF-16 size including the
    /// NUL; reading argument 2 as the term is why search once matched nothing
    /// (F44). A deck searches as you type, one request per keystroke.
    pub fn search(
        transaction_id: u32,
        descriptor: Descriptor,
        sort: SortOrder,
        term: &str,
    ) -> Self {
        Self::new(
            transaction_id,
            MessageKind::MENU_SEARCH,
            [
                descriptor.into(),
                Field::U32(sort.0),
                Field::U32(label_bytes(term)),
                Field::from(term),
                Field::U32(0),
            ],
        )
    }

    // -- reply builders --

    /// `SUCCESS`: "understood, and the result has `count` rows."
    pub fn success(transaction_id: u32, request: MessageKind, count: u32) -> Self {
        Self::new(
            transaction_id,
            MessageKind::SUCCESS,
            [Field::U32(u32::from(request.0)), Field::U32(count)],
        )
    }

    /// The reply to an `INTRODUCE`.
    ///
    /// Argument 1 is **our own player number**, not an item count — the one
    /// `SUCCESS` whose second argument means something else (F7).
    pub fn introduce_reply(server: DeviceNumber) -> Self {
        Self::new(
            SETUP_TRANSACTION_ID,
            MessageKind::SUCCESS,
            [
                Field::U32(u32::from(MessageKind::INTRODUCE.0)),
                Field::U32(u32::from(server.get())),
            ],
        )
    }

    /// The reply to [`MessageKind::UNKNOWN_3E03`], modelled byte for byte on a
    /// real one between two players.
    ///
    /// The meaning of either message is unknown. What is known is the cost of
    /// getting it wrong: answering with [`MessageKind::ERROR`] made a deck
    /// render every one of our root categories and then disconnect without
    /// opening a single one (F25).
    pub fn unknown_3e03_reply(transaction_id: u32, server: DeviceNumber) -> Self {
        Self::new(
            transaction_id,
            MessageKind::UNKNOWN_4B02,
            [
                Field::U32(u32::from(MessageKind::UNKNOWN_3E03.0)),
                Field::U32(0),
                Field::U32(u32::from(server.get())),
                Field::from(""),
            ],
        )
    }

    /// Opens a render: `MENU_HEADER [1, 0]`.
    pub fn menu_header(transaction_id: u32) -> Self {
        Self::new(
            transaction_id,
            MessageKind::MENU_HEADER,
            [Field::U32(1), Field::U32(0)],
        )
    }

    /// Closes a render. No arguments.
    pub fn menu_footer(transaction_id: u32) -> Self {
        Self::new(transaction_id, MessageKind::MENU_FOOTER, [])
    }

    /// The envelope every binary reply shares:
    /// `[request type, 0, byte length, blob, trailing…]`.
    ///
    /// **Argument 0 echoes the request's message type**, not the track id.
    ///
    /// An empty payload needs no special case: argument 2 goes out as zero and
    /// the blob disappears, which is exactly what a player sends for a track
    /// with no artwork. "No data" and "here is the data" are one shape.
    ///
    /// `None` when `trailing` would push the message past twelve arguments.
    pub fn binary_reply(
        transaction_id: u32,
        kind: MessageKind,
        request: MessageKind,
        payload: Vec<u8>,
        trailing: &[u32],
    ) -> Option<Self> {
        let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
        let args = Arguments::new(
            [
                Field::U32(u32::from(request.0)),
                Field::U32(0),
                Field::U32(length),
                Field::Blob(payload),
            ]
            .into_iter()
            .chain(trailing.iter().copied().map(Field::U32)),
        )?;
        Some(Self {
            transaction_id,
            kind,
            args,
        })
    }
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}(tx={:#x}) {:?}",
            self.kind, self.transaction_id, self.args
        )
    }
}

// -- the cursor -----------------------------------------------------------

/// A bounds-checked forward cursor whose short reads are [`Error::Truncated`].
///
/// Every failure it can produce is either "the buffer ended" — which on a TCP
/// stream means wait — or a length no real message carries. Nothing in it can
/// panic.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn position(&self) -> usize {
        self.pos
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).ok_or(Error::Truncated {
            need: len,
            at: self.pos,
            have: 0,
        })?;
        let slice = self.data.get(self.pos..end).ok_or(Error::Truncated {
            need: len,
            at: self.pos,
            have: self.data.len().saturating_sub(self.pos),
        })?;
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes: [u8; 2] = self.take(2)?.try_into().unwrap_or([0; 2]);
        Ok(u16::from_be_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes: [u8; 4] = self.take(4)?.try_into().unwrap_or([0; 4]);
        Ok(u32::from_be_bytes(bytes))
    }

    /// The magic, which is itself a tagged `UInt32`.
    ///
    /// A wrong first byte is reported before reading further: more bytes cannot
    /// turn a `0x99` into a `0x11`, so this is malformed rather than truncated
    /// and the caller must not sit waiting on it.
    fn magic(&mut self) -> Result<()> {
        let mut wanted = Vec::with_capacity(5);
        Field::U32(MAGIC).write(&mut wanted);
        if let Some(&first) = self.data.first()
            && first != FieldTag::U32.0
        {
            return Err(Error::BadMagic {
                expected: wanted.as_slice().into(),
                got: self.data.get(..1).unwrap_or_default().into(),
            });
        }
        let got = self.take(wanted.len())?;
        if got == wanted.as_slice() {
            Ok(())
        } else {
            Err(Error::BadMagic {
                expected: wanted.as_slice().into(),
                got: got.into(),
            })
        }
    }

    /// A header field whose tag the format fixes.
    fn expect_tag(&mut self, expected: FieldTag, what: &'static str) -> Result<()> {
        let at = self.pos;
        let tag = FieldTag(self.u8()?);
        if tag == expected {
            Ok(())
        } else {
            Err(Error::malformed(
                at,
                format!("{what} should be tagged {expected:?}, got {tag:?}"),
            ))
        }
    }

    fn tagged_u8(&mut self, what: &'static str) -> Result<u8> {
        self.expect_tag(FieldTag::U8, what)?;
        self.u8()
    }

    fn tagged_u16(&mut self, what: &'static str) -> Result<u16> {
        self.expect_tag(FieldTag::U16, what)?;
        self.u16()
    }

    fn tagged_u32(&mut self, what: &'static str) -> Result<u32> {
        self.expect_tag(FieldTag::U32, what)?;
        self.u32()
    }

    /// The header's twelve-byte argument-tag blob.
    fn arg_tag_blob(&mut self) -> Result<&'a [u8]> {
        let at = self.pos;
        self.expect_tag(FieldTag::BLOB, "argument tags")?;
        let length = self.u32()?;
        if length != u32::try_from(ARG_TAG_BLOB_LEN).unwrap_or(u32::MAX) {
            return Err(Error::malformed(
                at,
                format!("argument-tag blob is {length} bytes, not {ARG_TAG_BLOB_LEN}"),
            ));
        }
        self.take(ARG_TAG_BLOB_LEN)
    }

    /// One argument, checked against the type the header claimed for it.
    ///
    /// The two numberings must agree. Decoding a value whose own tag
    /// contradicts the header would produce a message that re-encodes
    /// differently from the one that arrived, which is the property everything
    /// here exists to preserve.
    fn field(&mut self, arg_tag: ArgTag, index: usize) -> Result<Field> {
        let at = self.pos;
        let tag = FieldTag(self.u8()?);
        if let Some(expected) = arg_tag.field_tag()
            && expected != tag
        {
            return Err(Error::malformed(
                at,
                format!("argument {index} is tagged {tag:?} but the header calls it {arg_tag:?}"),
            ));
        }
        match tag {
            FieldTag::U8 => Ok(Field::U8(self.u8()?)),
            FieldTag::U16 => Ok(Field::U16(self.u16()?)),
            FieldTag::U32 => Ok(Field::U32(self.u32()?)),
            FieldTag::BLOB => {
                let length = self.u32()?;
                if length > MAX_BLOB_LEN {
                    return Err(Error::ImplausibleLength {
                        what: "dbserver binary argument",
                        length: u64::from(length),
                        limit: u64::from(MAX_BLOB_LEN),
                    });
                }
                let bytes = self.take(usize::try_from(length).unwrap_or(usize::MAX))?;
                Ok(Field::Blob(bytes.to_vec()))
            }
            FieldTag::TEXT => {
                let characters = self.u32()?;
                if characters > MAX_STRING_CHARS {
                    return Err(Error::ImplausibleLength {
                        what: "dbserver string argument",
                        length: u64::from(characters),
                        limit: u64::from(MAX_STRING_CHARS),
                    });
                }
                let bytes = self.take(usize::try_from(characters * 2).unwrap_or(usize::MAX))?;
                let (text, terminator) = decode_string_body(bytes);
                Ok(Field::Text { text, terminator })
            }
            FieldTag(raw) => Err(Error::malformed(
                at,
                format!("argument {index} has unknown field tag {raw:#04x}"),
            )),
        }
    }
}

// -- menu items -----------------------------------------------------------

/// A menu-item kind, argument 6 of a [`MenuItem`].
///
/// **The same byte means different things in different replies.** `0x04` is the
/// track title in a `GET_METADATA` reply and the *container* in a
/// `GET_TRACK_INFO` reply, where the label is empty and the id holds a
/// rekordbox file type. Reading the pre-hardware literature's item table as
/// global is what made "item 1 is the title" look already answered, and sent
/// the search that eventually found it to the wrong field (F35).
///
/// CDJ-3000s pack extra information into the two high bytes, so compare through
/// [`ItemType::masked`].
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ItemType(pub u32);

impl ItemType {
    /// A file path. In `GET_TRACK_INFO`, argument 0 is the **file size** (F31).
    pub const PATH: Self = Self(0x0000);
    /// A folder.
    pub const FOLDER: Self = Self(0x0001);
    /// An album.
    pub const ALBUM: Self = Self(0x0002);
    /// A disc.
    pub const DISC: Self = Self(0x0003);
    /// The track title — **or the container**, in a `GET_TRACK_INFO` reply.
    pub const TRACK_TITLE: Self = Self(0x0004);
    /// A genre.
    pub const GENRE: Self = Self(0x0006);
    /// An artist.
    pub const ARTIST: Self = Self(0x0007);
    /// A playlist.
    pub const PLAYLIST: Self = Self(0x0008);
    /// A star rating.
    pub const RATING: Self = Self(0x000a);
    /// A duration in seconds.
    pub const DURATION: Self = Self(0x000b);
    /// A tempo, ×100.
    pub const TEMPO: Self = Self(0x000d);
    /// A record label.
    pub const LABEL: Self = Self(0x000e);
    /// A musical key.
    pub const KEY: Self = Self(0x000f);
    /// A bitrate in kbps.
    pub const BITRATE: Self = Self(0x0010);
    /// A year.
    pub const YEAR: Self = Self(0x0011);
    /// A colour.
    pub const COLOR: Self = Self(0x0013);
    /// A comment.
    pub const COMMENT: Self = Self(0x0023);
    /// A history playlist.
    pub const HISTORY_PLAYLIST: Self = Self(0x0024);
    /// An original artist.
    pub const ORIGINAL_ARTIST: Self = Self(0x0028);
    /// A remixer.
    pub const REMIXER: Self = Self(0x0029);
    /// The DJ play count.
    pub const PLAY_COUNT: Self = Self(0x002a);
    /// The date a track was added.
    pub const DATE_ADDED: Self = Self(0x002e);
    /// The sixth item of a `GET_TRACK_INFO` reply. Constant `1` across MP3,
    /// AAC, WAV and AIFF in a real deck-to-deck load, so it is **not** the
    /// container — that is item 1. Meaning unknown (F35).
    pub const TRACK_INFO_UNKNOWN: Self = Self(0x002f);
    /// The `ALL` row that heads a filtered list.
    pub const ALL: Self = Self(0x00a0);
    /// The `DEFAULT` sort option.
    pub const SORT_DEFAULT: Self = Self(0x00a1);
    /// The `ALPHABET` sort option.
    pub const SORT_ALPHABET: Self = Self(0x00a2);
    /// The GENRE root category.
    pub const MENU_GENRE: Self = Self(0x0080);
    /// The ARTIST root category.
    pub const MENU_ARTIST: Self = Self(0x0081);
    /// The ALBUM root category.
    pub const MENU_ALBUM: Self = Self(0x0082);
    /// The TRACK root category.
    pub const MENU_TRACK: Self = Self(0x0083);
    /// The PLAYLIST root category.
    pub const MENU_PLAYLIST: Self = Self(0x0084);
    /// The BPM sort option.
    pub const MENU_BPM: Self = Self(0x0085);
    /// The RATING sort option.
    pub const MENU_RATING: Self = Self(0x0086);
    /// The LABEL root category.
    pub const MENU_LABEL: Self = Self(0x0089);
    /// The KEY root category.
    pub const MENU_KEY: Self = Self(0x008b);
    /// The DATE ADDED root category.
    pub const MENU_DATE_ADDED: Self = Self(0x008c);
    /// The FOLDER root category.
    pub const MENU_FOLDER: Self = Self(0x0090);
    /// The SEARCH root category.
    pub const MENU_SEARCH: Self = Self(0x0091);
    /// The BITRATE root category.
    pub const MENU_BITRATE: Self = Self(0x0093);
    /// The HISTORY root category.
    pub const MENU_HISTORY: Self = Self(0x0095);
    /// The DJ PLAY COUNT sort option.
    pub const MENU_PLAY_COUNT: Self = Self(0x0097);

    /// The two low bytes, which is what the type actually is.
    ///
    /// CDJ-3000s pack extra information into the high half; a comparison that
    /// forgets to mask silently stops matching on newer hardware.
    #[must_use]
    pub const fn masked(self) -> Self {
        Self(self.0 & 0xffff)
    }

    /// The item type of a track row whose second column shows this field.
    ///
    /// `(column field type << 8) | 0x04` (F43). So the familiar `0x0704` is not
    /// "title and artist" as the pre-hardware literature names it — it is *a
    /// track whose second column is the ARTIST field*, and `0x0d04` is the same
    /// row with a BPM column. That the sort selects the second column is the
    /// feature which makes sorting useful rather than cosmetic.
    #[must_use]
    pub const fn as_track_column(self) -> Self {
        Self((self.0 << 8) | 0x04)
    }
}

impl fmt::Debug for ItemType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ItemType({:#06x})", self.0)
    }
}

/// The interlinear annotation anchor that opens a category label.
pub const MENU_LABEL_PREFIX: char = '\u{fffa}';
/// The interlinear annotation terminator that closes one.
pub const MENU_LABEL_SUFFIX: char = '\u{fffb}';

/// Wrap a root-menu or sort-menu label the way real hardware does.
///
/// Real players send `\u{fffa}PLAYLIST\u{fffb}` — U+FFFA (interlinear
/// annotation anchor) and U+FFFB (terminator). Presumably a marker telling the
/// player "this is a known category, substitute your localised string".
///
/// **A bare label is not merely unlocalised: it is not openable.** The deck
/// renders it perfectly and then declines to open the category, which reads as
/// a category that exists and is empty (F26).
pub fn menu_label(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 6);
    out.push(MENU_LABEL_PREFIX);
    out.push_str(text);
    out.push(MENU_LABEL_SUFFIX);
    out
}

/// The text inside a [`menu_label`] wrapper, or `None` if it is not wrapped.
pub fn unwrap_menu_label(text: &str) -> Option<&str> {
    text.strip_prefix(MENU_LABEL_PREFIX)?
        .strip_suffix(MENU_LABEL_SUFFIX)
}

/// One row of a menu — the `0x4101` reply, with names instead of positions.
///
/// A menu item is always **twelve** arguments in a fixed order, and three of
/// them are derived rather than given: the two label byte lengths, and argument
/// 10. Assembling twelve positional arguments by hand is how the reference
/// implementation shipped a subtly wrong track row for months.
///
/// **Argument 10 tracks argument 7.** Across all 1 700 menu items in the
/// reference captures the two are never independent: an item carrying
/// `flags = 0x01000000` also carries `0x100` there, and an item with zero flags
/// carries zero. Both are non-zero only on the rows that name a track. Deriving
/// it removes the chance of setting one and forgetting the other (F32).
///
/// There is deliberately no `Default`: an all-zero row is a *path* item with no
/// label, which is a real and quite specific thing rather than a blank slate.
/// The constructors below name what they build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// Argument 0. Zero on an ordinary browse row, and **not** structural
    /// padding — which is exactly why what it does carry went unnoticed:
    ///
    /// - the **file size**, on the path item of a `GET_TRACK_INFO` reply. It is
    ///   the one thing a load needs that browsing does not, and without it a
    ///   deck renders the track, resolves its path over NFS, never reads a
    ///   byte, and reports that it cannot decode the format (F31);
    /// - the referenced row's id for a text sort column, or the raw number for
    ///   a numeric one — `0x3390` is 132.00 BPM, `0x140` is 320 kbps — which
    ///   the deck formats itself (F43);
    /// - the harmonic tolerance, on a KEY drill row (F44).
    pub argument0: u32,
    /// Argument 1 — the id of the row this item names.
    ///
    /// In a `GET_METADATA` reply this is the id of the row **referenced**, not
    /// the track's own: the artist item carries the artist's id, which is what
    /// lets a player offer "more by this artist". Putting the track id in all
    /// thirteen renders identically and means something else (F32).
    pub id: u32,
    /// Argument 3 — the row's main text.
    pub label1: String,
    /// Argument 5 — the second column, selected by the sort order (F43).
    /// Numeric columns send this **empty** and put the number in
    /// [`MenuItem::argument0`].
    pub label2: String,
    /// Argument 6.
    pub item_type: ItemType,
    /// Argument 7. `0x01000000` on rows that name a track, **zero on category
    /// and sort rows** — copying the track value onto a category is one of the
    /// three things that stopped a deck opening our root menu (F26).
    pub flags: u32,
    /// Argument 8. Set on the title item of a `GET_METADATA` reply; without it
    /// a player never requests the image and INFO shows no cover (F32).
    pub artwork_id: u32,
    /// Argument 9 — position within a playlist.
    pub playlist_position: u32,
    /// Argument 11 — the row's key, as a **Camelot wheel index**, and what the
    /// deck draws its key-matching indicator from.
    ///
    /// *A new observation, not in the research record, which had this argument
    /// down as a constant zero.* Decoded by correlating all 1265 track rows a
    /// real deck served in `S27-sort-by-and-key` against those tracks' keys in
    /// the medium's own `export.pdb`. Every row agreed:
    ///
    /// ```text
    /// 1A→1  1B→2  2A→3  2B→4  …  8B→16  …  10A→19  11A→21  12A→23  12B→24
    /// ```
    ///
    /// so the value is `2 × position − 1` for the A ring and `2 × position` for
    /// the B ring — the wheel walked round in order, minor before major. Build
    /// one with [`CamelotKey`].
    ///
    /// **The comparison is the deck's, not ours.** It knows what is playing and
    /// we do not; all a server has to do is say which key each row is in.
    /// Sending zero — which is what we did — is what leaves the indicator dark
    /// beside every track.
    pub key_index: u32,
}

/// A key as the Camelot wheel numbers it: a position of 1–12 and a ring.
///
/// Exists because the wire wants one number and a library holds a name, and the
/// mapping between them is a fact about the protocol rather than about the
/// library. Parsing accepts Camelot notation (`8A`, `12B`) and the classical
/// names rekordbox writes when the DJ has that preference set (`Am`, `F#`), so
/// the indicator works whichever notation a medium uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CamelotKey {
    position: u8,
    minor: bool,
}

impl CamelotKey {
    /// Build from a wheel position (1–12) and ring, or `None` if out of range.
    pub const fn new(position: u8, minor: bool) -> Option<Self> {
        if position >= 1 && position <= 12 {
            Some(Self { position, minor })
        } else {
            None
        }
    }

    /// The wheel position, 1–12.
    pub const fn position(self) -> u8 {
        self.position
    }

    /// Whether this is the A ring (minor).
    pub const fn is_minor(self) -> bool {
        self.minor
    }

    /// The value argument 11 carries.
    #[expect(
        clippy::as_conversions,
        reason = "u8 to u32 is lossless, and `position` is 1..=12 by construction"
    )]
    pub const fn index(self) -> u32 {
        let ring = if self.minor { 1 } else { 2 };
        // `position` is 1..=12 by construction, so the wheel index is 1..=24
        // and nothing here can overflow.
        (self.position.saturating_sub(1) as u32).saturating_mul(2) + ring
    }

    /// Read a key name in either notation rekordbox writes.
    ///
    /// `None` for anything unrecognised, which is the honest answer: a track
    /// with no key, or a key in a notation we have never seen, has no indicator
    /// rather than a wrong one.
    pub fn parse(name: &str) -> Option<Self> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return None;
        }
        // Camelot: a position then a ring letter.
        let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            let rest = trimmed[digits.len()..].trim();
            let minor = match rest {
                "A" | "a" => true,
                "B" | "b" => false,
                _ => return Self::classical(trimmed),
            };
            return digits
                .parse()
                .ok()
                .and_then(|position| Self::new(position, minor));
        }
        Self::classical(trimmed)
    }

    /// The classical names, which rekordbox writes when the DJ prefers them.
    fn classical(name: &str) -> Option<Self> {
        let key = name.to_ascii_lowercase();
        let (position, minor) = match key.as_str() {
            "abm" | "g#m" => (1, true),
            "b" => (1, false),
            "ebm" | "d#m" => (2, true),
            "f#" | "gb" => (2, false),
            "bbm" | "a#m" => (3, true),
            "db" | "c#" => (3, false),
            "fm" => (4, true),
            "ab" | "g#" => (4, false),
            "cm" => (5, true),
            "eb" | "d#" => (5, false),
            "gm" => (6, true),
            "bb" | "a#" => (6, false),
            "dm" => (7, true),
            "f" => (7, false),
            "am" => (8, true),
            "c" => (8, false),
            "em" => (9, true),
            "g" => (9, false),
            "bm" => (10, true),
            "d" => (10, false),
            "f#m" | "gbm" => (11, true),
            "a" => (11, false),
            "dbm" | "c#m" => (12, true),
            "e" => (12, false),
            _ => return None,
        };
        Self::new(position, minor)
    }
}

impl MenuItem {
    /// The flags a row naming a track carries.
    pub const TRACK_FLAGS: u32 = 0x0100_0000;
    /// Bit 0 of [`MenuItem::flags`]: this track is in the requesting deck's tag
    /// list, and the row is drawn with the tag marker.
    ///
    /// Established by elimination across the corpus: the bit is set on 446 rows
    /// and all of them are in the one session where tracks were being tagged.
    /// Within a single reply the *same* musical key appears both set and clear,
    /// so it is not the key-matching indicator, and the set of tagged keys
    /// walks — `11A`, then `2A`, then `2A 7B`, then `7B` — exactly as tracks
    /// were tagged and untagged one at a time (F53).
    pub const TAGGED: u32 = 0x0000_0001;
    /// Bit 8 of [`MenuItem::flags`]: this row is the track the deck has loaded.
    ///
    /// Set on 48 rows across three sessions, never more than one per reply, and
    /// always on a track the sending deck's status packets show as loaded
    /// (F53). Not written by this library — we do not know what the browsing
    /// deck has loaded, and guessing would mark the wrong row.
    pub const LOADED: u32 = 0x0000_0100;
    /// What argument 10 carries when [`MenuItem::flags`] is non-zero.
    pub const TRACK_ARGUMENT_10: u32 = 0x0100;

    /// A track row: a title, a second column, and the artwork to fetch.
    pub fn track(id: u32, title: &str, column: &str, item_type: ItemType, artwork_id: u32) -> Self {
        Self {
            argument0: 0,
            id,
            label1: title.to_owned(),
            label2: column.to_owned(),
            item_type,
            flags: Self::TRACK_FLAGS,
            artwork_id,
            playlist_position: 0,
            key_index: 0,
        }
    }

    /// The same row with its key, so the deck can draw the match indicator.
    #[must_use]
    pub fn with_key(mut self, key: Option<CamelotKey>) -> Self {
        self.key_index = key.map_or(0, CamelotKey::index);
        self
    }

    /// A root-menu or sort-menu row: label wrapped, flags zero.
    pub fn category(id: u32, item_type: ItemType, label: &str) -> Self {
        Self {
            argument0: 0,
            id,
            label1: menu_label(label),
            label2: String::new(),
            item_type,
            flags: 0,
            artwork_id: 0,
            playlist_position: 0,
            key_index: 0,
        }
    }

    /// The `ALL` row that heads a filtered list.
    ///
    /// Sent **only when there is more than one entry** — a single-entry level
    /// goes out bare (F42). Choosing it sends [`FILTER_ALL`] as that level's
    /// filter, meaning "do not narrow here".
    pub fn all() -> Self {
        Self::category(FILTER_ALL, ItemType::ALL, "ALL")
    }

    /// A plain named row: an artist, an album, a genre.
    pub fn named(id: u32, item_type: ItemType, label: &str) -> Self {
        Self {
            argument0: 0,
            id,
            label1: label.to_owned(),
            label2: String::new(),
            item_type,
            flags: 0,
            artwork_id: 0,
            playlist_position: 0,
            key_index: 0,
        }
    }

    /// Argument 10, which is a function of the flags and never independent of
    /// them (F32).
    pub fn argument10(&self) -> u32 {
        if self.flags == 0 {
            0
        } else {
            Self::TRACK_ARGUMENT_10
        }
    }

    /// The twelve-argument `0x4101` message.
    pub fn to_message(&self, transaction_id: u32) -> Message {
        Message::new(
            transaction_id,
            MessageKind::MENU_ITEM,
            [
                Field::U32(self.argument0),
                Field::U32(self.id),
                Field::U32(label_bytes(&self.label1)),
                Field::from(self.label1.clone()),
                Field::U32(label_bytes(&self.label2)),
                Field::from(self.label2.clone()),
                Field::U32(self.item_type.0),
                Field::U32(self.flags),
                Field::U32(self.artwork_id),
                Field::U32(self.playlist_position),
                Field::U32(self.argument10()),
                Field::U32(self.key_index),
            ],
        )
    }

    /// Read a menu item out of a `0x4101`, or `None` for anything else.
    pub fn from_message(message: &Message) -> Option<Self> {
        if message.kind != MessageKind::MENU_ITEM {
            return None;
        }
        Some(Self {
            argument0: message.number(0)?,
            id: message.number(1)?,
            label1: message.text(3)?.to_owned(),
            label2: message.text(5)?.to_owned(),
            item_type: ItemType(message.number(6)?),
            flags: message.number(7)?,
            artwork_id: message.number(8).unwrap_or(0),
            playlist_position: message.number(9).unwrap_or(0),
            key_index: message.number(11).unwrap_or(0),
        })
    }
}

// -- the root menu --------------------------------------------------------

/// One entry of the root category list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootCategory {
    /// The id the row carries in argument 1, and what the deck sends back.
    pub id: u32,
    /// The item type in argument 6.
    pub item_type: ItemType,
    /// The label, before [`menu_label`] wraps it.
    pub label: &'static str,
}

impl RootCategory {
    /// This category as a menu row.
    pub fn to_item(self) -> MenuItem {
        MenuItem::category(self.id, self.item_type, self.label)
    }
}

/// All twelve root categories, **listed**, in the order a real player serves
/// them.
///
/// Read off a CDJ-2000NXS answering `MENU_ROOT` in `S20-browse-ground-truth`,
/// page by page across twelve renders.
///
/// **The id is not derived, and twice it looked as if it could be.** F26
/// computed it from the *menu request* type's low byte, which agrees for five
/// categories and gives KEY the id BITRATE uses — a deck opening our KEY
/// category dutifully asked for bitrates, got a refusal, and showed nothing.
/// F40 replaced that with `item type − 0x7f`, right for eleven of the twelve
/// and wrong for DATE ADDED, where the difference is `0x71`. Two derivations,
/// two exceptions. All twelve have now been observed, so there is nothing left
/// to derive (F43).
///
/// A server need not offer all twelve, but the choice is user-visible: an
/// unimplemented category is indistinguishable from an empty one on the deck's
/// screen, so advertising one you cannot answer is worse than omitting it
/// (F40). `FOLDER` is the usual omission — it browses unanalysed files with a
/// track-type-2 descriptor.
pub const ROOT_CATEGORIES: [RootCategory; 12] = [
    RootCategory {
        id: 0x05,
        item_type: ItemType::MENU_PLAYLIST,
        label: "PLAYLIST",
    },
    RootCategory {
        id: 0x03,
        item_type: ItemType::MENU_ALBUM,
        label: "ALBUM",
    },
    RootCategory {
        id: 0x01,
        item_type: ItemType::MENU_GENRE,
        label: "GENRE",
    },
    RootCategory {
        id: 0x0a,
        item_type: ItemType::MENU_LABEL,
        label: "LABEL",
    },
    RootCategory {
        id: 0x02,
        item_type: ItemType::MENU_ARTIST,
        label: "ARTIST",
    },
    RootCategory {
        id: 0x14,
        item_type: ItemType::MENU_BITRATE,
        label: "BITRATE",
    },
    RootCategory {
        id: 0x1b,
        item_type: ItemType::MENU_DATE_ADDED,
        label: "DATE ADDED",
    },
    RootCategory {
        id: 0x04,
        item_type: ItemType::MENU_TRACK,
        label: "TRACK",
    },
    RootCategory {
        id: 0x16,
        item_type: ItemType::MENU_HISTORY,
        label: "HISTORY",
    },
    RootCategory {
        id: 0x12,
        item_type: ItemType::MENU_SEARCH,
        label: "SEARCH",
    },
    RootCategory {
        id: 0x11,
        item_type: ItemType::MENU_FOLDER,
        label: "FOLDER",
    },
    RootCategory {
        id: 0x0c,
        item_type: ItemType::MENU_KEY,
        label: "KEY",
    },
];

// -- sorting --------------------------------------------------------------

/// Argument 1 of a track list, a playlist, a drill-down or a search.
///
/// Also selects the **second column** of every row it returns, which is why
/// `DEFAULT` is not simply "unsorted" (F43). Inside a playlist `DEFAULT` must
/// keep the curated order — that is what a playlist is for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SortOrder(pub u32);

impl SortOrder {
    /// The library's own order.
    pub const DEFAULT: Self = Self(0x00);
    /// By title. The deck labels this `ALPHABET`.
    pub const TITLE: Self = Self(0x01);
    /// By artist.
    pub const ARTIST: Self = Self(0x02);
    /// By album.
    pub const ALBUM: Self = Self(0x03);
    /// By tempo.
    pub const BPM: Self = Self(0x04);
    /// By star rating.
    pub const RATING: Self = Self(0x05);
    /// By genre.
    pub const GENRE: Self = Self(0x06);
    /// By comment.
    pub const COMMENT: Self = Self(0x07);
    /// By duration.
    pub const TIME: Self = Self(0x08);
    /// By remixer.
    pub const REMIXER: Self = Self(0x09);
    /// By record label.
    pub const LABEL: Self = Self(0x0a);
    /// By original artist.
    pub const ORIGINAL_ARTIST: Self = Self(0x0b);
    /// By musical key.
    pub const KEY: Self = Self(0x0c);
    /// By bitrate.
    pub const BITRATE: Self = Self(0x0d);
    /// By DJ play count.
    pub const PLAY_COUNT: Self = Self(0x10);
    /// By the date a track was added.
    pub const DATE_ADDED: Self = Self(0x11);

    /// The field this sort puts in a row's **second column**.
    ///
    /// `None` for an order no real SORT menu offers, in which case a server
    /// should fall back to [`SortOrder::DEFAULT`]'s column rather than invent
    /// one.
    pub fn column(self) -> Option<ItemType> {
        Some(match self {
            Self::DEFAULT | Self::TITLE | Self::ARTIST => ItemType::ARTIST,
            Self::ALBUM => ItemType::ALBUM,
            Self::BPM => ItemType::TEMPO,
            Self::RATING => ItemType::RATING,
            Self::GENRE => ItemType::GENRE,
            Self::LABEL => ItemType::LABEL,
            Self::KEY => ItemType::KEY,
            Self::BITRATE => ItemType::BITRATE,
            Self::PLAY_COUNT => ItemType::PLAY_COUNT,
            Self::DATE_ADDED => ItemType::DATE_ADDED,
            _ => return None,
        })
    }

    /// The item type a track row carries under this sort:
    /// `(column field type << 8) | 0x04` (F43).
    pub fn track_item_type(self) -> ItemType {
        self.column().unwrap_or(ItemType::ARTIST).as_track_column()
    }

    /// Whether this sort's second column is a number the deck formats itself.
    ///
    /// A numeric column sends an **empty** label and puts the raw value in
    /// [`MenuItem::argument0`]; a text column sends both the text and the
    /// referenced row's id (F43).
    pub fn column_is_numeric(self) -> bool {
        matches!(
            self,
            Self::BPM | Self::RATING | Self::BITRATE | Self::PLAY_COUNT
        )
    }
}

impl fmt::Debug for SortOrder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match SORT_MENU.iter().find(|entry| entry.sort.0 == self.0) {
            Some(entry) => f.write_str(entry.label),
            None => write!(f, "SortOrder({:#04x})", self.0),
        }
    }
}

/// One row of the SORT menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortOption {
    /// The id sent back in argument 1 of the next list request.
    pub sort: SortOrder,
    /// The item type this row carries.
    pub item_type: ItemType,
    /// The label, before [`menu_label`] wraps it.
    pub label: &'static str,
}

impl SortOption {
    /// This option as a menu row.
    pub fn to_item(self) -> MenuItem {
        MenuItem::category(self.sort.0, self.item_type, self.label)
    }
}

/// The twelve sort orders, in the order a real player lists them.
///
/// The answer to `MENU_SORT` (`0x1400`). Argument 2 of the request names the
/// menu being sorted and the reply is these twelve regardless — why the
/// argument exists is one of the known unknowns.
///
/// Read off a CDJ-2000NXS in `S20-browse-ground-truth`. The ids and item types
/// match F42's table exactly; the *order* is the deck's, which no table records.
pub const SORT_MENU: [SortOption; 12] = [
    SortOption {
        sort: SortOrder::DEFAULT,
        item_type: ItemType::SORT_DEFAULT,
        label: "DEFAULT",
    },
    SortOption {
        sort: SortOrder::TITLE,
        item_type: ItemType::SORT_ALPHABET,
        label: "ALPHABET",
    },
    SortOption {
        sort: SortOrder::ARTIST,
        item_type: ItemType::MENU_ARTIST,
        label: "ARTIST",
    },
    SortOption {
        sort: SortOrder::ALBUM,
        item_type: ItemType::MENU_ALBUM,
        label: "ALBUM",
    },
    SortOption {
        sort: SortOrder::BPM,
        item_type: ItemType::MENU_BPM,
        label: "BPM",
    },
    SortOption {
        sort: SortOrder::RATING,
        item_type: ItemType::MENU_RATING,
        label: "RATING",
    },
    SortOption {
        sort: SortOrder::KEY,
        item_type: ItemType::MENU_KEY,
        label: "KEY",
    },
    SortOption {
        sort: SortOrder::BITRATE,
        item_type: ItemType::MENU_BITRATE,
        label: "BITRATE",
    },
    SortOption {
        sort: SortOrder::PLAY_COUNT,
        item_type: ItemType::MENU_PLAY_COUNT,
        label: "DJ PLAY COUNT",
    },
    SortOption {
        sort: SortOrder::GENRE,
        item_type: ItemType::MENU_GENRE,
        label: "GENRE",
    },
    SortOption {
        sort: SortOrder::DATE_ADDED,
        item_type: ItemType::MENU_DATE_ADDED,
        label: "DATE ADDED",
    },
    SortOption {
        sort: SortOrder::LABEL,
        item_type: ItemType::MENU_LABEL,
        label: "LABEL",
    },
];

// -- metadata and track info ----------------------------------------------

/// One slot of a `GET_METADATA` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetadataSlot {
    /// The item type in argument 6.
    pub item_type: ItemType,
    /// Argument 0. `1` on eight of the thirteen and `0` on the other five. The
    /// split matches no rule anyone has been able to name — it is not "has a
    /// label", because comment has one and gets `0`, nor "has a browse menu",
    /// because tempo has one and gets `0` — so it is reproduced as observed
    /// rather than derived from a rule we would be inventing *(unknown)*.
    pub argument0: u32,
}

/// The thirteen items a `GET_METADATA` reply carries, in order (§5.9).
///
/// Thirteen, not nine. A player renders whatever it is given and looks entirely
/// correct with four of them missing — colour, date added, bitrate and label —
/// which is why the shortfall survived so long (F32).
///
/// Two more things a correct reply does that a plausible one does not:
///
/// - each item carries the id of the row it **references**, not the track's
///   own. The artist item carries the artist's id, which is what lets a player
///   offer "more by this artist";
/// - the **title item carries the artwork id**. Without it a player never
///   requests the image and INFO shows no cover.
///
/// Items are emitted unconditionally, including empty ones: a real deck sends
/// `label` with id 0 and no text rather than omitting it, and the count is what
/// the client pages against.
pub const METADATA_ITEMS: [MetadataSlot; 13] = [
    MetadataSlot {
        item_type: ItemType::TRACK_TITLE,
        argument0: 1,
    },
    MetadataSlot {
        item_type: ItemType::ARTIST,
        argument0: 1,
    },
    MetadataSlot {
        item_type: ItemType::ALBUM,
        argument0: 1,
    },
    MetadataSlot {
        item_type: ItemType::DURATION,
        argument0: 0,
    },
    MetadataSlot {
        item_type: ItemType::TEMPO,
        argument0: 0,
    },
    MetadataSlot {
        item_type: ItemType::COMMENT,
        argument0: 0,
    },
    MetadataSlot {
        item_type: ItemType::KEY,
        argument0: 1,
    },
    MetadataSlot {
        item_type: ItemType::RATING,
        argument0: 0,
    },
    MetadataSlot {
        item_type: ItemType::COLOR,
        argument0: 0,
    },
    MetadataSlot {
        item_type: ItemType::GENRE,
        argument0: 1,
    },
    MetadataSlot {
        item_type: ItemType::DATE_ADDED,
        argument0: 1,
    },
    MetadataSlot {
        item_type: ItemType::BITRATE,
        argument0: 1,
    },
    MetadataSlot {
        item_type: ItemType::LABEL,
        argument0: 1,
    },
];

/// The six items a `GET_TRACK_INFO` reply carries, in order (§5.10).
///
/// Six, not one. Returning only the path is enough to render a track and to
/// walk it over NFS, and **not enough to load it**: a deck sat at "NOW
/// LOADING…" and then reported that it could not decode the format, having
/// issued no READ of any kind — so the verdict came from this reply and nowhere
/// else (F31).
///
/// Two traps, and the reference implementation fell into both:
///
/// - **argument 0 of the path item is the file size.** It is zero on every
///   other menu item in every capture, which is exactly why it reads as
///   structural padding, and it is the one thing a load needs that browsing
///   does not (F31);
/// - **item 1 is the container, not the title.** `0x04` means the title in a
///   `GET_METADATA` reply and the container here, with an empty label and the
///   rekordbox file type in the id. Item 6 is a constant `1` across MP3, AAC,
///   WAV and AIFF. Two earlier readings had these the other way round and the
///   errors cancelled for the only format that had ever been captured; serving
///   the disc number in item 1 makes a disc-2 MP3 announce itself as AAC (F35).
pub const TRACK_INFO_ITEMS: [ItemType; 6] = [
    ItemType::TRACK_TITLE,
    ItemType::DURATION,
    ItemType::TEMPO,
    ItemType::COMMENT,
    ItemType::PATH,
    ItemType::TRACK_INFO_UNKNOWN,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a hex literal. Test-only: a bad literal is a panic, and the test
    /// is the thing that fails.
    fn hex(text: &str) -> Vec<u8> {
        assert!(
            text.len().is_multiple_of(2),
            "hex literal has an odd length"
        );
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("valid hex"))
            .collect()
    }

    fn device(number: u8) -> DeviceNumber {
        DeviceNumber::new(number).expect("a non-zero device number")
    }

    fn browsable(number: u8) -> BrowsableDeviceNumber {
        BrowsableDeviceNumber::new(number).expect("a browsable device number")
    }

    // -- captured messages ------------------------------------------------
    //
    // Every literal below was extracted from a pcap by reassembling the TCP
    // stream on port 1051 and slicing out one message. None was written by
    // hand, and each carries the capture it came from.

    /// `MENU_CLOSE`, from `captures/S05-link-browse`.
    ///
    /// A bare 32-byte message: zero arguments and an all-zero tag blob (F16).
    const MENU_CLOSE: &str = "11872349ae11038001b71000010f00140000000c000000000000000000000000";

    /// `INTRODUCE [1]`, from `captures/S05-link-browse`.
    const INTRODUCE: &str = concat!(
        "11872349ae11fffffffe1000000f01140000000c060000000000000000000000",
        "1100000001",
    );

    /// The `SUCCESS` answering it — `[0, 2]`, where 2 is the **server's** own
    /// player number rather than an item count (F7).
    const INTRODUCE_REPLY: &str = concat!(
        "11872349ae11fffffffe1040000f02140000000c060600000000000000000000",
        "11000000001100000002",
    );

    /// `MENU_ROOT [0x01010301, 0, 0xffffff]`, from `captures/S05-link-browse`.
    const MENU_ROOT: &str = concat!(
        "11872349ae11038001a81010000f03140000000c060606000000000000000000",
        "110101030111000000001100ffffff",
    );

    /// The PLAYLIST row of a real root menu, from `captures/S05-link-browse`.
    ///
    /// Pins the whole menu-item layout at once: twelve arguments, the
    /// U+FFFA/U+FFFB wrapping, the label byte length in argument 2 against the
    /// character count in the string field beside it, the category item type
    /// and the zero flags.
    const ROOT_PLAYLIST_ITEM: &str = concat!(
        "11872349ae11038001a91041010f0c140000000c060606020602060606060606",
        "110000000011000000051100000016260000000bfffa0050004c00410059004c",
        "004900530054fffb000011000000022600000001000011000000841100000000",
        "1100000000110000000011000000001100000000",
    );

    /// A real `GET_WAVEFORM_PREVIEW` request, from `captures/S06-load-and-play`.
    ///
    /// **Five arguments declared, four on the wire.** Argument 3 is zero, so
    /// argument 4 — the blob — is absent entirely. This is the message that
    /// desynchronises a naive parser.
    const OMITTED_BLOB: &str = concat!(
        "11872349ae110380036c1020040f05140000000c060606060300000000000000",
        "1101080301110000000311000000c81100000000",
    );

    /// The `0x4b02` answer to `0x3e03`, from dysentery's `LinkInfo.pcapng`.
    ///
    /// The contrast with [`OMITTED_BLOB`]: argument 3 is an **empty string**,
    /// and an empty string *is* on the wire — one character, the NUL, two
    /// bytes. Only blobs vanish.
    const UNKNOWN_4B02: &str = concat!(
        "11872349ae1103800001104b020f04140000000c060606020000000000000000",
        "1100003e031100000000110000000226000000010000",
    );

    /// The title item of a real `GET_METADATA` reply, from
    /// `captures/S06-load-and-play`.
    ///
    /// `[1, 0xc8, 0x2c, "Loneliness - Klub Cut", 2, "", 4, 0x01000000, 0xba, 0,
    /// 0x100, 0]` — the artwork id in argument 8, and argument 10 tracking the
    /// flags (F32).
    const METADATA_TITLE_ITEM: &str = concat!(
        "11872349ae110380036e1041010f0c140000000c060606020602060606060606",
        "110000000111000000c8110000002c2600000016004c006f006e0065006c0069",
        "006e0065007300730020002d0020004b006c0075006200200043007500740000",
        "1100000002260000000100001100000004110100000011000000ba1100000000",
        "11000001001100000000",
    );

    /// The path item of a real `GET_TRACK_INFO` reply, from
    /// `captures/S06-load-and-play`.
    ///
    /// Argument 0 is `0x747a7b` — 7 633 531 bytes, the file size, in the slot
    /// that is zero on every other menu item ever captured (F31).
    const TRACK_INFO_PATH_ITEM: &str = concat!(
        "11872349ae110380036b1041010f0c140000000c060606020602060606060606",
        "1100747a7b11000000c811000000862600000043002f0043006f006e00740065",
        "006e00740073002f0054006f006d00630072006100660074002f004c006f006e",
        "0065006c0069006e006500730073002f0054006f006d00630072006100660074",
        "0020002d0020004c006f006e0065006c0069006e0065007300730020002d0020",
        "004b006c007500620020004300750074002e006d007000330000110000000226",
        "0000000100001100000000110000000011000000001100000000110000000011",
        "00000000",
    );

    /// A real `0x4902` reply, from `captures/S20-browse-ground-truth`.
    ///
    /// An undecoded message type carrying a genuine 148-byte blob. Two things
    /// at once: a blob argument that is *present*, and a type this crate does
    /// not model surviving a round trip intact.
    const BINARY_REPLY: &str = concat!(
        "11872349ae11038008b11049020f04140000000c060606030000000000000000",
        "1100003903110000000011000000941400000094530041004d00320000000000",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "000000000000000000000000000000000000000032003000320035002d003000",
        "36002d0032003400000000003100300030003000000000000000000000000000",
        "000000000000000000000000b40200000000010123000000070000000080ca28",
        "0500000000c0007d",
    );

    /// A real `MENU_SEARCH` for the single letter `H`, from
    /// `captures/S20-browse-ground-truth`.
    const SEARCH: &str = concat!(
        "11872349ae11038008171013000f05140000000c060606020600000000000000",
        "1102010301110000000011000000042600000002004800001100000000",
    );

    /// A real `MENU_ITEM` whose two empty labels end in `0x0009` rather than a
    /// NUL, from `captures/S06-load-and-play`.
    ///
    /// The byte-length arguments say both labels are empty (2 bytes each) and
    /// both string fields announce one character. The character a real deck put
    /// in the terminator slot is `0x0009`. See [`Field::Text`].
    const STALE_TERMINATOR: &str = concat!(
        "11872349ae11038003691041010f0c140000000c060606020602060606060606",
        "110000000011000000c8110000000226000000010009110000000226000000",
        "0100091100000004110100000011000000001100000000110000010011000000",
        "00",
    );

    const CAPTURED: &[(&str, &str)] = &[
        ("menu_close", MENU_CLOSE),
        ("introduce", INTRODUCE),
        ("introduce_reply", INTRODUCE_REPLY),
        ("menu_root", MENU_ROOT),
        ("root_playlist_item", ROOT_PLAYLIST_ITEM),
        ("omitted_blob", OMITTED_BLOB),
        ("unknown_4b02", UNKNOWN_4B02),
        ("metadata_title_item", METADATA_TITLE_ITEM),
        ("track_info_path_item", TRACK_INFO_PATH_ITEM),
        ("binary_reply", BINARY_REPLY),
        ("search", SEARCH),
        ("stale_terminator", STALE_TERMINATOR),
    ];

    // -- framing ----------------------------------------------------------

    #[test]
    fn every_captured_message_round_trips_byte_for_byte() {
        // The committed fixture floor: a coverage regression cannot hide behind
        // an empty corpus.
        assert!(CAPTURED.len() >= 12, "the fixture floor has shrunk");
        for (name, text) in CAPTURED {
            let raw = hex(text);
            let (message, consumed) =
                Message::decode(&raw).unwrap_or_else(|err| panic!("{name}: {err}"));
            assert_eq!(consumed, raw.len(), "{name}: consumed the whole message");
            assert_eq!(
                message.encode(),
                raw,
                "{name} re-encoded differently:\n  {message:?}"
            );
        }
    }

    #[test]
    fn a_zero_length_blob_is_omitted_from_the_wire() {
        let raw = hex(OMITTED_BLOB);
        let (message, consumed) = Message::decode(&raw).unwrap();
        assert_eq!(consumed, raw.len());
        assert_eq!(message.kind, MessageKind::GET_WAVEFORM_PREVIEW);
        // Five arguments declared in the header...
        assert_eq!(raw[14], 5);
        assert_eq!(message.args.len(), 5);
        // ...and the fifth is a blob that never appeared.
        assert_eq!(message.number(3), Some(0));
        assert_eq!(message.blob(4), Some(&[][..]));
        // A naive parser would read the next message's magic here: the header
        // is 32 bytes and four UInt32 arguments are 5 bytes each.
        assert_eq!(raw.len(), 32 + 4 * 5);
        assert_eq!(message.encode(), raw);
    }

    #[test]
    fn an_empty_string_is_present_where_an_empty_blob_would_not_be() {
        let raw = hex(UNKNOWN_4B02);
        let (message, _) = Message::decode(&raw).unwrap();
        assert_eq!(message.text(3), Some(""));
        // One character — the NUL — and two bytes of it, on the wire.
        assert!(
            raw.ends_with(&[0x26, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00]),
            "the empty string is written out in full"
        );
    }

    #[test]
    fn an_empty_blob_is_written_when_its_absence_could_not_be_inferred() {
        // The encoder omits an argument if and only if the decoder would infer
        // it is missing. Here the preceding argument is non-zero, so a reader
        // expects the blob and the writer must supply it — otherwise the pair
        // stop being inverses and the stream desynchronises.
        let message = Message::new(
            1,
            MessageKind::ARTWORK,
            [Field::U32(7), Field::Blob(Vec::new())],
        );
        let raw = message.encode();
        let (decoded, consumed) = Message::decode(&raw).unwrap();
        assert_eq!(consumed, raw.len());
        assert_eq!(decoded, message);
        assert!(raw.ends_with(&[FieldTag::BLOB.0, 0, 0, 0, 0]));
    }

    #[test]
    fn an_omitted_blob_counts_as_one_for_the_argument_after_it() {
        // The ksy's rule: an argument that was itself omitted contributes 1,
        // not 0, so two blobs in a row after a zero length do not both vanish.
        let message = Message::new(
            1,
            MessageKind::CUE_POINTS,
            [
                Field::U32(0),
                Field::Blob(Vec::new()),
                Field::Blob(Vec::new()),
            ],
        );
        let raw = message.encode();
        let (decoded, _) = Message::decode(&raw).unwrap();
        assert_eq!(decoded, message);
        // The first blob is omitted; the second is written out.
        assert_eq!(raw.len(), 32 + 5 + 5);
    }

    #[test]
    fn a_truncated_message_is_distinguishable_from_a_malformed_one() {
        let raw = hex(ROOT_PLAYLIST_ITEM);
        for cut in 1..raw.len() {
            let err = Message::decode(&raw[..cut]).expect_err("must not decode");
            assert!(
                err.is_truncated(),
                "cutting at {cut} should mean 'wait for more', got {err}"
            );
        }
        assert!(Message::decode(&raw).is_ok());
    }

    #[test]
    fn a_bad_magic_is_not_truncation() {
        let mut raw = hex(INTRODUCE);
        raw[2] = 0x00;
        let err = Message::decode(&raw).expect_err("must not decode");
        assert!(matches!(err, Error::BadMagic { .. }));
        assert!(!err.is_truncated(), "more bytes will not help");

        // Nor is a first byte that is not even a UInt32 tag.
        let err = Message::decode(b"GET / HTTP/1.1\r\n").expect_err("must not decode");
        assert!(matches!(err, Error::BadMagic { .. }));
        assert!(!err.is_truncated());
    }

    #[test]
    fn an_implausible_length_is_refused_before_allocating() {
        // A blob claiming 3 GB with nothing behind it. Truncation would be the
        // wrong answer: no amount of waiting produces four gigabytes.
        let mut raw = hex(INTRODUCE);
        raw[20] = ArgTag::BLOB.0;
        raw.truncate(32);
        raw.extend_from_slice(&[FieldTag::BLOB.0, 0xc0, 0x00, 0x00, 0x00]);
        let err = Message::decode(&raw).expect_err("must not decode");
        assert!(matches!(err, Error::ImplausibleLength { .. }));
        assert!(!err.is_truncated());
    }

    #[test]
    fn the_two_numberings_must_agree() {
        // Argument 0 of an INTRODUCE is a UInt32; relabel it a string in the
        // header blob only. A decoder that trusted one numbering and ignored
        // the other would yield a message that re-encodes differently from the
        // bytes that arrived.
        let mut raw = hex(INTRODUCE);
        raw[20] = ArgTag::TEXT.0;
        let err = Message::decode(&raw).expect_err("must not decode");
        assert!(matches!(err, Error::Malformed { .. }), "got {err}");
    }

    #[test]
    fn the_tag_blob_is_derived_from_the_arguments_not_stored() {
        let message = Message::new(
            1,
            MessageKind::ARTWORK,
            [
                Field::U32(0x2003),
                Field::U32(0),
                Field::U32(3),
                Field::Blob(vec![1, 2, 3]),
                Field::from("x"),
            ],
        );
        let raw = message.encode();
        assert_eq!(
            &raw[20..32],
            &[
                ArgTag::U32.0,
                ArgTag::U32.0,
                ArgTag::U32.0,
                ArgTag::BLOB.0,
                ArgTag::TEXT.0,
                0,
                0,
                0,
                0,
                0,
                0,
                0
            ],
            "one tag per argument, zeroes after"
        );
        assert_eq!(raw[14], 5, "the count is derived too");
    }

    #[test]
    fn more_than_twelve_arguments_cannot_be_built_or_decoded() {
        assert!(Arguments::new((0..13).map(Field::U32)).is_none());
        assert!(Arguments::new((0..12).map(Field::U32)).is_some());

        let mut raw = hex(INTRODUCE);
        raw[14] = 13;
        let err = Message::decode(&raw).expect_err("must not decode");
        assert!(matches!(err, Error::Malformed { .. }), "got {err}");
    }

    #[test]
    fn a_camelot_key_indexes_the_wheel_the_way_a_real_deck_does() {
        // Every one of these pairs was read off the wire: 1265 track rows a
        // real deck served in `S27-sort-by-and-key`, correlated against those
        // tracks' keys in the medium's own `export.pdb`. The wheel is walked
        // round in order, minor before major.
        for (name, expected) in [
            ("1A", 1),
            ("1B", 2),
            ("2A", 3),
            ("2B", 4),
            ("3A", 5),
            ("3B", 6),
            ("4A", 7),
            ("4B", 8),
            ("5A", 9),
            ("5B", 10),
            ("6A", 11),
            ("6B", 12),
            ("7A", 13),
            ("7B", 14),
            ("8A", 15),
            ("8B", 16),
            ("9A", 17),
            ("9B", 18),
            ("10A", 19),
            ("11A", 21),
            ("11B", 22),
            ("12A", 23),
            ("12B", 24),
        ] {
            let key = CamelotKey::parse(name).unwrap_or_else(|| panic!("{name} must parse"));
            assert_eq!(key.index(), expected, "{name}");
        }
    }

    #[test]
    fn the_classical_names_rekordbox_writes_land_on_the_same_wheel() {
        // A medium written with the classical preference must light the same
        // indicator as one written in Camelot.
        for (classical, camelot) in [
            ("Am", "8A"),
            ("C", "8B"),
            ("F#m", "11A"),
            ("A", "11B"),
            ("Abm", "1A"),
            ("B", "1B"),
            ("Dbm", "12A"),
            ("E", "12B"),
            ("G#m", "1A"),
            ("Gb", "2B"),
        ] {
            assert_eq!(
                CamelotKey::parse(classical).map(CamelotKey::index),
                CamelotKey::parse(camelot).map(CamelotKey::index),
                "{classical} should be {camelot}",
            );
        }
    }

    #[test]
    fn a_track_with_no_key_gets_no_indicator_rather_than_a_wrong_one() {
        assert_eq!(CamelotKey::parse(""), None);
        assert_eq!(CamelotKey::parse("   "), None);
        assert_eq!(CamelotKey::parse("13A"), None, "off the wheel");
        assert_eq!(CamelotKey::parse("0A"), None);
        assert_eq!(CamelotKey::parse("nonsense"), None);
        assert_eq!(
            MenuItem::track(1, "t", "", ItemType::TRACK_TITLE, 0)
                .with_key(None)
                .key_index,
            0
        );
    }

    #[test]
    fn a_media_info_body_matches_the_one_a_real_deck_sent() {
        // The whole reply, captured from a CDJ-2000NXS answering `0x3903`.
        let raw = hex(BINARY_REPLY);
        let (message, _) = Message::decode(&raw).unwrap();
        assert_eq!(message.kind, MessageKind::MEDIA_INFO);
        let body = message.blob(3).expect("the 148-byte body");

        let parsed = MediaInfo::parse(body).expect("it parses");
        assert_eq!(
            parsed.volume_name, "SAM2",
            "UTF-16 *little*-endian, unlike every other string here"
        );
        assert_eq!(parsed.created, "2025-06-24");
        // The two counts are what tie this body to the UDP media response for
        // the same medium, which reports exactly these.
        assert_eq!(parsed.track_count, 692);
        assert_eq!(parsed.playlist_count, 35);
        assert_eq!(parsed.total_bytes, 0x28ca_8000);

        // And ours is byte-identical to the deck's, unknown words included.
        assert_eq!(parsed.encode(), body, "we must send what a deck sends");
    }

    #[test]
    fn an_unknown_message_type_decodes_instead_of_failing() {
        // A blob argument that is *present*, and a message whose type this
        // crate does not model surviving a round trip intact. `0x4902` used to
        // be the unnamed one here; it is `media_info` now, so the type byte is
        // moved to one nothing names.
        let mut raw = hex(BINARY_REPLY);
        raw[11] = 0x7f;
        raw[12] = 0x7f;
        let (message, _) = Message::decode(&raw).unwrap();
        assert_eq!(message.kind, MessageKind(0x7f7f));
        assert_eq!(message.kind.name(), None);
        assert_eq!(message.blob(3).map(<[u8]>::len), Some(148));
        assert_eq!(message.encode(), raw, "and survives a round trip");
    }

    #[test]
    fn two_messages_decode_one_after_the_other() {
        let mut stream = PREAMBLE.to_vec();
        stream.extend_from_slice(&hex(INTRODUCE));
        stream.extend_from_slice(&hex(MENU_ROOT));

        let body = skip_preamble(&stream);
        assert_eq!(body.len(), stream.len() - PREAMBLE.len());
        let (first, used) = Message::decode(body).unwrap();
        assert_eq!(first.kind, MessageKind::INTRODUCE);
        let (second, used2) = Message::decode(&body[used..]).unwrap();
        assert_eq!(second.kind, MessageKind::MENU_ROOT);
        assert_eq!(used + used2, body.len(), "nothing left over");
    }

    #[test]
    fn skipping_the_preamble_leaves_a_stream_without_one_alone() {
        let raw = hex(INTRODUCE);
        assert_eq!(skip_preamble(&raw), raw.as_slice());
        assert_eq!(skip_preamble(&PREAMBLE), &[] as &[u8]);
    }

    // -- strings ----------------------------------------------------------

    #[test]
    fn a_string_length_counts_characters_including_the_nul_not_bytes() {
        let encoded = encode_string("abc");
        assert_eq!(
            &encoded[..4],
            4u32.to_be_bytes(),
            "three characters plus the NUL"
        );
        assert_eq!(encoded.len(), 4 + 8);
        assert_eq!(string_characters("abc"), 4);
        // The menu-item label arguments are the same thing in bytes.
        assert_eq!(label_bytes("abc"), 8);
        assert_eq!(label_bytes(""), 2);
    }

    #[test]
    fn a_string_is_utf16_big_endian_not_little() {
        assert_eq!(encode_string("A"), [0, 0, 0, 2, 0x00, b'A', 0x00, 0x00]);
    }

    #[test]
    fn a_terminator_that_is_not_a_nul_is_reproduced_rather_than_normalised() {
        // A real deck announces one character — "the terminator and nothing
        // else" — and writes 0x0009 in it. Both reference implementations
        // rewrite that as a NUL, and neither round-trip test notices because
        // the one capture they read does not contain the case.
        let raw = hex(STALE_TERMINATOR);
        let (message, _) = Message::decode(&raw).unwrap();
        assert_eq!(message.number(2), Some(2), "the label is empty (2 bytes)");
        assert_eq!(message.text(3), Some(""), "and reads as empty");
        assert_eq!(
            message.args.get(3),
            Some(&Field::Text {
                text: String::new(),
                terminator: Some(0x0009),
            })
        );
        assert_eq!(message.encode(), raw);
        // A string we build ourselves terminates properly.
        assert_eq!(
            Field::from(""),
            Field::Text {
                text: String::new(),
                terminator: Some(NUL),
            }
        );
    }

    #[test]
    fn a_label_length_counts_utf16_units_not_code_points() {
        // The captured PLAYLIST row announces 0x16 for a ten-character label.
        let wrapped = menu_label("PLAYLIST");
        assert_eq!(wrapped.chars().count(), 10);
        assert_eq!(label_bytes(&wrapped), 0x16);
        // An astral character is one code point and two UTF-16 units; the wire
        // counts what it carries.
        assert_eq!("\u{1f3b5}".chars().count(), 1);
        assert_eq!(string_characters("\u{1f3b5}"), 3);
        assert_eq!(encode_string("\u{1f3b5}").len(), 4 + 6);
    }

    #[test]
    fn a_non_ascii_string_survives_a_round_trip() {
        for text in ["", "Blue Monday", "夜のテーマ", "\u{1f3b5} track"] {
            let message = Message::new(1, MessageKind::MENU_ITEM, [Field::from(text)]);
            let (decoded, _) = Message::decode(&message.encode()).unwrap();
            assert_eq!(decoded.text(0), Some(text));
        }
    }

    #[test]
    fn the_captured_playlist_row_carries_the_label_wrapping() {
        let raw = hex(ROOT_PLAYLIST_ITEM);
        let (message, _) = Message::decode(&raw).unwrap();
        let item = MenuItem::from_message(&message).expect("a menu item");
        assert_eq!(item.label1, "\u{fffa}PLAYLIST\u{fffb}");
        assert_eq!(unwrap_menu_label(&item.label1), Some("PLAYLIST"));
        assert_eq!(item.id, 0x05);
        assert_eq!(item.item_type, ItemType::MENU_PLAYLIST);
        assert_eq!(item.flags, 0, "a category row carries no flags (F26)");
        assert_eq!(message.number(2), Some(0x16), "label length in bytes");
        // And a rebuilt one is byte-identical to the deck's.
        assert_eq!(item.to_message(message.transaction_id).encode(), raw);
    }

    // -- the descriptor ---------------------------------------------------

    #[test]
    fn a_descriptor_packs_device_menu_slot_and_track_type() {
        let descriptor = Descriptor::new(
            browsable(3),
            Slot::USB,
            MenuTarget::MAIN,
            TrackType::REKORDBOX,
        );
        assert_eq!(descriptor.to_raw(), 0x0301_0301);
        assert_eq!(Descriptor::parse(0x0301_0301), Some(descriptor));
        // SD is 2 and USB is 3, as the status packets number them (F37).
        assert_eq!(
            Descriptor::new(
                browsable(2),
                Slot::SD,
                MenuTarget::MAIN,
                TrackType::REKORDBOX
            )
            .to_raw(),
            0x0201_0201
        );
        assert_eq!(Descriptor::parse(0), None, "device 0 answers nothing");
    }

    #[test]
    fn the_captured_root_request_names_a_usb_slot_on_device_one() {
        let (message, _) = Message::decode(&hex(MENU_ROOT)).unwrap();
        let descriptor = message.descriptor().expect("a descriptor");
        assert_eq!(descriptor.device, device(1));
        assert_eq!(descriptor.menu, MenuTarget::MAIN);
        assert_eq!(descriptor.slot, Slot::USB);
        assert_eq!(descriptor.track_type, TrackType::REKORDBOX);
    }

    #[test]
    fn the_slot_byte_is_what_separates_two_media_on_one_connection() {
        // F37: one connection, two libraries, told apart per message.
        let sd = Descriptor::new(
            browsable(2),
            Slot::SD,
            MenuTarget::MAIN,
            TrackType::REKORDBOX,
        );
        let usb = Descriptor {
            slot: Slot::USB,
            ..sd
        };
        assert_ne!(sd.to_raw(), usb.to_raw());
        assert_eq!(sd.to_raw() ^ usb.to_raw(), 0x0000_0100);
    }

    // -- the drill grid ---------------------------------------------------

    #[test]
    fn every_observed_drill_type_comes_from_the_one_formula() {
        // The thirteen types seen in one exhaustive session (F42).
        const OBSERVED: [(u16, u8, u8); 13] = [
            (0x1101, 1, 0x01),
            (0x1102, 1, 0x02),
            (0x1103, 1, 0x03),
            (0x110a, 1, 0x0a),
            (0x1111, 1, 0x11),
            (0x1112, 1, 0x12),
            (0x1114, 1, 0x14),
            (0x1201, 2, 0x01),
            (0x1202, 2, 0x02),
            (0x120a, 2, 0x0a),
            (0x1214, 2, 0x14),
            (0x1301, 3, 0x01),
            (0x130a, 3, 0x0a),
        ];
        for (raw, depth, category) in OBSERVED {
            let drill = Drill { depth, category };
            assert_eq!(drill.kind(), MessageKind(raw));
            assert_eq!(Drill::parse(MessageKind(raw)), Some(drill));
        }
        // The three named messages of the pre-hardware literature are three
        // points in that grid, not three messages.
        assert_eq!(drill_kind(1, 0x01), MessageKind(0x1101)); // artists for genre
        assert_eq!(drill_kind(1, 0x02), MessageKind(0x1102)); // albums for artist
        assert_eq!(drill_kind(1, 0x03), MessageKind(0x1103)); // tracks for album
    }

    #[test]
    fn a_flat_menu_is_not_a_drill() {
        assert_eq!(Drill::parse(MessageKind::MENU_KEY), None);
        assert_eq!(Drill::parse(MessageKind::MENU_ROOT), None);
        assert_eq!(Drill::parse(MessageKind::GET_METADATA), None);
        // MENU_PLAYLIST is 0x1105, which the formula also spells "one level
        // into category 5". The grid is a superset of the named menus.
        assert_eq!(
            Drill::parse(MessageKind::MENU_PLAYLIST),
            Some(Drill {
                depth: 1,
                category: 0x05
            })
        );
    }

    #[test]
    fn the_drill_category_and_the_root_id_are_different_numberings() {
        // KEY is 0x14 in the request numbering and 0x0c as a root id. Deriving
        // one from the other is exactly F40's bug: a deck opening KEY asked for
        // bitrates, because 0x14 is BITRATE's root id.
        let key_request = MessageKind::MENU_KEY.0 & 0xff;
        assert_eq!(key_request, 0x14);
        let key_root = ROOT_CATEGORIES
            .iter()
            .find(|category| category.label == "KEY")
            .expect("KEY is a root category");
        assert_eq!(key_root.id, 0x0c);
        let bitrate_root = ROOT_CATEGORIES
            .iter()
            .find(|category| category.label == "BITRATE")
            .expect("BITRATE is a root category");
        assert_eq!(bitrate_root.id, u32::from(key_request));
    }

    // -- the vocabulary tables --------------------------------------------

    #[test]
    fn all_twelve_root_categories_are_listed_not_derived() {
        assert_eq!(ROOT_CATEGORIES.len(), 12);
        // Eleven of the twelve differ from their item type by 0x7f, and DATE
        // ADDED by 0x71. Both derivations that were tried produced a wrong id
        // for a category a deck then failed to open (F26, F40, F43).
        let exceptions: Vec<&str> = ROOT_CATEGORIES
            .iter()
            .filter(|category| category.item_type.0 - category.id != 0x7f)
            .map(|category| category.label)
            .collect();
        assert_eq!(exceptions, ["DATE ADDED"]);
        let date_added = ROOT_CATEGORIES
            .iter()
            .find(|category| category.label == "DATE ADDED")
            .expect("DATE ADDED is a root category");
        assert_eq!((date_added.id, date_added.item_type.0), (0x1b, 0x8c));

        let ids: Vec<u32> = ROOT_CATEGORIES.iter().map(|entry| entry.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "no two categories share an id");
    }

    #[test]
    fn a_root_category_row_matches_a_real_player() {
        let playlist = ROOT_CATEGORIES
            .iter()
            .find(|category| category.label == "PLAYLIST")
            .expect("PLAYLIST is a root category");
        let raw = hex(ROOT_PLAYLIST_ITEM);
        let (captured, _) = Message::decode(&raw).unwrap();
        assert_eq!(
            playlist
                .to_item()
                .to_message(captured.transaction_id)
                .encode(),
            raw,
            "byte-identical to the deck's own row"
        );
    }

    #[test]
    fn the_sort_menu_is_the_twelve_a_real_player_lists() {
        assert_eq!(SORT_MENU.len(), 12);
        let ids: Vec<u32> = SORT_MENU.iter().map(|option| option.sort.0).collect();
        assert_eq!(
            ids,
            [0, 1, 2, 3, 4, 5, 0x0c, 0x0d, 0x10, 6, 0x11, 0x0a],
            "the order a CDJ-2000NXS serves them in"
        );
        let types: Vec<u32> = SORT_MENU.iter().map(|option| option.item_type.0).collect();
        assert_eq!(
            types,
            [
                0xa1, 0xa2, 0x81, 0x82, 0x85, 0x86, 0x8b, 0x93, 0x97, 0x80, 0x8c, 0x89
            ]
        );
    }

    #[test]
    fn the_sort_order_selects_the_second_column() {
        // (column field type << 8) | 0x04 (F43). 0x0704 is not "title and
        // artist": it is a track row whose second column is the ARTIST field.
        for (sort, item_type) in [
            (SortOrder::DEFAULT, 0x0704),
            (SortOrder::TITLE, 0x0704),
            (SortOrder::ARTIST, 0x0704),
            (SortOrder::ALBUM, 0x0204),
            (SortOrder::BPM, 0x0d04),
            (SortOrder::RATING, 0x0a04),
            (SortOrder::GENRE, 0x0604),
            (SortOrder::LABEL, 0x0e04),
            (SortOrder::KEY, 0x0f04),
            (SortOrder::BITRATE, 0x1004),
            (SortOrder::PLAY_COUNT, 0x2a04),
            (SortOrder::DATE_ADDED, 0x2e04),
        ] {
            assert_eq!(
                sort.track_item_type(),
                ItemType(item_type),
                "second column for {sort:?}"
            );
        }
        // Numeric columns send an empty label and the raw number in argument 0.
        assert!(SortOrder::BPM.column_is_numeric());
        assert!(SortOrder::BITRATE.column_is_numeric());
        assert!(!SortOrder::ARTIST.column_is_numeric());
        assert!(!SortOrder::DATE_ADDED.column_is_numeric());
        // An order no SORT menu offers falls back rather than inventing one.
        assert_eq!(SortOrder(0x7f).column(), None);
        assert_eq!(SortOrder(0x7f).track_item_type(), ItemType(0x0704));
    }

    #[test]
    fn a_metadata_reply_is_thirteen_items_in_a_fixed_order() {
        assert_eq!(METADATA_ITEMS.len(), 13);
        let types: Vec<u32> = METADATA_ITEMS.iter().map(|slot| slot.item_type.0).collect();
        assert_eq!(
            types,
            [
                0x04, 0x07, 0x02, 0x0b, 0x0d, 0x23, 0x0f, 0x0a, 0x13, 0x06, 0x2e, 0x10, 0x0e
            ],
            "title, artist, album, duration, tempo, comment, key, rating, \
             colour, genre, date added, bitrate, label"
        );
        // Argument 0 is 1 on eight of them and 0 on five, matching no rule
        // anyone has named.
        let ones = METADATA_ITEMS
            .iter()
            .filter(|slot| slot.argument0 == 1)
            .count();
        assert_eq!(ones, 8);
        assert_eq!(METADATA_ITEMS.len() - ones, 5);
    }

    #[test]
    fn the_metadata_title_item_matches_a_real_deck() {
        let raw = hex(METADATA_TITLE_ITEM);
        let (message, _) = Message::decode(&raw).unwrap();
        let item = MenuItem::from_message(&message).expect("a menu item");
        assert_eq!(item.item_type, ItemType::TRACK_TITLE);
        assert_eq!(item.label1, "Loneliness - Klub Cut");
        assert_eq!(item.flags, MenuItem::TRACK_FLAGS);
        assert_eq!(item.artwork_id, 0xba, "without this, INFO shows no cover");
        // Argument 10 tracks argument 7 and is never independent of it (F32).
        assert_eq!(message.number(10), Some(MenuItem::TRACK_ARGUMENT_10));
        assert_eq!(item.argument10(), MenuItem::TRACK_ARGUMENT_10);
        // The metadata title item's argument 0 is 1, per METADATA_ITEMS.
        assert_eq!(item.argument0, METADATA_ITEMS[0].argument0);

        let mut rebuilt = MenuItem::track(
            item.id,
            &item.label1,
            &item.label2,
            ItemType::TRACK_TITLE,
            0xba,
        );
        rebuilt.argument0 = 1;
        assert_eq!(
            rebuilt.to_message(message.transaction_id).encode(),
            raw,
            "a rebuilt row is byte-identical"
        );
    }

    #[test]
    fn argument_ten_is_zero_exactly_when_the_flags_are() {
        let mut item = MenuItem::named(1, ItemType::ARTIST, "Tomcraft");
        assert_eq!(item.flags, 0);
        assert_eq!(item.argument10(), 0);
        item.flags = MenuItem::TRACK_FLAGS;
        assert_eq!(item.argument10(), MenuItem::TRACK_ARGUMENT_10);
    }

    #[test]
    fn a_track_info_reply_is_six_items_and_argument_zero_is_the_file_size() {
        assert_eq!(TRACK_INFO_ITEMS.len(), 6);
        let types: Vec<u32> = TRACK_INFO_ITEMS.iter().map(|item| item.0).collect();
        assert_eq!(types, [0x04, 0x0b, 0x0d, 0x23, 0x00, 0x2f]);

        let raw = hex(TRACK_INFO_PATH_ITEM);
        let (message, _) = Message::decode(&raw).unwrap();
        let item = MenuItem::from_message(&message).expect("a menu item");
        assert_eq!(item.item_type, ItemType::PATH);
        assert_eq!(
            item.argument0, 0x0074_7a7b,
            "argument 0 is the file size, not padding (F31)"
        );
        assert_eq!(item.argument0, 7_633_531);
        assert!(item.label1.ends_with("Klub Cut.mp3"));
        assert_eq!(item.flags, 0, "track-info rows carry no flags");
        assert_eq!(item.to_message(message.transaction_id).encode(), raw);
    }

    #[test]
    fn the_same_item_type_means_different_things_in_different_replies() {
        // 0x04 is the title in GET_METADATA and the container in
        // GET_TRACK_INFO (F35). Both tables name it and neither shadows the
        // other.
        assert_eq!(METADATA_ITEMS[0].item_type, ItemType::TRACK_TITLE);
        assert_eq!(TRACK_INFO_ITEMS[0], ItemType::TRACK_TITLE);
    }

    #[test]
    fn a_cdj_3000_item_type_masks_down_to_a_known_one() {
        assert_eq!(ItemType(0x0100_0004).masked(), ItemType::TRACK_TITLE);
    }

    // -- builders ---------------------------------------------------------

    #[test]
    fn a_built_introduce_matches_a_real_one() {
        assert_eq!(Message::introduce(browsable(1)).encode(), hex(INTRODUCE));
        assert_eq!(
            Message::introduce_reply(device(2)).encode(),
            hex(INTRODUCE_REPLY)
        );
    }

    #[test]
    fn a_built_menu_close_matches_a_real_one() {
        let message = Message::new(0x0380_01b7, MessageKind::MENU_CLOSE, []);
        assert_eq!(message.encode(), hex(MENU_CLOSE));
        assert!(message.kind.expects_no_reply());
        assert!(!MessageKind::MENU_ROOT.expects_no_reply());
    }

    #[test]
    fn a_built_unknown_3e03_reply_matches_a_real_one() {
        assert_eq!(
            Message::unknown_3e03_reply(0x0380_0001, device(2)).encode(),
            hex(UNKNOWN_4B02)
        );
    }

    #[test]
    fn a_binary_reply_with_no_payload_is_the_same_shape_as_one_with() {
        let request = MessageKind::GET_ARTWORK;
        let empty =
            Message::binary_reply(1, MessageKind::ARTWORK, request, Vec::new(), &[]).unwrap();
        assert_eq!(empty.args.len(), 4);
        assert_eq!(empty.number(2), Some(0));
        // Four arguments declared, three on the wire: the header plus three
        // UInt32 fields.
        assert_eq!(empty.encode().len(), 32 + 3 * 5);

        let full =
            Message::binary_reply(1, MessageKind::ARTWORK, request, vec![9; 10], &[]).unwrap();
        assert_eq!(full.number(2), Some(10));
        assert_eq!(full.encode().len(), 32 + 3 * 5 + 5 + 10);
        for message in [empty, full] {
            let (decoded, _) = Message::decode(&message.encode()).unwrap();
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn a_render_names_the_result_set_by_descriptor_and_total() {
        // Keying pending result sets on the count alone broke the moment
        // metadata became thirteen items and collided with a thirteen-track
        // album (F27, F41). Both halves of the key travel in the render.
        let descriptor = Descriptor::new(
            browsable(2),
            Slot::USB,
            MenuTarget::SUB,
            TrackType::REKORDBOX,
        );
        let render = Message::render_of(0x0380_0003, descriptor, 20, 6, 692);
        assert_eq!(render.args.len(), 6);
        assert_eq!(render.descriptor(), Some(descriptor));
        assert_eq!(render.number(1), Some(20));
        assert_eq!(render.number(2), Some(6));
        assert_eq!(render.number(4), Some(692));
        let (decoded, _) = Message::decode(&render.encode()).unwrap();
        assert_eq!(decoded, render);
        // The plain form pages a result set whose size it already knows.
        let simple = Message::render(1, descriptor, 0, 10);
        assert_eq!(simple.number(2), simple.number(4));
    }

    #[test]
    fn a_search_puts_the_term_in_argument_three() {
        // Reading argument 2 as the term is why search once matched nothing
        // (F44): argument 2 is the term's size in bytes including its NUL.
        let raw = hex(SEARCH);
        let (captured, _) = Message::decode(&raw).unwrap();
        let descriptor = captured.descriptor().expect("a descriptor");
        let search = Message::search(captured.transaction_id, descriptor, SortOrder::DEFAULT, "H");
        assert_eq!(search.number(2), Some(4));
        assert_eq!(search.text(3), Some("H"));
        assert_eq!(search.number(4), Some(0));
        assert_eq!(search.encode(), raw, "byte for byte a real deck's request");
    }

    #[test]
    fn a_menu_request_refuses_to_overflow_the_tag_blob() {
        let descriptor = Descriptor::new(
            browsable(1),
            Slot::SD,
            MenuTarget::MAIN,
            TrackType::REKORDBOX,
        );
        assert!(Message::menu_request(1, MessageKind::MENU_TRACK, descriptor, &[0; 11]).is_some());
        assert!(Message::menu_request(1, MessageKind::MENU_TRACK, descriptor, &[0; 12]).is_none());
    }

    #[test]
    fn the_all_row_heads_a_filtered_list() {
        let item = MenuItem::all();
        assert_eq!(item.id, FILTER_ALL);
        assert_eq!(item.item_type, ItemType::ALL);
        assert_eq!(item.label1, "\u{fffa}ALL\u{fffb}");
        assert_eq!(item.flags, 0);
    }

    #[test]
    fn the_header_footer_and_success_replies_have_the_shapes_a_deck_sends() {
        let header = Message::menu_header(1);
        assert_eq!(header.number(0), Some(1));
        assert_eq!(header.number(1), Some(0));
        assert!(Message::menu_footer(1).args.is_empty());
        let success = Message::success(1, MessageKind::MENU_ROOT, 12);
        assert_eq!(success.number(0), Some(0x1000));
        assert_eq!(success.number(1), Some(12));
        assert_eq!(
            Message::disconnect().transaction_id,
            SETUP_TRANSACTION_ID,
            "introduce and disconnect share the reserved id"
        );
    }

    // -- port discovery ---------------------------------------------------

    #[test]
    fn the_port_query_is_the_documented_nineteen_bytes() {
        assert_eq!(PORT_QUERY.len(), 19);
        assert_eq!(&PORT_QUERY[..4], 15u32.to_be_bytes());
        assert_eq!(&PORT_QUERY[4..18], b"RemoteDBServer");
        assert_eq!(PORT_QUERY[18], 0);
        assert_eq!(decode_port_reply(&encode_port_reply(PORT)).unwrap(), 1051);
        assert!(decode_port_reply(&[0x04]).unwrap_err().is_truncated());
    }

    #[test]
    fn the_preamble_is_a_uint32_field_holding_one() {
        assert_eq!(PREAMBLE, [0x11, 0x00, 0x00, 0x00, 0x01]);
    }
}

/// The body of a [`MessageKind::MEDIA_INFO`] reply: a medium's description.
///
/// # What this is, and how we know
///
/// `0x3903` appears in the research record only as one of four undecoded
/// message types "seen around a loaded track". It is not undecoded any more.
/// A real deck answers it with `0x4902` carrying 148 bytes, and those bytes are
/// the *same description the UDP media query returns* — the volume name, the
/// creation date, the track and playlist counts and the medium's size — laid
/// out differently and in the opposite byte order:
///
/// ```text
/// 0x00  volume name    64 bytes, UTF-16 little-endian    "SAM2"
/// 0x40  created        24 bytes, UTF-16 little-endian    "2025-06-24"
/// 0x58  unknown         8 bytes, UTF-16 little-endian    "1000"
/// 0x60  zeros          24 bytes
/// 0x78  u32 LE         track count                       692
/// 0x7c  u32 LE         unknown, 0x01010000 observed
/// 0x80  u32 LE         playlist count                    35
/// 0x84  u32 LE         unknown, 7 observed
/// 0x88  u32 LE         total bytes
/// 0x8c  u32 LE         unknown, 5 observed
/// 0x90  u32 LE         free bytes
/// ```
///
/// The two counts and the two sizes are what tie it to the UDP reply: the
/// sample carries 692 tracks and 35 playlists, which is exactly what that
/// medium's `0x06` response reports, and the same total-bytes word appears in
/// both — big-endian there, little-endian here. **Note the endianness**: every
/// other string in this protocol is UTF-16 *big*-endian, and this one is not.
///
/// The four unknown words are reproduced as observed. Substituting a plausible
/// zero is what this codebase does not do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaInfo {
    /// The volume label, as the DJ formatted it.
    pub volume_name: String,
    /// The medium's creation date, e.g. `2025-06-24`.
    pub created: String,
    /// How many tracks it holds. The true count, as everywhere else (F24).
    pub track_count: u32,
    /// How many playlists it holds.
    pub playlist_count: u32,
    /// Capacity in bytes.
    pub total_bytes: u32,
    /// Free space in bytes.
    pub free_bytes: u32,
}

impl MediaInfo {
    /// Bytes the body occupies.
    pub const LEN: usize = 148;

    const OFF_VOLUME: usize = 0x00;
    const LEN_VOLUME: usize = 0x40;
    const OFF_CREATED: usize = 0x40;
    const LEN_CREATED: usize = 0x18;
    const OFF_UNKNOWN_TEXT: usize = 0x58;
    const OFF_TRACKS: usize = 0x78;
    const OFF_PLAYLISTS: usize = 0x80;
    const OFF_TOTAL: usize = 0x88;
    const OFF_FREE: usize = 0x90;

    /// Encode the body a real deck sends.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = vec![0u8; Self::LEN];
        put_utf16le(
            &mut body,
            Self::OFF_VOLUME,
            Self::LEN_VOLUME,
            &self.volume_name,
        );
        put_utf16le(
            &mut body,
            Self::OFF_CREATED,
            Self::LEN_CREATED,
            &self.created,
        );
        // Reproduced from the one capture; meaning unknown.
        put_utf16le(&mut body, Self::OFF_UNKNOWN_TEXT, 8, "1000");
        put_u32le(&mut body, Self::OFF_TRACKS, self.track_count);
        put_u32le(&mut body, Self::OFF_TRACKS + 4, 0x0101_0000);
        put_u32le(&mut body, Self::OFF_PLAYLISTS, self.playlist_count);
        put_u32le(&mut body, Self::OFF_PLAYLISTS + 4, 7);
        put_u32le(&mut body, Self::OFF_TOTAL, self.total_bytes);
        put_u32le(&mut body, Self::OFF_TOTAL + 4, 5);
        put_u32le(&mut body, Self::OFF_FREE, self.free_bytes);
        body
    }

    /// Read a body a peer sent, or `None` if it is the wrong length.
    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::LEN {
            return None;
        }
        Some(Self {
            volume_name: utf16le(body, Self::OFF_VOLUME, Self::LEN_VOLUME),
            created: utf16le(body, Self::OFF_CREATED, Self::LEN_CREATED),
            track_count: u32le(body, Self::OFF_TRACKS),
            playlist_count: u32le(body, Self::OFF_PLAYLISTS),
            total_bytes: u32le(body, Self::OFF_TOTAL),
            free_bytes: u32le(body, Self::OFF_FREE),
        })
    }
}

/// UTF-16 **little**-endian, which this one body uses and nothing else here
/// does.
fn put_utf16le(out: &mut [u8], offset: usize, width: usize, text: &str) {
    let Some(field) = out.get_mut(offset..offset.saturating_add(width)) else {
        return;
    };
    for (pair, unit) in field.chunks_exact_mut(2).zip(text.encode_utf16()) {
        pair.copy_from_slice(&unit.to_le_bytes());
    }
}

fn put_u32le(out: &mut [u8], offset: usize, value: u32) {
    if let Some(field) = out.get_mut(offset..offset.saturating_add(4)) {
        field.copy_from_slice(&value.to_le_bytes());
    }
}

fn utf16le(body: &[u8], offset: usize, width: usize) -> String {
    let Some(field) = body.get(offset..offset.saturating_add(width)) else {
        return String::new();
    };
    let units: Vec<u16> = field
        .chunks_exact(2)
        .filter_map(|pair| <[u8; 2]>::try_from(pair).ok())
        .map(u16::from_le_bytes)
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

fn u32le(body: &[u8], offset: usize) -> u32 {
    body.get(offset..offset.saturating_add(4))
        .and_then(|field| <[u8; 4]>::try_from(field).ok())
        .map_or(0, u32::from_le_bytes)
}
