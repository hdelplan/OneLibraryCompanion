// SPDX-License-Identifier: GPL-3.0-only

//! Player status, media queries and device settings — UDP 50002, unicast.
//!
//! The half of Pro DJ Link that is **invisible until you announce**. A player
//! unicasts on 50002 to peers that have announced themselves and to nobody
//! else: all 1507 status packets in one session went deck-to-deck, and not one
//! reached a host that had been on the network the whole time without
//! announcing (F21).
//!
//! That single fact decides the shape of the whole library. Slot occupancy is
//! published here and *nowhere else* (F20), so a virtual CDJ is a hard
//! prerequisite both for browsing a deck and for being browsed by one — and a
//! device that does not emit status is a device with empty slots however loudly
//! it announces.
//!
//! # The header is not the one on port 50000
//!
//! The device name occupies `0x0b`–`0x1e` — one byte earlier and one byte
//! shorter than on the discovery port — and byte `0x1f` is a structural `0x01`
//! where the discovery header has its name's last byte (C14). The type byte at
//! `0x0a` is *shared* between the two ports and the layouts behind it are not:
//! `0x06` is a keep-alive on 50000 and a media response here. Reusing
//! [`crate::djl`]'s decoder yields plausible nonsense rather than an error.
//!
//! ```text
//! 0x00-0x09  magic "Qspt1WmJOL"
//! 0x0a       packet kind
//! 0x0b-0x1e  device name        20 bytes ASCII, NUL-padded
//! 0x1f       structural 0x01
//! 0x20       subtype
//! 0x21       sender device number
//! 0x22-0x23  body length        bytes following 0x24
//! 0x24…      body
//! ```
//!
//! # What is a field and what is a template
//!
//! [`MediaQuery`] and [`SettingsQuery`] are short and fully understood, so they
//! are ordinary structs. [`CdjStatus`], [`MediaResponse`] and
//! [`SettingsResponse`] are not: a status packet is 284 bytes of which about a
//! dozen can be named. Those three own their datagram and expose the fields we
//! understand as accessors — which is honest about the ~260 bytes we do not,
//! and is what lets an emitted packet be byte-diffed against a real one.
//!
//! Their constructors establish, once, that the buffer carries the magic, the
//! right kind byte and enough length for every field the accessors read. The
//! accessors are therefore total. Fields past that guaranteed prefix return
//! [`Option`], because a short packet from older hardware genuinely does not
//! contain them.

use std::fmt;
use std::net::Ipv4Addr;
use std::time::Duration;

use crate::beat::{BeatInBar, Pitch};
use crate::device::{DeviceName, DeviceNumber};
use crate::status_templates as templates;
use crate::{Error, MAGIC, Result, Slot};

/// Status-packet cadence.
///
/// Measured at 199 ms mean (min 63, max 207) on a real CDJ-2000nexus.
pub const STATUS_INTERVAL: Duration = Duration::from_millis(200);

// The offsets below, and the four helpers further down marked `pub(crate)`,
// describe the header **shared with UDP 50001** — see [`crate::beat`], which
// reads and writes exactly this layout with a different byte at `0x0a`. They
// live here because this is where they were first written down; a second copy
// in the beat codec would be two things that have to be corrected together.

/// Offset of the 20-byte device name. **Not** `0x0c` as on the discovery port.
pub(crate) const OFF_NAME: usize = 0x0b;
/// Offset of the structural `0x01` (C14).
pub(crate) const OFF_CONST_ONE: usize = 0x1f;
pub(crate) const OFF_SUBTYPE: usize = 0x20;
pub(crate) const OFF_SENDER: usize = 0x21;
pub(crate) const OFF_BODY_LEN: usize = 0x22;
/// First byte of the body; body length counts from here.
const BODY_START: usize = 0x24;

/// Byte `0x0a`, the discriminator on this port.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatusKind(pub u8);

impl StatusKind {
    /// "Device N, describe slot S."
    pub const MEDIA_QUERY: Self = Self(0x05);
    /// The answer to a [`Self::MEDIA_QUERY`].
    pub const MEDIA_RESPONSE: Self = Self(0x06);
    /// A player publishing what it is doing, every ~200 ms.
    pub const CDJ_STATUS: Self = Self(0x0a);
    /// A mixer publishing the same.
    pub const MIXER_STATUS: Self = Self(0x29);
    /// "Give me the settings on your slot N."
    pub const SETTINGS_QUERY: Self = Self(0x35);
    /// The answer to a [`Self::SETTINGS_QUERY`].
    pub const SETTINGS_RESPONSE: Self = Self(0x36);

    /// A name for logs, or `None` for a kind we have never observed.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::MEDIA_QUERY => "media_query",
            Self::MEDIA_RESPONSE => "media_response",
            Self::CDJ_STATUS => "cdj_status",
            Self::MIXER_STATUS => "mixer_status",
            Self::SETTINGS_QUERY => "settings_query",
            Self::SETTINGS_RESPONSE => "settings_response",
            _ => return None,
        })
    }
}

impl fmt::Debug for StatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "StatusKind({:#04x})", self.0),
        }
    }
}

/// A slot's local state, as the owning player reports it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaState(pub u8);

impl MediaState {
    /// A medium is present and mounted.
    pub const LOADED: Self = Self(0x00);
    /// The medium is being ejected.
    pub const UNMOUNTING: Self = Self(0x02);
    /// A second unmounting state; the difference is not understood.
    pub const UNMOUNTING_ALT: Self = Self(0x03);
    /// The slot is empty.
    pub const EMPTY: Self = Self(0x04);

    /// Whether a medium is present and usable.
    pub const fn has_media(self) -> bool {
        self.0 == Self::LOADED.0
    }
}

impl fmt::Debug for MediaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::LOADED => f.write_str("loaded"),
            Self::UNMOUNTING => f.write_str("unmounting"),
            Self::UNMOUNTING_ALT => f.write_str("unmounting_alt"),
            Self::EMPTY => f.write_str("empty"),
            Self(raw) => write!(f, "MediaState({raw:#04x})"),
        }
    }
}

/// One decoded UDP-50002 datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Packet {
    /// A player's periodic status.
    CdjStatus(CdjStatus),
    /// "Describe slot S of device N."
    MediaQuery(MediaQuery),
    /// The description of a medium.
    MediaResponse(MediaResponse),
    /// "Give me the settings on your slot N."
    SettingsQuery(SettingsQuery),
    /// The settings from a medium, inline.
    SettingsResponse(SettingsResponse),
    /// A well-formed datagram of a kind this crate does not model, including
    /// mixer status.
    Other {
        /// The kind byte at `0x0a`.
        kind: StatusKind,
        /// The whole datagram.
        raw: Vec<u8>,
    },
}

impl Packet {
    /// The kind byte this packet carries.
    pub fn kind(&self) -> StatusKind {
        match self {
            Self::CdjStatus(_) => StatusKind::CDJ_STATUS,
            Self::MediaQuery(_) => StatusKind::MEDIA_QUERY,
            Self::MediaResponse(_) => StatusKind::MEDIA_RESPONSE,
            Self::SettingsQuery(_) => StatusKind::SETTINGS_QUERY,
            Self::SettingsResponse(_) => StatusKind::SETTINGS_RESPONSE,
            Self::Other { kind, .. } => *kind,
        }
    }
}

/// Decode one UDP-50002 datagram.
///
/// A kind we do not model, or one too short for the fields its layout declares,
/// comes back as [`Packet::Other`] rather than as an error: a decoder that gave
/// up on the first surprise would stop tracking the peers it does understand.
pub fn decode(data: &[u8]) -> Result<Packet> {
    if data.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
        let got = data.get(..MAGIC.len()).unwrap_or(data);
        return Err(Error::BadMagic {
            expected: MAGIC.as_slice().into(),
            got: got.into(),
        });
    }
    let Some(&raw_kind) = data.get(MAGIC.len()) else {
        return Err(Error::Truncated {
            need: MAGIC.len() + 1,
            at: 0,
            have: data.len(),
        });
    };
    let kind = StatusKind(raw_kind);
    let other = || Packet::Other {
        kind,
        raw: data.to_vec(),
    };

    Ok(match kind {
        StatusKind::CDJ_STATUS => {
            CdjStatus::parse(data).map_or_else(|_| other(), Packet::CdjStatus)
        }
        StatusKind::MEDIA_QUERY => {
            MediaQuery::parse(data).map_or_else(|_| other(), Packet::MediaQuery)
        }
        StatusKind::MEDIA_RESPONSE => {
            MediaResponse::parse(data).map_or_else(|_| other(), Packet::MediaResponse)
        }
        StatusKind::SETTINGS_QUERY => {
            SettingsQuery::parse(data).map_or_else(|_| other(), Packet::SettingsQuery)
        }
        StatusKind::SETTINGS_RESPONSE => {
            SettingsResponse::parse(data).map_or_else(|_| other(), Packet::SettingsResponse)
        }
        _ => other(),
    })
}

// -- byte access ----------------------------------------------------------

pub(crate) fn byte_at(raw: &[u8], offset: usize) -> Option<u8> {
    raw.get(offset).copied()
}

pub(crate) fn be_u16_at(raw: &[u8], offset: usize) -> Option<u16> {
    let bytes = raw.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_be_bytes(bytes.try_into().ok()?))
}

pub(crate) fn be_u32_at(raw: &[u8], offset: usize) -> Option<u32> {
    let bytes = raw.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

/// Decode a fixed-width UTF-16 **big**-endian field.
///
/// Big-endian here and in the dbserver protocol; the NFS layer of the same
/// protocol uses little-endian. The two must never share a helper.
///
/// The padding is not a terminator — the field is always its full width and the
/// name simply stops — so decoding runs to the first NUL and no further.
fn utf16be_at(raw: &[u8], offset: usize, len: usize) -> String {
    let Some(field) = raw.get(offset..offset.saturating_add(len)) else {
        return String::new();
    };
    let units: Vec<u16> = field
        .chunks_exact(2)
        .map(|pair| {
            u16::from_be_bytes([
                pair.first().copied().unwrap_or(0),
                pair.get(1).copied().unwrap_or(0),
            ])
        })
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Write the 50002 header into an already-sized buffer.
fn write_header(out: &mut [u8], kind: StatusKind, name: DeviceName, sender: u8) {
    write_shared_header(out, kind.0, name, sender);
}

/// Write the header shared by UDP 50001 and 50002 into an already-sized buffer.
///
/// The byte at `0x0a` is port-specific — [`StatusKind`] here and
/// [`crate::beat::BeatKind`] on 50001 — so it arrives as a plain byte rather
/// than as either enumeration.
pub(crate) fn write_shared_header(out: &mut [u8], kind: u8, name: DeviceName, sender: u8) {
    let total = out.len();
    if let Some(magic) = out.get_mut(..MAGIC.len()) {
        magic.copy_from_slice(&MAGIC);
    }
    if let Some(slot) = out.get_mut(MAGIC.len()) {
        *slot = kind;
    }
    if let Some(field) = out.get_mut(OFF_NAME..OFF_NAME + DeviceName::LEN) {
        field.copy_from_slice(&name.0);
    }
    if let Some(slot) = out.get_mut(OFF_CONST_ONE) {
        *slot = 0x01;
    }
    if let Some(slot) = out.get_mut(OFF_SENDER) {
        *slot = sender;
    }
    if let Some(field) = out.get_mut(OFF_BODY_LEN..OFF_BODY_LEN + 2) {
        let body = u16::try_from(total.saturating_sub(BODY_START)).unwrap_or(u16::MAX);
        field.copy_from_slice(&body.to_be_bytes());
    }
}

/// The header fields every packet on this port carries, once the constructor
/// has proven the buffer is long enough to hold them.
macro_rules! header_accessors {
    () => {
        /// The whole datagram.
        pub fn as_bytes(&self) -> &[u8] {
            &self.raw
        }

        /// Take ownership of the datagram.
        pub fn into_bytes(self) -> Vec<u8> {
            self.raw
        }

        /// `0x0b`–`0x1e`, the announcing device's name.
        pub fn name(&self) -> DeviceName {
            let mut field = [0u8; DeviceName::LEN];
            if let Some(bytes) = self.raw.get(OFF_NAME..OFF_NAME + DeviceName::LEN) {
                field.copy_from_slice(bytes);
            }
            DeviceName(field)
        }

        /// Byte `0x20`.
        pub fn subtype(&self) -> u8 {
            byte_at(&self.raw, OFF_SUBTYPE).unwrap_or(0)
        }

        /// Byte `0x21` — who sent this.
        ///
        /// Present on every kind on this port, which is what lets a receiver
        /// attribute a packet without looking at the source address. `None`
        /// when the field is zero, which is not a device any request can be
        /// addressed to.
        pub fn sender(&self) -> Option<DeviceNumber> {
            DeviceNumber::new(byte_at(&self.raw, OFF_SENDER).unwrap_or(0))
        }

        /// `0x22`–`0x23` — bytes following `0x24`, as the sender counts them.
        pub fn body_length(&self) -> u16 {
            be_u16_at(&self.raw, OFF_BODY_LEN).unwrap_or(0)
        }
    };
}

// -- CDJ status (0x0a) ----------------------------------------------------

/// A player's periodic status packet.
///
/// Owns its datagram. Constructing one proves the buffer carries the magic, the
/// What byte `0x92` holds when the player has no track and so no tempo.
///
/// Read as a number it is 655.35 BPM, which is why it is a named sentinel here
/// and an [`Option`] in the accessor rather than something every caller has to
/// remember (F49).
const NO_TEMPO: u16 = 0xffff;

/// What byte `0x9f` holds when no master handoff is in progress.
const NO_YIELD: u8 = 0xff;

/// Byte `0x89`: what the player is doing, as four bits.
///
/// A newtype over the raw byte rather than a `bitflags` set, because three of
/// the eight bits have never been seen to move and inventing names for them
/// would be a guess presented as knowledge. Across 46,012 captured status
/// packets exactly eight values appear — `0x84`, `0x94`, `0xa4`, `0xb4`,
/// `0xc4`, `0xd4`, `0xe4`, `0xf4` — so bits `0x80` and `0x04` are always set,
/// bits `0x08`, `0x02` and `0x01` never are, and only the three bits named
/// below vary (F50).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatusFlags(pub u8);

impl StatusFlags {
    /// Bit `0x40`. The deck is producing sound.
    ///
    /// Not the same thing as [`CdjStatus::play_state`] being `3`: 289 of the
    /// 8,859 captured packets with play state `3` have this bit clear, so the
    /// two are separate observations and this is the one about audio.
    pub const PLAYING: u8 = 0x40;
    /// Bit `0x20`. This deck is tempo master. See
    /// [`CdjStatus::is_tempo_master`], which is the authoritative field.
    pub const TEMPO_MASTER: u8 = 0x20;
    /// Bit `0x10`. SYNC is lit on this deck.
    ///
    /// Established by the pitch slewing to match the master's effective tempo
    /// in the same packet the bit first appears (F51).
    pub const SYNC: u8 = 0x10;
    /// Bit `0x08`. The mixer says this channel is audible.
    ///
    /// **Never observed**: no DJM took part in any capture here, and a CDJ only
    /// believes it is on air because a mixer told it so. Named from the
    /// literature so a log can print it.
    pub const ON_AIR: u8 = 0x08;

    /// The deck is producing sound.
    pub fn is_playing(self) -> bool {
        self.0 & Self::PLAYING != 0
    }

    /// The deck claims tempo master.
    pub fn is_tempo_master(self) -> bool {
        self.0 & Self::TEMPO_MASTER != 0
    }

    /// SYNC is lit.
    pub fn is_synced(self) -> bool {
        self.0 & Self::SYNC != 0
    }

    /// A mixer has told the deck its channel is audible.
    pub fn is_on_air(self) -> bool {
        self.0 & Self::ON_AIR != 0
    }
}

impl fmt::Debug for StatusFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut set = f.debug_set();
        for (bit, name) in [
            (Self::PLAYING, "playing"),
            (Self::TEMPO_MASTER, "master"),
            (Self::SYNC, "sync"),
            (Self::ON_AIR, "on_air"),
        ] {
            if self.0 & bit != 0 {
                set.entry(&name);
            }
        }
        set.finish()
    }
}

/// `0x0a` kind byte and at least [`CdjStatus::MIN_LEN`] bytes, which is what
/// makes the accessors below that offset total.
///
/// 284 bytes on firmware 1.44. **Length does not identify the generation** — a
/// plain CDJ-2000nexus sends the `0x11c` length that reference documentation
/// maps to "Nexus 2" (F22).
#[derive(Clone, PartialEq, Eq)]
pub struct CdjStatus {
    raw: Vec<u8>,
}

impl CdjStatus {
    /// The shortest packet worth decoding.
    ///
    /// The media fields — which are the reason this packet matters — live below
    /// `0x76`, so this is the real floor. Reference implementations discard
    /// anything under `0xc8` as a truncated rekordbox status, which would throw
    /// away slot occupancy for no gain.
    pub const MIN_LEN: usize = 0x76;

    const OFF_DEVICE_2: usize = 0x24;
    const OFF_SOURCE_PLAYER: usize = 0x28;
    const OFF_SOURCE_SLOT: usize = 0x29;
    const OFF_TRACK_TYPE: usize = 0x2a;
    const OFF_TRACK_ID: usize = 0x2c;
    const OFF_USB_STATE: usize = 0x6f;
    const OFF_SD_STATE: usize = 0x73;
    const OFF_LINK_AVAILABLE: usize = 0x75;
    const OFF_PLAY_STATE: usize = 0x7b;
    const OFF_FIRMWARE: usize = 0x7c;
    const OFF_BROWSE_LIST_SIZE: usize = 0x46;
    const OFF_FLAGS: usize = 0x89;
    /// A second play-state byte, and **not** a copy of `0x7b`.
    ///
    /// `0xfa` while the deck is producing sound and `0xfe` otherwise, across
    /// every one of the 25,000 status packets in this corpus bar 120 caught
    /// mid-transition. A deck that leaves this at the idle value while claiming
    /// to play is describing something no hardware has ever sent.
    const OFF_PLAY_STATE_3: usize = 0x8b;
    /// A third statement of what the deck is doing, as a small bit field.
    ///
    /// `0x00` with no track, `0x01` with one loaded, `0x09` while playing —
    /// across all 25,766 status packets in this corpus, which reads as bit 0
    /// "a track is loaded" and bit 3 "sound is coming out". Four packets carry
    /// `0x0d`, a fifth bit we cannot name and do not set.
    const OFF_LOADED_AND_PLAYING: usize = 0x9d;
    /// Bit 0 of `0x9d`.
    const LOADED_BIT: u8 = 0x01;
    /// Bit 3 of `0x9d`.
    const PLAYING_BIT: u8 = 0x08;

    /// `0x8000` when the deck has a tempo, `0x7fff` when it does not.
    ///
    /// Perfectly correlated with the BPM field being something other than the
    /// `0xffff` sentinel — 16,192 captured packets with no tempo all carry
    /// `0x7fff` and every packet with one carries `0x8000`. It reads like the
    /// "is my tempo meaningful" flag a follower checks before locking to a
    /// master, which is what makes it worth writing rather than inheriting.
    const OFF_TEMPO_VALID: usize = 0x90;
    const OFF_PITCH: usize = 0x8c;
    const OFF_BPM: usize = 0x92;
    const OFF_MASTER_MEANINGFUL: usize = 0x9e;
    const OFF_YIELDING_TO: usize = 0x9f;
    /// Beats since the start of the track, counting from 1.
    const OFF_BEAT_NUMBER: usize = 0xa0;
    /// Which beat of the bar, 1–4. `0` when there is no bar to be in.
    const OFF_BEAT_IN_BAR: usize = 0xa6;
    const OFF_PACKET_COUNTER: usize = 0xc8;

    /// The pitch fader, four times over.
    ///
    /// A deck writes the same value to all four while it is playing, and zeroes
    /// the second and fourth while it is cued or paused (S06). We write all
    /// four, because nothing says which one a given follower reads and
    /// disagreement between them is a state no deck has ever produced.
    const OFF_PITCH_COPIES: [usize; 4] = [0x8c, 0x98, 0xc0, 0xc4];

    /// Byte `0xa0`–`0xa3` when the deck has no track: `0xffffffff`, not zero.
    const NO_BEAT_NUMBER: u32 = 0xffff_ffff;

    /// Parse a status packet, or fail if it is not one.
    pub fn parse(data: &[u8]) -> Result<Self> {
        check(data, StatusKind::CDJ_STATUS, Self::MIN_LEN)?;
        Ok(Self { raw: data.to_vec() })
    }

    header_accessors!();

    /// Start building one from a captured skeleton.
    pub fn builder() -> CdjStatusBuilder {
        CdjStatusBuilder::default()
    }

    /// Which player the loaded track came from; `None` when nothing is loaded.
    pub fn source_player(&self) -> Option<DeviceNumber> {
        DeviceNumber::new(self.guaranteed(Self::OFF_SOURCE_PLAYER))
    }

    /// Which of that player's slots the loaded track came from.
    pub fn source_slot(&self) -> Slot {
        Slot(self.guaranteed(Self::OFF_SOURCE_SLOT))
    }

    /// `1` for a rekordbox track, `2` for an unanalysed file.
    pub fn track_type(&self) -> u8 {
        self.guaranteed(Self::OFF_TRACK_TYPE)
    }

    /// The loaded track's row id, or `0` when nothing is loaded.
    pub fn track_id(&self) -> u32 {
        be_u32_at(&self.raw, Self::OFF_TRACK_ID).unwrap_or(0)
    }

    /// The USB slot's state. **Media presence is published here and nowhere
    /// else** (F20).
    pub fn usb_state(&self) -> MediaState {
        MediaState(self.guaranteed(Self::OFF_USB_STATE))
    }

    /// The SD slot's state.
    pub fn sd_state(&self) -> MediaState {
        MediaState(self.guaranteed(Self::OFF_SD_STATE))
    }

    /// The state of one slot. `None` for a slot this packet does not describe.
    pub fn slot_state(&self, slot: Slot) -> Option<MediaState> {
        match slot {
            Slot::USB => Some(self.usb_state()),
            Slot::SD => Some(self.sd_state()),
            _ => None,
        }
    }

    /// Byte `0x75`: set when any media is available anywhere on the network.
    pub fn link_available(&self) -> bool {
        self.guaranteed(Self::OFF_LINK_AVAILABLE) != 0
    }

    /// Byte `0x7b`, the play state. `None` on a packet too short to carry it.
    pub fn play_state(&self) -> Option<u8> {
        byte_at(&self.raw, Self::OFF_PLAY_STATE)
    }

    /// The four-character firmware string, e.g. `1.44`.
    pub fn firmware(&self) -> Option<String> {
        let field = self.raw.get(Self::OFF_FIRMWARE..Self::OFF_FIRMWARE + 4)?;
        Some(
            field
                .iter()
                .copied()
                .take_while(|&b| b != 0)
                .map(char::from)
                .collect(),
        )
    }

    /// `0x46`–`0x47`: how many rows are in the list this player is *showing*.
    ///
    /// The sending deck's own browse UI, not anything about its media. It is
    /// the item count a dbserver menu reply gave that deck: across the corpus
    /// every non-zero value here is a count that same deck had been told as a
    /// client — 651 while it browsed a whole track list, 15 and then 13 while
    /// it opened two albums, 1 for a one-item list (F57).
    ///
    /// `None` when the field is zero, which is what a player with nothing on
    /// screen sends and what this library sends always: it serves media and
    /// browses nothing, so it has no list to report. That asymmetry is the
    /// largest single difference between our status packet and a real deck's,
    /// and it is correct.
    pub fn browse_list_size(&self) -> Option<u16> {
        be_u16_at(&self.raw, Self::OFF_BROWSE_LIST_SIZE).filter(|size| *size != 0)
    }

    /// Byte `0x89`: playing, master, sync and on-air, as one field.
    ///
    /// `None` on a packet too short to carry it.
    pub fn flags(&self) -> Option<StatusFlags> {
        byte_at(&self.raw, Self::OFF_FLAGS).map(StatusFlags)
    }

    /// `0x8c`–`0x8f`, the pitch fader as a multiplier.
    ///
    /// The tempo the deck is actually producing is this times [`Self::bpm`],
    /// which is what [`Self::effective_bpm`] returns. While a follower is
    /// synced this field is not the DJ's fader position at all — the deck slews
    /// it continuously to hold its effective tempo equal to the master's.
    pub fn pitch(&self) -> Option<Pitch> {
        be_u32_at(&self.raw, Self::OFF_PITCH).map(Pitch)
    }

    /// Tempo in centi-BPM, before the pitch fader is applied.
    ///
    /// `None` both on a packet too short to carry it and on the `0xffff`
    /// sentinel a deck sends when it has no track and therefore no tempo —
    /// 31,424 of the 46,012 status packets in this corpus. Reading that
    /// sentinel as a number gives 655.35 BPM (F49).
    pub fn bpm_centi(&self) -> Option<u16> {
        be_u16_at(&self.raw, Self::OFF_BPM).filter(|&raw| raw != NO_TEMPO)
    }

    /// The track's own tempo, before the pitch fader.
    pub fn bpm(&self) -> Option<f64> {
        self.bpm_centi().map(|centi| f64::from(centi) / 100.0)
    }

    /// The tempo actually playing: the track's tempo with the fader applied.
    ///
    /// `None` when either half is missing, rather than a tempo derived from a
    /// default that would look like a measurement.
    pub fn effective_bpm(&self) -> Option<f64> {
        Some(self.bpm()? * self.pitch()?.multiplier())
    }

    /// Whether this player currently holds tempo master.
    ///
    /// Byte `0x9e` is `1` for a master with a usable beat grid and `2` for one
    /// without — the `2` form appears 309 times in this corpus and only while
    /// playing unanalysed files (`captures/S11-format-matrix`). Anything
    /// non-zero is the master. This is the only place mastership is published,
    /// so a device that never announces can never know who the master is.
    ///
    /// It agrees with [`StatusFlags::is_tempo_master`] in 46,011 of the 46,012
    /// captured packets; the one disagreement is a single frame inside a
    /// handoff. Prefer this byte, which is what the other decks act on.
    pub fn is_tempo_master(&self) -> Option<bool> {
        byte_at(&self.raw, Self::OFF_MASTER_MEANINGFUL).map(|byte| byte != 0)
    }

    /// The device this master is handing mastership to, if it is mid-handoff.
    ///
    /// Byte `0x9f`, which is `0xff` — no handoff — in 46,003 of the 46,012
    /// captured packets. In the other nine it names the device that sent the
    /// [`crate::beat::MasterRequest`], and it is set for the one or two packets
    /// during which **both** decks report themselves master. A follower that
    /// takes `is_tempo_master` at face value without consulting this will see
    /// mastership flicker between two devices.
    pub fn yielding_to(&self) -> Option<DeviceNumber> {
        let target = byte_at(&self.raw, Self::OFF_YIELDING_TO)?;
        if target == NO_YIELD {
            return None;
        }
        DeviceNumber::new(target)
    }

    /// Beats since the start of the track, counting the first beat as 1.
    ///
    /// `None` when the deck has no beat grid to count against, which it reports
    /// as `0xffffffff` — a value that reads as four billion if it is taken at
    /// face value.
    pub fn beat_number(&self) -> Option<u32> {
        be_u32_at(&self.raw, Self::OFF_BEAT_NUMBER).filter(|&raw| raw != Self::NO_BEAT_NUMBER)
    }

    /// Which beat of the bar the deck is on, 1–4.
    ///
    /// `None` off the grid: byte `0xa6` is then `0`, which means "no bar to be
    /// in" rather than "beat zero" — the same convention as the beat packet's
    /// byte `0x5c`.
    pub fn beat_in_bar(&self) -> Option<BeatInBar> {
        BeatInBar::new(byte_at(&self.raw, Self::OFF_BEAT_IN_BAR)?)
    }

    /// The sender's monotonic packet counter.
    pub fn packet_counter(&self) -> Option<u32> {
        be_u32_at(&self.raw, Self::OFF_PACKET_COUNTER)
    }

    /// A byte the constructor has already proven is present.
    fn guaranteed(&self, offset: usize) -> u8 {
        debug_assert!(
            offset < Self::MIN_LEN,
            "offset {offset:#x} is past the guaranteed prefix"
        );
        byte_at(&self.raw, offset).unwrap_or(0)
    }
}

impl fmt::Debug for CdjStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdjStatus")
            .field("sender", &self.sender())
            .field("name", &self.name())
            .field("usb", &self.usb_state())
            .field("sd", &self.sd_state())
            .field("link_available", &self.link_available())
            .field("track_id", &self.track_id())
            .field("bytes", &self.raw.len())
            .finish()
    }
}

/// Write one byte, if the buffer is long enough to hold it.
///
/// Free functions rather than closures over the buffer: several of these have
/// to run between reads of the same buffer, and a closure that captured it
/// would hold the borrow across all of them.
fn put(raw: &mut [u8], offset: usize, value: u8) {
    if let Some(slot) = raw.get_mut(offset) {
        *slot = value;
    }
}

fn put_u16(raw: &mut [u8], offset: usize, value: u16) {
    if let Some(field) = raw.get_mut(offset..offset.saturating_add(2)) {
        field.copy_from_slice(&value.to_be_bytes());
    }
}

fn put_u32(raw: &mut [u8], offset: usize, value: u32) {
    if let Some(field) = raw.get_mut(offset..offset.saturating_add(4)) {
        field.copy_from_slice(&value.to_be_bytes());
    }
}

/// Builds the status packet a virtual CDJ emits.
///
/// Starts from a captured skeleton and substitutes only understood fields; the
/// ~260 bytes we cannot name are reproduced exactly (F23).
#[derive(Clone, Debug)]
// Six of them, and each is one wire bit that a caller sets independently. A
// struct of enums here would be six enums.
#[allow(clippy::struct_excessive_bools)]
pub struct CdjStatusBuilder {
    device_number: u8,
    name: DeviceName,
    usb_state: MediaState,
    sd_state: MediaState,
    link_available: bool,
    play_state: u8,
    firmware: String,
    packet_counter: u32,
    bpm_centi: Option<u16>,
    pitch: Pitch,
    playing: bool,
    on_air: bool,
    synced: bool,
    tempo_master: bool,
    yielding_to: Option<DeviceNumber>,
    beat_number: Option<u32>,
    beat_in_bar: Option<BeatInBar>,
    loaded_track: Option<LoadedTrack>,
}

/// What a deck is playing, and whose medium it came from.
///
/// **A deck that states a tempo always states this too.** Across 25,766
/// captured status packets, bytes `0x28`-`0x2f` are zero only while byte `0x92`
/// holds the no-tempo sentinel, and non-zero in every packet that carries a
/// real tempo. Announcing a tempo with no track is a combination no hardware
/// has ever sent, and a deck asked to follow such a master ignores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadedTrack {
    /// Whose medium it came from — the deck's own number when it is playing
    /// off its own stick, and another player's when playing over the link.
    pub source_player: DeviceNumber,
    /// Which of that player's slots.
    pub slot: Slot,
    /// The rekordbox row id.
    pub id: u32,
}

impl Default for CdjStatusBuilder {
    fn default() -> Self {
        Self {
            device_number: 0,
            name: DeviceName::default(),
            usb_state: MediaState::EMPTY,
            sd_state: MediaState::EMPTY,
            link_available: false,
            play_state: 0x00,
            firmware: "1.44".to_owned(),
            packet_counter: 0,
            bpm_centi: None,
            pitch: Pitch::UNITY,
            playing: false,
            on_air: false,
            synced: false,
            tempo_master: false,
            yielding_to: None,
            beat_number: None,
            beat_in_bar: None,
            loaded_track: None,
        }
    }
}

impl CdjStatusBuilder {
    /// The number we announce as. Appears twice in the packet, at `0x21` and
    /// `0x24`.
    #[must_use]
    pub fn device_number(mut self, number: DeviceNumber) -> Self {
        self.device_number = number.get();
        self
    }

    /// The name we announce as.
    #[must_use]
    pub fn name(mut self, name: DeviceName) -> Self {
        self.name = name;
        self
    }

    /// What to say about a slot. A slot reported [`MediaState::EMPTY`] is a slot
    /// no player will ever ask about (F20).
    #[must_use]
    pub fn slot_state(mut self, slot: Slot, state: MediaState) -> Self {
        match slot {
            Slot::USB => self.usb_state = state,
            Slot::SD => self.sd_state = state,
            _ => {}
        }
        self
    }

    /// Whether any media is available anywhere on the network.
    #[must_use]
    pub fn link_available(mut self, available: bool) -> Self {
        self.link_available = available;
        self
    }

    /// The play state byte.
    #[must_use]
    pub fn play_state(mut self, state: u8) -> Self {
        self.play_state = state;
        self
    }

    /// The firmware string, truncated to four bytes.
    #[must_use]
    pub fn firmware(mut self, firmware: &str) -> Self {
        firmware.clone_into(&mut self.firmware);
        self
    }

    /// The counter, which increments once per packet on real hardware.
    ///
    /// Passed in rather than held as builder state so that emission stays a pure
    /// function of its inputs.
    #[must_use]
    pub fn packet_counter(mut self, counter: u32) -> Self {
        self.packet_counter = counter;
        self
    }

    /// The tempo: the track's own, before the fader, and the fader itself.
    ///
    /// `None` leaves the `0xffff` a deck sends with no track loaded, which is
    /// the only value that means "no tempo" — a zero here reads as 0.00 BPM and
    /// is a measurement rather than an absence.
    #[must_use]
    pub fn tempo(mut self, bpm_centi: Option<u16>, pitch: Pitch) -> Self {
        self.bpm_centi = bpm_centi.filter(|&centi| centi != NO_TEMPO);
        self.pitch = pitch;
        self
    }

    /// Whether sound is coming out, which is byte `0x89` bit 6.
    ///
    /// Not a restatement of [`Self::play_state`]; see
    /// [`StatusFlags::is_playing`].
    #[must_use]
    pub fn playing(mut self, playing: bool) -> Self {
        self.playing = playing;
        self
    }

    /// Whether the mixer says this channel is audible, byte `0x89` bit 3.
    #[must_use]
    pub fn on_air(mut self, on_air: bool) -> Self {
        self.on_air = on_air;
        self
    }

    /// Whether SYNC is engaged, byte `0x89` bit 4.
    #[must_use]
    pub fn synced(mut self, synced: bool) -> Self {
        self.synced = synced;
        self
    }

    /// Whether we hold tempo master.
    ///
    /// Sets both byte `0x9e` and flag bit 5, which a real deck keeps in step:
    /// they agreed in 46,011 of the 46,012 captured packets, and the one
    /// exception is a single frame inside a handoff.
    #[must_use]
    pub fn tempo_master(mut self, master: bool) -> Self {
        self.tempo_master = master;
        self
    }

    /// The device this master is handing mastership to, byte `0x9f`.
    ///
    /// **This is the grant.** A deck asking for mastership watches for its own
    /// number to appear here; the holder's claim going away is not a grant and
    /// a real CDJ does not act on one. A handover that only drops byte `0x9e`
    /// therefore looks to the requester exactly like a refusal — it asked, the
    /// holder went quiet, and nobody ended up master.
    ///
    /// So it is set *with* `tempo_master(true)` for the packets between the
    /// answer and the requester picking mastership up, and cleared with the
    /// claim afterwards. `None` writes the `0xff` that means no handoff, which
    /// is what 46,003 of the 46,012 captured packets carry.
    #[must_use]
    pub fn yielding_to(mut self, successor: Option<DeviceNumber>) -> Self {
        self.yielding_to = successor;
        self
    }

    /// What is loaded, and whose medium it came from.
    ///
    /// `None` is "nothing loaded", and it is the only state in which a tempo
    /// may be absent — the two go together on real hardware and a follower
    /// treats a master with no track as having nothing to follow.
    #[must_use]
    pub fn loaded_track(mut self, track: Option<LoadedTrack>) -> Self {
        self.loaded_track = track;
        self
    }

    /// Where the playhead is on the grid: beats since the start of the track,
    /// counting from 1, and which beat of the bar that is.
    ///
    /// `None` for either is "off the grid", and both are written as the
    /// sentinels a real deck uses rather than as zeros.
    #[must_use]
    pub fn beat(mut self, number: Option<u32>, in_bar: Option<BeatInBar>) -> Self {
        self.beat_number = number;
        self.beat_in_bar = in_bar;
        self
    }

    /// Everything that says what the deck is doing: the play-state bytes, the
    /// tempo, the flags, the grid position and the loaded track.
    ///
    /// Split out from `build` because they are a *set* -- a deck keeps them
    /// consistent and a follower reads several of them, so leaving any one at
    /// the skeleton's idle value describes a deck that has never existed.
    fn write_transport(&self, raw: &mut [u8]) {
        put(raw, CdjStatus::OFF_PLAY_STATE, self.play_state);
        // The other two bytes that say the same thing in different words. A
        // real deck keeps all three in step and a follower reads more than one
        // of them: leaving these at the skeleton's idle values while byte 0x7b
        // said "playing" is what made a CDJ draw our phase and refuse to
        // follow our tempo.
        put(
            raw,
            CdjStatus::OFF_PLAY_STATE_3,
            if self.playing { 0xfa } else { 0xfe },
        );
        put_u16(
            raw,
            CdjStatus::OFF_TEMPO_VALID,
            if self.bpm_centi.is_some() {
                0x8000
            } else {
                0x7fff
            },
        );

        // Bit 7 of the flags byte is set in every captured packet and has no
        // known meaning, so it is kept from the template rather than rebuilt.
        let mut flags = raw.get(CdjStatus::OFF_FLAGS).copied().unwrap_or(0x84);
        for (bit, set) in [
            (StatusFlags::PLAYING, self.playing),
            (StatusFlags::TEMPO_MASTER, self.tempo_master),
            (StatusFlags::SYNC, self.synced),
            (StatusFlags::ON_AIR, self.on_air),
        ] {
            if set {
                flags |= bit;
            } else {
                flags &= !bit;
            }
        }
        put(raw, CdjStatus::OFF_FLAGS, flags);
        put(
            raw,
            CdjStatus::OFF_MASTER_MEANINGFUL,
            u8::from(self.tempo_master),
        );
        // Written every packet rather than left to the skeleton, for the same
        // reason as the transport bytes above: the skeleton is one frame from
        // one deck, and a field only ever written when it is interesting is a
        // field that keeps whatever the last interesting packet left in it.
        put(
            raw,
            CdjStatus::OFF_YIELDING_TO,
            self.yielding_to.map_or(NO_YIELD, DeviceNumber::get),
        );

        for offset in CdjStatus::OFF_PITCH_COPIES {
            put_u32(raw, offset, self.pitch.0);
        }
        put_u32(
            raw,
            CdjStatus::OFF_BEAT_NUMBER,
            self.beat_number.unwrap_or(CdjStatus::NO_BEAT_NUMBER),
        );
        put_u16(raw, CdjStatus::OFF_BPM, self.bpm_centi.unwrap_or(NO_TEMPO));
        put(
            raw,
            CdjStatus::OFF_BEAT_IN_BAR,
            self.beat_in_bar.map_or(0, BeatInBar::get),
        );

        // What is loaded. Zeroed rather than left as the skeleton's when
        // nothing is, because the skeleton came from a deck with a track in it.
        put(
            raw,
            CdjStatus::OFF_SOURCE_PLAYER,
            self.loaded_track
                .map_or(0, |track| track.source_player.get()),
        );
        put(
            raw,
            CdjStatus::OFF_SOURCE_SLOT,
            self.loaded_track.map_or(0, |track| track.slot.0),
        );
        // `1` is a rekordbox track. Everything this library serves or plays is
        // one; `2`, an unanalysed file, would be a claim we cannot support.
        put(
            raw,
            CdjStatus::OFF_TRACK_TYPE,
            u8::from(self.loaded_track.is_some()),
        );
        put_u32(
            raw,
            CdjStatus::OFF_TRACK_ID,
            self.loaded_track.map_or(0, |track| track.id),
        );

        let mut loaded_and_playing = 0u8;
        if self.loaded_track.is_some() || self.bpm_centi.is_some() {
            loaded_and_playing |= CdjStatus::LOADED_BIT;
        }
        if self.playing {
            loaded_and_playing |= CdjStatus::PLAYING_BIT;
        }
        put(raw, CdjStatus::OFF_LOADED_AND_PLAYING, loaded_and_playing);
    }

    /// Produce the packet.
    pub fn build(&self) -> CdjStatus {
        let mut raw = templates::CDJ_STATUS.to_vec();
        write_header(
            &mut raw,
            StatusKind::CDJ_STATUS,
            self.name,
            self.device_number,
        );
        // The device number appears at 0x21 and again at 0x24.
        put(&mut raw, CdjStatus::OFF_DEVICE_2, self.device_number);
        put(&mut raw, CdjStatus::OFF_USB_STATE, self.usb_state.0);
        put(&mut raw, CdjStatus::OFF_SD_STATE, self.sd_state.0);
        // 0x74 is left exactly as the real deck sent it. It takes 0 and 1 and is
        // clearly media-related, but it does not track 0x75: three of the four
        // combinations occur, so it is a separate flag we cannot yet name.
        put(
            &mut raw,
            CdjStatus::OFF_LINK_AVAILABLE,
            u8::from(self.link_available),
        );
        for (index, byte) in self.firmware.bytes().take(4).enumerate() {
            put(&mut raw, CdjStatus::OFF_FIRMWARE + index, byte);
        }
        put_u32(&mut raw, CdjStatus::OFF_PACKET_COUNTER, self.packet_counter);
        self.write_transport(&mut raw);
        CdjStatus { raw }
    }
}

// -- media query (0x05) ---------------------------------------------------

/// "Device *target*, describe slot *slot*."
///
/// **The step no reference implementation performs, because none of them
/// serve.** A deck asks what a slot actually contains and will not offer a
/// medium it believes is empty — announcing and emitting status are not enough
/// (F24). One query per slot, issued when a deck first browses it, and not
/// repeated (F37).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaQuery {
    /// The device asking, from byte `0x21`.
    pub requester: DeviceNumber,
    /// Where to send the answer. The requester names itself by address as well
    /// as by number.
    pub requester_ip: Ipv4Addr,
    /// The device being asked about.
    pub target: DeviceNumber,
    /// The slot being asked about.
    pub slot: Slot,
}

impl MediaQuery {
    /// Bytes on the wire.
    pub const LEN: usize = 0x30;

    const OFF_REQUESTER_IP: usize = 0x24;
    const OFF_TARGET: usize = 0x28;
    const OFF_SLOT: usize = 0x2c;

    /// Parse a media query.
    pub fn parse(data: &[u8]) -> Result<Self> {
        check(data, StatusKind::MEDIA_QUERY, Self::LEN)?;
        let requester = DeviceNumber::new(byte_at(data, OFF_SENDER).unwrap_or(0))
            .ok_or_else(|| Error::malformed(OFF_SENDER, "media query from device 0"))?;
        let target = be_u32_at(data, Self::OFF_TARGET)
            .and_then(|raw| u8::try_from(raw).ok())
            .and_then(DeviceNumber::new)
            .ok_or_else(|| Error::malformed(Self::OFF_TARGET, "media query for device 0"))?;
        let slot = be_u32_at(data, Self::OFF_SLOT)
            .and_then(|raw| u8::try_from(raw).ok())
            .map(Slot)
            .ok_or_else(|| Error::malformed(Self::OFF_SLOT, "slot number out of range"))?;
        let mut octets = [0u8; 4];
        if let Some(bytes) = data.get(Self::OFF_REQUESTER_IP..Self::OFF_REQUESTER_IP + 4) {
            octets.copy_from_slice(bytes);
        }
        Ok(Self {
            requester,
            requester_ip: Ipv4Addr::from(octets),
            target,
            slot,
        })
    }

    /// Encode this query.
    pub fn encode(&self, name: DeviceName) -> Vec<u8> {
        let mut raw = vec![0u8; Self::LEN];
        write_header(
            &mut raw,
            StatusKind::MEDIA_QUERY,
            name,
            self.requester.get(),
        );
        if let Some(field) = raw.get_mut(Self::OFF_REQUESTER_IP..Self::OFF_REQUESTER_IP + 4) {
            field.copy_from_slice(&self.requester_ip.octets());
        }
        if let Some(field) = raw.get_mut(Self::OFF_TARGET..Self::OFF_TARGET + 4) {
            field.copy_from_slice(&u32::from(self.target.get()).to_be_bytes());
        }
        if let Some(field) = raw.get_mut(Self::OFF_SLOT..Self::OFF_SLOT + 4) {
            field.copy_from_slice(&u32::from(self.slot.0).to_be_bytes());
        }
        raw
    }
}

// -- media response (0x06) ------------------------------------------------

/// A medium's description: its label, its counts and its size.
///
/// Answer with the **true** counts: a deck told there are no tracks has no
/// reason to offer the medium (F24).
#[derive(Clone, PartialEq, Eq)]
pub struct MediaResponse {
    raw: Vec<u8>,
}

impl MediaResponse {
    /// The shortest response carrying every field below.
    pub const MIN_LEN: usize = 0xb0;

    const OFF_DEVICE: usize = 0x24;
    const OFF_SLOT: usize = 0x28;
    const OFF_VOLUME_NAME: usize = 0x2c;
    const LEN_VOLUME_NAME: usize = 0x40;
    const OFF_CREATED: usize = 0x6c;
    const LEN_CREATED: usize = 0x18;
    const OFF_TRACK_COUNT: usize = 0xa4;
    const OFF_PLAYLIST_COUNT: usize = 0xac;
    const OFF_TOTAL_BYTES: usize = 0xb4;
    const OFF_FREE_BYTES: usize = 0xbc;

    /// Parse a media response.
    pub fn parse(data: &[u8]) -> Result<Self> {
        check(data, StatusKind::MEDIA_RESPONSE, Self::MIN_LEN)?;
        Ok(Self { raw: data.to_vec() })
    }

    header_accessors!();

    /// Start building one from a captured skeleton.
    pub fn builder() -> MediaResponseBuilder {
        MediaResponseBuilder::default()
    }

    /// The device this medium is in.
    pub fn device(&self) -> Option<DeviceNumber> {
        be_u32_at(&self.raw, Self::OFF_DEVICE)
            .and_then(|raw| u8::try_from(raw).ok())
            .and_then(DeviceNumber::new)
    }

    /// The slot this medium is in.
    pub fn slot(&self) -> Slot {
        Slot(u8::try_from(be_u32_at(&self.raw, Self::OFF_SLOT).unwrap_or(0) & 0xff).unwrap_or(0))
    }

    /// The volume label the DJ formatted the medium with, UTF-16 big-endian.
    ///
    /// **Often empty and legitimately so**: an unlabelled stick reports no name
    /// while carrying a full library, so emptiness here is not emptiness of the
    /// slot.
    pub fn volume_name(&self) -> String {
        utf16be_at(&self.raw, Self::OFF_VOLUME_NAME, Self::LEN_VOLUME_NAME)
    }

    /// The medium's creation date, e.g. `2025-06-24`.
    pub fn created(&self) -> String {
        utf16be_at(&self.raw, Self::OFF_CREATED, Self::LEN_CREATED)
    }

    /// How many tracks the medium holds.
    pub fn track_count(&self) -> u32 {
        be_u32_at(&self.raw, Self::OFF_TRACK_COUNT).unwrap_or(0)
    }

    /// How many playlists the medium holds.
    pub fn playlist_count(&self) -> u32 {
        be_u32_at(&self.raw, Self::OFF_PLAYLIST_COUNT).unwrap_or(0)
    }

    /// The medium's capacity in bytes.
    pub fn total_bytes(&self) -> Option<u32> {
        be_u32_at(&self.raw, Self::OFF_TOTAL_BYTES)
    }

    /// Free space in bytes.
    pub fn free_bytes(&self) -> Option<u32> {
        be_u32_at(&self.raw, Self::OFF_FREE_BYTES)
    }
}

impl fmt::Debug for MediaResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaResponse")
            .field("device", &self.device())
            .field("slot", &self.slot())
            .field("volume_name", &self.volume_name())
            .field("tracks", &self.track_count())
            .field("playlists", &self.playlist_count())
            .finish()
    }
}

/// Builds the answer to a [`MediaQuery`].
#[derive(Clone, Debug, Default)]
pub struct MediaResponseBuilder {
    device_number: u8,
    slot: u8,
    name: DeviceName,
    volume_name: String,
    track_count: u32,
    playlist_count: u32,
    total_bytes: Option<u32>,
    free_bytes: Option<u32>,
}

impl MediaResponseBuilder {
    /// Which device the medium is in — ours.
    #[must_use]
    pub fn device_number(mut self, number: DeviceNumber) -> Self {
        self.device_number = number.get();
        self
    }

    /// Which slot the medium is in.
    #[must_use]
    pub fn slot(mut self, slot: Slot) -> Self {
        self.slot = slot.0;
        self
    }

    /// The name we announce as.
    #[must_use]
    pub fn name(mut self, name: DeviceName) -> Self {
        self.name = name;
        self
    }

    /// The volume label to show on the deck.
    #[must_use]
    pub fn volume_name(mut self, label: &str) -> Self {
        label.clone_into(&mut self.volume_name);
        self
    }

    /// The true counts. A deck told a medium holds nothing will not offer it.
    #[must_use]
    pub fn counts(mut self, tracks: u32, playlists: u32) -> Self {
        self.track_count = tracks;
        self.playlist_count = playlists;
        self
    }

    /// Capacity and free space. Left as the skeleton's values when not set.
    #[must_use]
    pub fn size(mut self, total_bytes: u32, free_bytes: u32) -> Self {
        self.total_bytes = Some(total_bytes);
        self.free_bytes = Some(free_bytes);
        self
    }

    /// Produce the packet.
    pub fn build(&self) -> MediaResponse {
        let mut raw = templates::MEDIA_RESPONSE.to_vec();
        write_header(
            &mut raw,
            StatusKind::MEDIA_RESPONSE,
            self.name,
            self.device_number,
        );
        {
            let mut put_u32 = |offset: usize, value: u32| {
                if let Some(field) = raw.get_mut(offset..offset + 4) {
                    field.copy_from_slice(&value.to_be_bytes());
                }
            };
            put_u32(MediaResponse::OFF_DEVICE, u32::from(self.device_number));
            put_u32(MediaResponse::OFF_SLOT, u32::from(self.slot));
            put_u32(MediaResponse::OFF_TRACK_COUNT, self.track_count);
            put_u32(MediaResponse::OFF_PLAYLIST_COUNT, self.playlist_count);
            if let Some(total) = self.total_bytes {
                put_u32(MediaResponse::OFF_TOTAL_BYTES, total);
            }
            if let Some(free) = self.free_bytes {
                put_u32(MediaResponse::OFF_FREE_BYTES, free);
            }
        }

        if let Some(field) = raw.get_mut(
            MediaResponse::OFF_VOLUME_NAME
                ..MediaResponse::OFF_VOLUME_NAME + MediaResponse::LEN_VOLUME_NAME,
        ) {
            field.fill(0);
            for (pair, unit) in field
                .chunks_exact_mut(2)
                .zip(self.volume_name.encode_utf16())
            {
                pair.copy_from_slice(&unit.to_be_bytes());
            }
        }
        MediaResponse { raw }
    }
}

// -- device settings (0x35 / 0x36) ----------------------------------------

/// "Give me the settings on your slot N."
///
/// LOAD SETTINGS over LINK, and **it is not a file read** (F38). The requesting
/// deck mounts the NFS export, reads nothing from it, and asks here instead. A
/// server implementing only NFS sees a mount, concludes nothing was wanted, and
/// never learns a request was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsQuery {
    /// The device that wants the settings, from byte `0x24`.
    ///
    /// In a query this equals the sender; in a response it does not, which is
    /// how the two are told apart without looking at the kind byte.
    pub requester: DeviceNumber,
    /// Byte `0x21`.
    pub sender: DeviceNumber,
    /// The slot whose settings are wanted.
    ///
    /// There is no target field: the packet is unicast, so the destination
    /// address identifies whose medium is being asked about.
    pub slot: Slot,
}

impl SettingsQuery {
    /// Bytes on the wire.
    pub const LEN: usize = 0x28;

    const OFF_REQUESTER: usize = 0x24;
    const OFF_SLOT: usize = 0x25;

    /// Parse a settings query.
    pub fn parse(data: &[u8]) -> Result<Self> {
        check(data, StatusKind::SETTINGS_QUERY, Self::LEN)?;
        let requester = DeviceNumber::new(byte_at(data, Self::OFF_REQUESTER).unwrap_or(0))
            .ok_or_else(|| Error::malformed(Self::OFF_REQUESTER, "settings query from device 0"))?;
        let sender = DeviceNumber::new(byte_at(data, OFF_SENDER).unwrap_or(0)).unwrap_or(requester);
        Ok(Self {
            requester,
            sender,
            slot: Slot(byte_at(data, Self::OFF_SLOT).unwrap_or(0)),
        })
    }

    /// Encode this query.
    pub fn encode(&self, name: DeviceName) -> Vec<u8> {
        let mut raw = vec![0u8; Self::LEN];
        write_header(
            &mut raw,
            StatusKind::SETTINGS_QUERY,
            name,
            self.sender.get(),
        );
        if let Some(slot) = raw.get_mut(Self::OFF_REQUESTER) {
            *slot = self.requester.get();
        }
        if let Some(slot) = raw.get_mut(Self::OFF_SLOT) {
            *slot = self.slot.0;
        }
        raw
    }
}

/// The settings from a medium, carried inline.
///
/// The bytes come from `PIONEER/MYSETTING.DAT`, with the two leading words
/// byte-swapped to big-endian. They are deliberately not interpreted here: they
/// look like `0x80`-based enumerations but nothing in the research record maps
/// them to the named options on the deck's screen, and a server only has to hand
/// over what the medium holds.
#[derive(Clone, PartialEq, Eq)]
pub struct SettingsResponse {
    raw: Vec<u8>,
}

impl SettingsResponse {
    /// Bytes on the wire.
    pub const LEN: usize = 0x50;
    /// Bytes of settings a response carries.
    pub const PAYLOAD_LEN: usize = 32;

    /// Leads the settings block. Constant in the one exchange captured, and the
    /// same value that leads the payload of `PIONEER/MYSETTING.DAT`, which is
    /// what ties the file to the wire.
    pub const MAGIC: u32 = 0x1234_5678;

    const OFF_REQUESTER: usize = 0x24;
    const OFF_SLOT: usize = 0x25;
    const OFF_MAGIC: usize = 0x28;
    const OFF_UNKNOWN: usize = 0x2c;
    const OFF_PAYLOAD: usize = 0x30;

    /// Parse a settings response.
    pub fn parse(data: &[u8]) -> Result<Self> {
        check(data, StatusKind::SETTINGS_RESPONSE, Self::LEN)?;
        Ok(Self { raw: data.to_vec() })
    }

    header_accessors!();

    /// The device that asked.
    pub fn requester(&self) -> Option<DeviceNumber> {
        DeviceNumber::new(byte_at(&self.raw, Self::OFF_REQUESTER).unwrap_or(0))
    }

    /// The slot the settings came from.
    pub fn slot(&self) -> Slot {
        Slot(byte_at(&self.raw, Self::OFF_SLOT).unwrap_or(0))
    }

    /// The 32 settings bytes.
    pub fn payload(&self) -> &[u8] {
        self.raw
            .get(Self::OFF_PAYLOAD..Self::OFF_PAYLOAD + Self::PAYLOAD_LEN)
            .unwrap_or(&[])
    }

    /// Build a response carrying `settings` verbatim, padded or truncated to
    /// [`Self::PAYLOAD_LEN`].
    ///
    /// An empty block is a legitimate answer — a medium with no saved settings —
    /// so a missing file is not an error.
    pub fn build(
        name: DeviceName,
        sender: DeviceNumber,
        requester: DeviceNumber,
        slot: Slot,
        settings: &[u8],
    ) -> Self {
        let mut raw = vec![0u8; Self::LEN];
        write_header(&mut raw, StatusKind::SETTINGS_RESPONSE, name, sender.get());
        if let Some(byte) = raw.get_mut(Self::OFF_REQUESTER) {
            *byte = requester.get();
        }
        if let Some(byte) = raw.get_mut(Self::OFF_SLOT) {
            *byte = slot.0;
        }
        if let Some(field) = raw.get_mut(Self::OFF_MAGIC..Self::OFF_MAGIC + 4) {
            field.copy_from_slice(&Self::MAGIC.to_be_bytes());
        }
        if let Some(field) = raw.get_mut(Self::OFF_UNKNOWN..Self::OFF_UNKNOWN + 4) {
            field.copy_from_slice(&2u32.to_be_bytes());
        }
        if let Some(field) = raw.get_mut(Self::OFF_PAYLOAD..Self::OFF_PAYLOAD + Self::PAYLOAD_LEN) {
            for (byte, value) in field.iter_mut().zip(settings.iter().copied()) {
                *byte = value;
            }
        }
        Self { raw }
    }
}

impl fmt::Debug for SettingsResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SettingsResponse")
            .field("sender", &self.sender())
            .field("requester", &self.requester())
            .field("slot", &self.slot())
            .field("payload_len", &self.payload().len())
            .finish()
    }
}

/// The check every constructor on this port performs, once.
fn check(data: &[u8], kind: StatusKind, min_len: usize) -> Result<()> {
    check_header(data, kind.0, min_len).map_err(|error| match error {
        // Restore the named form in the message: `0x0a` reads as `cdj_status`
        // on this port and as nothing at all on 50001.
        Error::Malformed { at, .. } if at == MAGIC.len() => Error::malformed(
            at,
            format!(
                "expected {kind:?} ({:#04x}), got {:#04x}",
                kind.0,
                byte_at(data, MAGIC.len()).unwrap_or(0)
            ),
        ),
        other => other,
    })
}

/// The magic, kind and minimum-length check shared by UDP 50001 and 50002.
///
/// Performed **once**, by a constructor, which is what makes every accessor
/// below `min_len` total.
pub(crate) fn check_header(data: &[u8], kind: u8, min_len: usize) -> Result<()> {
    if data.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
        let got = data.get(..MAGIC.len()).unwrap_or(data);
        return Err(Error::BadMagic {
            expected: MAGIC.as_slice().into(),
            got: got.into(),
        });
    }
    match byte_at(data, MAGIC.len()) {
        Some(actual) if actual == kind => {}
        Some(actual) => {
            return Err(Error::malformed(
                MAGIC.len(),
                format!("expected kind {kind:#04x}, got {actual:#04x}"),
            ));
        }
        None => {
            return Err(Error::Truncated {
                need: MAGIC.len() + 1,
                at: 0,
                have: data.len(),
            });
        }
    }
    if data.len() < min_len {
        return Err(Error::Truncated {
            need: min_len,
            at: 0,
            have: data.len(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_browse_list_size_is_a_menu_count_the_sender_was_told() {
        // 0x46 is the sending deck's own browse UI: how many rows are on its
        // screen. Every non-zero value in the corpus is an item count that
        // deck had been given by a dbserver menu reply (F57).
        //
        // It is zero in two thirds of the corpus and zero in everything this
        // library sends, because a server browses nothing.
        let Some(corpus) = prolink_capture::Corpus::locate() else {
            return;
        };
        let mut sizes = std::collections::BTreeSet::new();
        let mut zero = 0usize;
        for path in corpus.captures() {
            let Ok(capture) = prolink_capture::Capture::open(&path) else {
                continue;
            };
            for packet in capture.udp_to(crate::STATUS_PORT).flatten() {
                let Ok(status) = CdjStatus::parse(&packet.payload) else {
                    continue;
                };
                match status.browse_list_size() {
                    Some(size) => {
                        sizes.insert(size);
                    }
                    None => zero += 1,
                }
            }
        }
        if sizes.is_empty() && zero == 0 {
            return;
        }
        assert!(zero > 1000, "the field is usually zero; {zero} were");
        // The whole 651-track list, and the 40-track format stick, both of
        // which are counts a deck was answered with in those sessions.
        assert!(sizes.contains(&651), "651 is missing from {sizes:?}");
        assert!(sizes.contains(&40), "40 is missing from {sizes:?}");
        assert!(
            sizes.iter().all(|size| *size <= 1000),
            "a list size should be a plausible menu count: {sizes:?}"
        );
    }
    use super::*;

    fn device(number: u8) -> DeviceNumber {
        DeviceNumber::new(number).expect("a non-zero device number")
    }

    #[test]
    fn a_built_status_parses_back() {
        let built = CdjStatus::builder()
            .device_number(device(5))
            .name(DeviceName::new("CDJ-2000nexus"))
            .slot_state(Slot::USB, MediaState::LOADED)
            .slot_state(Slot::SD, MediaState::EMPTY)
            .link_available(true)
            .packet_counter(0x0102_0304)
            .build();

        assert_eq!(
            built.as_bytes().len(),
            284,
            "the length a firmware-1.44 deck sends"
        );
        let parsed = CdjStatus::parse(built.as_bytes()).unwrap();
        assert_eq!(parsed.sender(), Some(device(5)));
        assert_eq!(parsed.name().as_str(), "CDJ-2000nexus");
        assert_eq!(parsed.usb_state(), MediaState::LOADED);
        assert_eq!(parsed.sd_state(), MediaState::EMPTY);
        assert!(parsed.link_available());
        assert_eq!(parsed.firmware().as_deref(), Some("1.44"));
        assert_eq!(parsed.packet_counter(), Some(0x0102_0304));
    }

    #[test]
    fn a_status_differs_from_its_skeleton_only_where_we_substituted() {
        let built = CdjStatus::builder()
            .device_number(device(3))
            .slot_state(Slot::USB, MediaState::LOADED)
            .link_available(true)
            .build();
        let differing: Vec<usize> = built
            .as_bytes()
            .iter()
            .zip(templates::CDJ_STATUS.iter())
            .enumerate()
            .filter(|(_, (ours, theirs))| ours != theirs)
            .map(|(offset, _)| offset)
            .collect();
        // Everything the builder claims to substitute, and nothing else. The
        // remaining ~260 bytes are the real deck's, untouched — which is the
        // difference between plausible and indistinguishable (F23).
        let substituted: Vec<usize> = (OFF_NAME..OFF_NAME + DeviceName::LEN)
            .chain([
                OFF_CONST_ONE,
                OFF_SENDER,
                CdjStatus::OFF_DEVICE_2,
                CdjStatus::OFF_USB_STATE,
                CdjStatus::OFF_SD_STATE,
                CdjStatus::OFF_LINK_AVAILABLE,
                CdjStatus::OFF_PLAY_STATE,
                CdjStatus::OFF_FLAGS,
                CdjStatus::OFF_MASTER_MEANINGFUL,
                CdjStatus::OFF_BEAT_IN_BAR,
                CdjStatus::OFF_PLAY_STATE_3,
                CdjStatus::OFF_LOADED_AND_PLAYING,
                CdjStatus::OFF_SOURCE_PLAYER,
                CdjStatus::OFF_SOURCE_SLOT,
                CdjStatus::OFF_TRACK_TYPE,
            ])
            .chain(CdjStatus::OFF_FIRMWARE..CdjStatus::OFF_FIRMWARE + 4)
            .chain(CdjStatus::OFF_PACKET_COUNTER..CdjStatus::OFF_PACKET_COUNTER + 4)
            .chain(CdjStatus::OFF_BPM..CdjStatus::OFF_BPM + 2)
            .chain(CdjStatus::OFF_TEMPO_VALID..CdjStatus::OFF_TEMPO_VALID + 2)
            .chain(CdjStatus::OFF_BEAT_NUMBER..CdjStatus::OFF_BEAT_NUMBER + 4)
            .chain(CdjStatus::OFF_TRACK_ID..CdjStatus::OFF_TRACK_ID + 4)
            .chain(
                CdjStatus::OFF_PITCH_COPIES
                    .into_iter()
                    .flat_map(|offset| offset..offset + 4),
            )
            .chain(OFF_BODY_LEN..OFF_BODY_LEN + 2)
            .collect();
        let unexpected: Vec<usize> = differing
            .iter()
            .copied()
            .filter(|offset| !substituted.contains(offset))
            .collect();
        assert!(
            unexpected.is_empty(),
            "disturbed bytes we do not understand: {unexpected:x?}"
        );
    }

    #[test]
    fn the_three_play_state_bytes_agree_with_each_other() {
        // A follower reads more than one of them. Byte 0x7b saying "playing"
        // while 0x8b and 0x90 still hold the skeleton's idle values describes a
        // deck no hardware has ever been, and a CDJ that saw it drew our phase
        // and would not lock to our tempo.
        let playing = CdjStatus::builder()
            .play_state(0x03)
            .playing(true)
            .tempo(Some(14_110), Pitch::UNITY)
            .build();
        assert_eq!(byte_at(playing.as_bytes(), 0x8b), Some(0xfa));
        assert_eq!(be_u16_at(playing.as_bytes(), 0x90), Some(0x8000));

        let idle = CdjStatus::builder().build();
        assert_eq!(byte_at(idle.as_bytes(), 0x8b), Some(0xfe));
        assert_eq!(be_u16_at(idle.as_bytes(), 0x90), Some(0x7fff));

        // Loaded but stopped: it has a tempo to state and is making no sound,
        // and the two bytes say those two different things.
        let paused = CdjStatus::builder()
            .play_state(0x05)
            .tempo(Some(14_110), Pitch::UNITY)
            .build();
        assert_eq!(byte_at(paused.as_bytes(), 0x8b), Some(0xfe));
        assert_eq!(be_u16_at(paused.as_bytes(), 0x90), Some(0x8000));
    }

    #[test]
    fn a_tempo_always_comes_with_a_track_it_belongs_to() {
        // Bytes 0x28-0x2f are zero *only* while byte 0x92 holds the no-tempo
        // sentinel, in all 25,766 captured packets. A master announcing a tempo
        // and no track is a combination no deck has ever sent, and a CDJ asked
        // to follow one ignores it -- which is exactly what it did.
        let playing = CdjStatus::builder()
            .device_number(device(4))
            .tempo(Some(14_110), Pitch::UNITY)
            .playing(true)
            .loaded_track(Some(LoadedTrack {
                source_player: device(2),
                slot: Slot::USB,
                id: 1234,
            }))
            .build();
        let parsed = CdjStatus::parse(playing.as_bytes()).expect("our own packet");
        assert_eq!(parsed.source_player(), Some(device(2)));
        assert_eq!(parsed.source_slot(), Slot::USB);
        assert_eq!(parsed.track_type(), 1, "a rekordbox track");
        assert_eq!(parsed.track_id(), 1234);
        // ...and the third byte that says the same thing a third way.
        assert_eq!(byte_at(parsed.as_bytes(), 0x9d), Some(0x09));

        let idle = CdjStatus::builder().device_number(device(4)).build();
        assert_eq!(idle.source_player(), None);
        assert_eq!(idle.track_id(), 0);
        assert_eq!(idle.track_type(), 0);
        assert_eq!(byte_at(idle.as_bytes(), 0x9d), Some(0x00));

        // Loaded but stopped: bit 0 without bit 3.
        let paused = CdjStatus::builder()
            .device_number(device(4))
            .tempo(Some(14_110), Pitch::UNITY)
            .loaded_track(Some(LoadedTrack {
                source_player: device(4),
                slot: Slot::SD,
                id: 7,
            }))
            .build();
        assert_eq!(byte_at(paused.as_bytes(), 0x9d), Some(0x01));
        assert_eq!(paused.source_slot(), Slot::SD);
    }

    #[test]
    fn a_deck_with_no_track_sends_sentinels_rather_than_zeros() {
        // Zero is a measurement here and the deck means an absence: 0.00 BPM
        // and "beat zero" are both readings a follower would act on. The wire
        // says 0xffff and 0xffffffff, and byte 0xa6 uses 0 for "no bar" only
        // because it has no room for a sentinel.
        let idle = CdjStatus::builder()
            .device_number(device(2))
            .tempo(None, Pitch::UNITY)
            .beat(None, None)
            .build();
        assert_eq!(idle.bpm_centi(), None);
        assert_eq!(idle.beat_number(), None);
        assert_eq!(idle.beat_in_bar(), None);
        assert_eq!(idle.is_tempo_master(), Some(false));
        assert_eq!(
            idle.flags().map(StatusFlags::is_playing),
            Some(false),
            "an idle deck is not playing"
        );
    }

    #[test]
    fn a_playing_master_round_trips_its_tempo_and_its_place_in_the_bar() {
        let playing = CdjStatus::builder()
            .device_number(device(1))
            .play_state(0x03)
            .tempo(Some(13_201), Pitch(0x000f_8312))
            .playing(true)
            .tempo_master(true)
            .beat(Some(67), BeatInBar::new(3))
            .build();
        let parsed = CdjStatus::parse(playing.as_bytes()).expect("our own packet parses");
        assert_eq!(parsed.bpm_centi(), Some(13_201));
        assert_eq!(parsed.pitch(), Some(Pitch(0x000f_8312)));
        assert_eq!(parsed.beat_number(), Some(67));
        assert_eq!(parsed.beat_in_bar(), BeatInBar::new(3));
        assert_eq!(parsed.is_tempo_master(), Some(true));
        // The two places mastership is written have to agree; they did in
        // 46,011 of the 46,012 captured packets, and the exception is a single
        // frame mid-handoff, which we never produce.
        let flags = parsed.flags().expect("flags");
        assert!(flags.is_tempo_master());
        assert!(flags.is_playing());
        assert!(!flags.is_synced());
        // 132.01 at −3.05% is what is actually coming out.
        let effective = parsed.effective_bpm().expect("an effective tempo");
        assert!(
            (effective - 128.0).abs() < 0.5,
            "132.01 BPM at -3.05% is about 128, not {effective}"
        );
    }

    #[test]
    fn the_pitch_is_written_to_every_copy_a_deck_writes() {
        // A playing deck put the same value at 0x8c, 0x98, 0xc0 and 0xc4 in
        // every packet of S06. Nothing says which one a follower reads, and
        // four copies that disagree is a state no deck has ever produced.
        let built = CdjStatus::builder()
            .tempo(Some(14_500), Pitch(0x0010_8000))
            .build();
        for offset in CdjStatus::OFF_PITCH_COPIES {
            assert_eq!(
                be_u32_at(built.as_bytes(), offset),
                Some(0x0010_8000),
                "pitch copy at {offset:#x}"
            );
        }
    }

    #[test]
    fn a_media_query_round_trips() {
        let query = MediaQuery {
            requester: device(2),
            requester_ip: Ipv4Addr::new(169, 254, 1, 2),
            target: device(4),
            slot: Slot::SD,
        };
        let raw = query.encode(DeviceName::default());
        assert_eq!(raw.len(), MediaQuery::LEN);
        // Bytes following 0x24: the address, the target and the slot.
        assert_eq!(be_u16_at(&raw, OFF_BODY_LEN), Some(0x0c));
        assert_eq!(MediaQuery::parse(&raw).unwrap(), query);
    }

    #[test]
    fn a_media_response_carries_the_counts_a_deck_needs() {
        let built = MediaResponse::builder()
            .device_number(device(4))
            .slot(Slot::USB)
            .volume_name("MY STICK")
            .counts(692, 35)
            .build();
        let parsed = MediaResponse::parse(built.as_bytes()).unwrap();
        assert_eq!(parsed.device(), Some(device(4)));
        assert_eq!(parsed.slot(), Slot::USB);
        assert_eq!(parsed.volume_name(), "MY STICK");
        assert_eq!(parsed.track_count(), 692);
        assert_eq!(parsed.playlist_count(), 35);
    }

    #[test]
    fn a_volume_name_survives_non_ascii() {
        let built = MediaResponse::builder().volume_name("カガミ").build();
        assert_eq!(
            MediaResponse::parse(built.as_bytes())
                .unwrap()
                .volume_name(),
            "カガミ"
        );
    }

    #[test]
    fn a_settings_query_round_trips() {
        let query = SettingsQuery {
            requester: device(1),
            sender: device(1),
            slot: Slot::USB,
        };
        let raw = query.encode(DeviceName::default());
        assert_eq!(raw.len(), SettingsQuery::LEN);
        assert_eq!(SettingsQuery::parse(&raw).unwrap(), query);
    }

    #[test]
    fn a_settings_response_carries_the_medium_bytes_verbatim() {
        let settings: Vec<u8> = (0..32u8).collect();
        let built = SettingsResponse::build(
            DeviceName::default(),
            device(4),
            device(2),
            Slot::USB,
            &settings,
        );
        let parsed = SettingsResponse::parse(built.as_bytes()).unwrap();
        assert_eq!(parsed.payload(), settings.as_slice());
        assert_eq!(parsed.requester(), Some(device(2)));
        assert_eq!(parsed.sender(), Some(device(4)));
        // The requester and the sender differ, which is how a response is told
        // from a query without looking at the kind byte.
        assert_ne!(parsed.requester(), parsed.sender());
    }

    #[test]
    fn a_short_settings_block_is_padded_not_rejected() {
        let built =
            SettingsResponse::build(DeviceName::default(), device(4), device(2), Slot::USB, &[]);
        assert_eq!(built.payload(), [0u8; 32]);
    }

    #[test]
    fn decoding_dispatches_on_the_kind_byte() {
        let status = CdjStatus::builder().device_number(device(1)).build();
        assert!(matches!(
            decode(status.as_bytes()),
            Ok(Packet::CdjStatus(_))
        ));

        let mut mangled = status.into_bytes();
        if let Some(byte) = mangled.get_mut(MAGIC.len()) {
            *byte = StatusKind::MIXER_STATUS.0;
        }
        let decoded = decode(&mangled).unwrap();
        assert_eq!(decoded.kind(), StatusKind::MIXER_STATUS);
        assert!(matches!(decoded, Packet::Other { .. }));
    }

    #[test]
    fn a_keep_alive_is_not_a_media_response() {
        // Type 0x06 means a keep-alive on 50000 and a media response here. The
        // keep-alive is far too short for this layout, so it comes back as
        // `Other` rather than decoding into confident nonsense.
        let keep_alive = crate::djl::Packet::new(
            DeviceName::default(),
            crate::DeviceKind::CDJ,
            crate::djl::Body::KeepAlive {
                device_number: 2,
                was_first_on_network: 1,
                mac: crate::MacAddress::default(),
                ip: Ipv4Addr::UNSPECIFIED,
                peer_count: 1,
                pad_31: [0; 3],
                flags: 1,
                trailing: 0,
            },
        )
        .encode();
        assert!(matches!(decode(&keep_alive), Ok(Packet::Other { .. })));
        assert!(MediaResponse::parse(&keep_alive).is_err());
    }
}
