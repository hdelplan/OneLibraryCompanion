// SPDX-License-Identifier: GPL-3.0-only

//! Discovery, device numbering and keep-alive — UDP 50000, broadcast.
//!
//! The announcement protocol Pioneer players and mixers use to find each other
//! and to agree on device numbers. Written from `docs/PROTOCOL.md` §2 and
//! validated against 7833 packets from 38 capture files.
//!
//! The handshake, all broadcast, ~300 ms apart:
//!
//! ```text
//! 3× hello → 3× claim_mac → 3× claim_ip → N× claim_number → keep_alive forever
//! ```
//!
//! `N` is 3 into an empty network and 1 into a populated one (C13). It is *not*
//! governed by the auto/manual assignment setting, which is what the
//! pre-hardware literature claims — three controlled boots with the setting
//! held constant settle it.
//!
//! # This is UDP 50000 only
//!
//! Port 50002 carries a *different* header — the device name starts one byte
//! earlier and is one byte shorter, and byte `0x1f` is a structural `0x01`
//! (C14) — and lives in [`crate::status`]. The type byte at `0x0a` is shared
//! across the two ports and the layouts behind it are not: `0x06` is a
//! keep-alive here and a media response there. Sharing one decoder yields
//! plausible nonsense rather than an error, which is worse, so the two are
//! deliberately kept apart.
//!
//! # Common header
//!
//! ```text
//! 0x00-0x09  magic "Qspt1WmJOL"
//! 0x0a       packet kind        the discriminator
//! 0x0b       subtype            0x00 on everything observed
//! 0x0c-0x1f  device name        20 bytes ASCII, NUL-padded
//! 0x20       constant 0x01
//! 0x21       device kind        0x01 mixer / 0x02 CDJ / 0x03 rekordbox or CDJ-3000
//! 0x22       padding 0x00
//! 0x23       stype              equals the total datagram length (C2)
//! 0x24…      body
//! ```

use std::fmt;
use std::io::Cursor;
use std::net::Ipv4Addr;
use std::time::Duration;

use binrw::{BinRead, binrw, helpers::until_eof};

use crate::device::{DeviceKind, DeviceName, MacAddress};
use crate::{Error, MAGIC, Result};

/// Steady-state keep-alive cadence.
///
/// A real CDJ-2000NXS sends every **2.0026 s** (n=28, min 2.002, max 2.003) — a
/// tight hardware timer. The pre-hardware literature gives 1.5 s and marks it
/// confirmed, but all four of its citations trace back to the *send* interval a
/// reference tool chose rather than to a measurement (C12). Since the goal is
/// to be indistinguishable from a CDJ, match the hardware.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);

/// Cadence of the startup handshake packets.
pub const DISCOVERY_INTERVAL: Duration = Duration::from_millis(300);

/// How long after its last keep-alive a peer is considered gone.
///
/// At the observed 2.0 s cadence that is five missed keep-alives, not the six
/// or seven inferred from an assumed 1.5 s.
pub const DEVICE_TIMEOUT: Duration = Duration::from_secs(10);

/// Length of the common header, and the offset of every body.
pub const HEADER_LEN: usize = 0x24;

/// Byte `0x0a`, the discriminator.
///
/// A newtype rather than an enum: a mixer, a CDJ-3000 or a newer firmware may
/// send a kind we have never seen, and a decoder that refused it would take out
/// discovery for the devices we *do* understand.
#[binrw]
#[brw(big)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PacketKind(pub u8);

impl PacketKind {
    /// Stage 1 of the claim chain: publish the MAC.
    pub const CLAIM_MAC: Self = Self(0x00);
    /// Mixer → player, on the channel-specific port only.
    pub const MIXER_ASSIGN_INTENT: Self = Self(0x01);
    /// Stage 2: publish the IP and propose a device number.
    pub const CLAIM_IP: Self = Self(0x02);
    /// Mixer → player: "use device number D".
    pub const MIXER_ASSIGN: Self = Self(0x03);
    /// Stage 3: assert the number.
    pub const CLAIM_NUMBER: Self = Self(0x04);
    /// "The number I hold is N", unicast to a device claiming one.
    pub const NUMBER_IN_USE: Self = Self(0x05);
    /// Steady state, broadcast every ~2 s.
    pub const KEEP_ALIVE: Self = Self(0x06);
    /// "That number is mine", unicast by the device that already holds it.
    pub const NUMBER_CONFLICT: Self = Self(0x08);
    /// "I am here", the first thing a device broadcasts.
    pub const HELLO: Self = Self(0x0a);

    /// A name for logs, or `None` for a kind we have never observed.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::CLAIM_MAC => "claim_mac",
            Self::MIXER_ASSIGN_INTENT => "mixer_assign_intent",
            Self::CLAIM_IP => "claim_ip",
            Self::MIXER_ASSIGN => "mixer_assign",
            Self::CLAIM_NUMBER => "claim_number",
            Self::NUMBER_IN_USE => "number_in_use",
            Self::KEEP_ALIVE => "keep_alive",
            Self::NUMBER_CONFLICT => "number_conflict",
            Self::HELLO => "hello",
            _ => return None,
        })
    }

    /// The total datagram length this kind is sent with, if we have seen one.
    ///
    /// The `stype` byte at `0x23` equals this for every kind observed. The
    /// pre-hardware literature lists `claim_number` as `stype` `0x26` but
    /// length `0x2a`; six real type-`0x04` packets are `0x26` bytes long, so
    /// its length column is simply wrong there (C2).
    pub fn wire_length(self) -> Option<u8> {
        Some(match self {
            Self::HELLO => 0x25,
            Self::CLAIM_MAC => 0x2c,
            Self::CLAIM_IP => 0x32,
            Self::CLAIM_NUMBER | Self::NUMBER_IN_USE => 0x26,
            Self::KEEP_ALIVE => 0x36,
            Self::NUMBER_CONFLICT => 0x29,
            _ => return None,
        })
    }
}

impl fmt::Debug for PacketKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "PacketKind({:#04x})", self.0),
        }
    }
}

/// Byte `0x31` of the stage-2 claim: how this device chose its number.
#[binrw]
#[brw(big)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssignmentMode(pub u8);

impl AssignmentMode {
    /// The device picks a number itself.
    pub const AUTO: Self = Self(0x01);
    /// The DJ set a specific number.
    pub const MANUAL: Self = Self(0x02);
}

impl fmt::Debug for AssignmentMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::AUTO => f.write_str("auto"),
            Self::MANUAL => f.write_str("manual"),
            Self(raw) => write!(f, "AssignmentMode({raw:#04x})"),
        }
    }
}

/// One UDP-50000 datagram.
///
/// Decoding preserves every byte, including the ones whose meaning is unknown,
/// so a captured packet re-encodes byte-for-byte. That property is what the
/// corpus test checks, and it is what lets the virtual CDJ be diffed against
/// real hardware rather than merely inspected.
///
/// The kind byte at `0x0a` is deliberately **not** a field: it is a function of
/// the body ([`Packet::kind`]), so a packet whose header says keep-alive and
/// whose body is a hello cannot be built. The three header bytes that *are*
/// fields — `const_one`, `pad_22`, `stype` — are invariant on every packet ever
/// captured but are still data rather than derived values, because nothing
/// guarantees firmware we have not seen agrees.
#[binrw]
#[brw(big, magic = b"Qspt1WmJOL")]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Packet {
    /// Byte `0x0a`, consumed to dispatch [`Packet::body`] and re-derived from
    /// it on the way out.
    #[br(temp)]
    #[bw(calc = body.kind())]
    kind: PacketKind,
    /// Byte `0x0b`. Zero on everything observed.
    pub subtype: u8,
    /// `0x0c`–`0x1f`.
    pub name: DeviceName,
    /// Byte `0x20`. Invariant `0x01` across every packet in every capture,
    /// stored rather than asserted so an unexpected value survives a round trip
    /// instead of dropping the datagram.
    pub const_one: u8,
    /// Byte `0x21`.
    pub device_kind: DeviceKind,
    /// Byte `0x22`. Padding, always zero.
    pub pad_22: u8,
    /// Byte `0x23`. Equals the total datagram length for every kind observed.
    pub stype: u8,
    /// Everything from `0x24` on.
    #[br(args(kind))]
    pub body: Body,
    /// Bytes past the fields this kind declares.
    ///
    /// Empty for every packet in the corpus. Kept so that a longer variant from
    /// firmware we have not seen round-trips rather than being silently
    /// truncated — generation variants do differ in length.
    #[br(parse_with = until_eof)]
    pub trailing: Vec<u8>,
}

/// Everything from offset `0x24` on, dispatched on the packet kind.
#[binrw]
#[brw(big)]
#[br(import(kind: PacketKind))]
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Body {
    /// Type `0x0a` — "I am here". Broadcast about three times, ~300 ms apart.
    #[br(pre_assert(kind == PacketKind::HELLO))]
    Hello {
        /// `0x01` CDJ, `0x02` mixer. A DJM-900NXS has been seen sending `0x03`.
        payload: u8,
    },

    /// Type `0x00` — stage 1, publishes the MAC.
    #[br(pre_assert(kind == PacketKind::CLAIM_MAC))]
    ClaimMac {
        /// 1, 2, 3 — position in the three-packet burst.
        iteration: u8,
        /// The CDJ/mixer role byte.
        flags: u8,
        /// This device's hardware address.
        mac: MacAddress,
    },

    /// Type `0x02` — stage 2. Publishes the IP and *proposes* a device number.
    #[br(pre_assert(kind == PacketKind::CLAIM_IP))]
    ClaimIp {
        /// The address this device has taken.
        #[br(map = |raw: [u8; 4]| Ipv4Addr::from(raw))]
        #[bw(map = |ip: &Ipv4Addr| ip.octets())]
        ip: Ipv4Addr,
        /// This device's hardware address.
        mac: MacAddress,
        /// Byte `0x2e`. The number being proposed, not yet held.
        device_number: u8,
        /// Position in the three-packet burst.
        iteration: u8,
        /// Byte `0x30`. A CDJ/mixer role, **not** a constant (C1).
        role: u8,
        /// Byte `0x31` (F36). Every capture before F36 had both decks numbered
        /// manually, so only `manual` had ever been seen.
        assignment_mode: AssignmentMode,
    },

    /// Type `0x04` — stage 3, asserts the number.
    #[br(pre_assert(kind == PacketKind::CLAIM_NUMBER))]
    ClaimNumber {
        /// The number being claimed.
        device_number: u8,
        /// Position in the burst.
        iteration: u8,
    },

    /// Type `0x05` — "the number I hold is N".
    ///
    /// Byte-for-byte a [`Body::ClaimNumber`] but for the type byte. The
    /// pre-hardware literature files `0x05` under mixer channel assignment.
    /// What we saw instead: in the same instant a joining deck sent its stage-3
    /// claim, an **auto-numbered** deck *unicast* one of these back carrying its
    /// own number (F36). Reading it as "this number is taken" fits what an
    /// auto-assigning device must publish, though that is inference from a
    /// single occurrence.
    #[br(pre_assert(kind == PacketKind::NUMBER_IN_USE))]
    NumberInUse {
        /// The number the sender holds.
        device_number: u8,
        /// Position in the burst.
        iteration: u8,
    },

    /// Type `0x06` — the steady-state keep-alive, and the load-bearing packet:
    /// it is what makes a virtual CDJ visible, and passively it is the only
    /// source of peers.
    #[br(pre_assert(kind == PacketKind::KEEP_ALIVE))]
    KeepAlive {
        /// Byte `0x24`.
        device_number: u8,
        /// Byte `0x25`: **"was I first on this network?"** — `0x02` if this
        /// device was first, `0x01` if peers were already present. Latched at
        /// boot and never re-evaluated: a deck held `0x02` while its peer count
        /// went 1→2 (F9). It is not a CDJ/mixer role byte as documented, and
        /// not the peer count.
        was_first_on_network: u8,
        /// `0x26`–`0x2b`.
        mac: MacAddress,
        /// `0x2c`–`0x2f`.
        #[br(map = |raw: [u8; 4]| Ipv4Addr::from(raw))]
        #[bw(map = |ip: &Ipv4Addr| ip.octets())]
        ip: Ipv4Addr,
        /// Byte `0x30`. Includes the sender.
        peer_count: u8,
        /// `0x31`–`0x33`.
        pad_31: [u8; 3],
        /// Byte `0x34`. This one *is* the role byte: `0x01` CDJ, `0x02` mixer,
        /// consistent across every packet observed.
        flags: u8,
        /// Byte `0x35`. **`0x00` on nexus hardware, not `0x01`** as documented
        /// (C3) — 148 packets from a CDJ-2000nexus and 91 from a DJM-2000nexus
        /// all carry `0x00`. `0x64` is required for CDJ-3000 coexistence, and
        /// the wrong value there can make CDJ-3000s set to player 5/6
        /// repeatedly kick themselves off the network.
        trailing: u8,
    },

    /// Type `0x08` — "that number is mine", **unicast** by the device that
    /// already holds it, in reply to someone else's claim.
    ///
    /// Silence is not evidence a number is free: an XDJ-XZ and an Opus Quad do
    /// not defend their numbers with these at all, so only having watched the
    /// network is.
    #[br(pre_assert(kind == PacketKind::NUMBER_CONFLICT))]
    NumberConflict {
        /// The number being defended.
        device_number: u8,
        /// The defender's address.
        #[br(map = |raw: [u8; 4]| Ipv4Addr::from(raw))]
        #[bw(map = |ip: &Ipv4Addr| ip.octets())]
        ip: Ipv4Addr,
    },

    /// A well-formed datagram whose kind this crate does not model.
    ///
    /// Returned rather than raised. The mixer-side assignment types `0x01` and
    /// `0x03` land here, and so would anything a newer firmware invents; a
    /// decoder that threw would take out discovery for the kinds we do
    /// understand.
    Unknown {
        /// The kind byte, carried over from the header so the packet can be
        /// re-encoded.
        #[br(calc = kind)]
        #[bw(ignore)]
        kind: PacketKind,
        /// The undecoded body.
        #[br(parse_with = until_eof)]
        payload: Vec<u8>,
    },
}

impl Body {
    /// The header's kind byte for this body.
    pub fn kind(&self) -> PacketKind {
        match self {
            Self::Hello { .. } => PacketKind::HELLO,
            Self::ClaimMac { .. } => PacketKind::CLAIM_MAC,
            Self::ClaimIp { .. } => PacketKind::CLAIM_IP,
            Self::ClaimNumber { .. } => PacketKind::CLAIM_NUMBER,
            Self::NumberInUse { .. } => PacketKind::NUMBER_IN_USE,
            Self::KeepAlive { .. } => PacketKind::KEEP_ALIVE,
            Self::NumberConflict { .. } => PacketKind::NUMBER_CONFLICT,
            Self::Unknown { kind, .. } => *kind,
        }
    }

    /// The device number this body carries, where it carries one.
    pub fn device_number(&self) -> Option<u8> {
        match *self {
            Self::ClaimIp { device_number, .. }
            | Self::ClaimNumber { device_number, .. }
            | Self::NumberInUse { device_number, .. }
            | Self::KeepAlive { device_number, .. }
            | Self::NumberConflict { device_number, .. } => Some(device_number),
            _ => None,
        }
    }

    /// The hardware address this body carries, where it carries one.
    pub fn mac(&self) -> Option<MacAddress> {
        match *self {
            Self::ClaimMac { mac, .. }
            | Self::ClaimIp { mac, .. }
            | Self::KeepAlive { mac, .. } => Some(mac),
            _ => None,
        }
    }

    /// The address this body carries, where it carries one.
    pub fn ip(&self) -> Option<Ipv4Addr> {
        match *self {
            Self::ClaimIp { ip, .. }
            | Self::KeepAlive { ip, .. }
            | Self::NumberConflict { ip, .. } => Some(ip),
            _ => None,
        }
    }
}

impl Packet {
    /// Build a packet with the header a CDJ sends, filling `stype` from the
    /// body's kind.
    pub fn new(name: DeviceName, device_kind: DeviceKind, body: Body) -> Self {
        let stype = body.kind().wire_length().unwrap_or(0);
        Self {
            subtype: 0x00,
            name,
            const_one: 0x01,
            device_kind,
            pad_22: 0x00,
            stype,
            body,
            trailing: Vec::new(),
        }
    }

    /// Byte `0x0a`, the kind this packet will be sent as.
    pub fn kind(&self) -> PacketKind {
        self.body.kind()
    }

    /// Decode one datagram.
    ///
    /// Fails when the magic is absent or the datagram is too short for the
    /// common header. An unrecognised *kind* yields [`Body::Unknown`] rather
    /// than an error.
    pub fn decode(data: &[u8]) -> Result<Self> {
        if !carries_magic(data) {
            let got = data.get(..MAGIC.len()).unwrap_or(data);
            return Err(Error::BadMagic {
                expected: MAGIC.as_slice().into(),
                got: got.into(),
            });
        }
        if data.len() < HEADER_LEN {
            return Err(Error::Truncated {
                need: HEADER_LEN,
                at: 0,
                have: data.len(),
            });
        }
        Ok(Self::read(&mut Cursor::new(data))?)
    }

    /// Encode this packet.
    pub fn encode(&self) -> Vec<u8> {
        crate::to_bytes(self)
    }
}

/// Cheap guard: does this datagram carry the Pro DJ Link magic?
///
/// Shared with [`crate::status`] — the magic is the same on all three ports;
/// only what follows it differs.
pub fn carries_magic(data: &[u8]) -> bool {
    data.get(..MAGIC.len()) == Some(MAGIC.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real 54-byte keep-alive, byte for byte, from `captures/S24b-e9-control`.
    ///
    /// Committed so the header layout is pinned by a literal rather than by
    /// whatever happens to be in a capture directory today.
    const KEEP_ALIVE: &[u8] = &[
        0x51, 0x73, 0x70, 0x74, 0x31, 0x57, 0x6d, 0x4a, 0x4f, 0x4c, 0x06, 0x00, 0x43, 0x44, 0x4a,
        0x2d, 0x32, 0x30, 0x30, 0x30, 0x6e, 0x65, 0x78, 0x75, 0x73, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x02, 0x00, 0x36, 0x05, 0x02, 0xa0, 0xce, 0xc8, 0xe2, 0x26, 0xde, 0xa9,
        0xfe, 0x63, 0x64, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
    ];

    #[test]
    fn decodes_a_real_keep_alive() {
        let packet = Packet::decode(KEEP_ALIVE).unwrap();
        assert_eq!(packet.kind(), PacketKind::KEEP_ALIVE);
        assert_eq!(packet.name.as_str(), "CDJ-2000nexus");
        assert_eq!(packet.name.0, *b"CDJ-2000nexus\0\0\0\0\0\0\0");
        assert_eq!(packet.device_kind, DeviceKind::CDJ);
        // stype equals the datagram length on every kind we have seen (C2).
        assert_eq!(usize::from(packet.stype), KEEP_ALIVE.len());
        assert_eq!(packet.stype, 0x36);
        let Body::KeepAlive {
            device_number,
            mac,
            ip,
            peer_count,
            trailing,
            ..
        } = packet.body
        else {
            panic!("expected a keep-alive body, got {:?}", packet.body);
        };
        assert_eq!(device_number, 5);
        assert_eq!(mac.to_string(), "a0:ce:c8:e2:26:de");
        assert_eq!(ip, Ipv4Addr::new(169, 254, 99, 100));
        assert_eq!(peer_count, 1);
        // 0x00 on nexus hardware, not 0x01 as the literature has it (C3).
        assert_eq!(trailing, 0x00);
    }

    #[test]
    fn round_trips_byte_for_byte() {
        let packet = Packet::decode(KEEP_ALIVE).unwrap();
        assert_eq!(packet.encode(), KEEP_ALIVE);
    }

    #[test]
    fn an_unknown_kind_decodes_instead_of_failing() {
        let mut raw = KEEP_ALIVE.to_vec();
        raw[0x0a] = 0x7f;
        let packet = Packet::decode(&raw).unwrap();
        assert_eq!(packet.kind(), PacketKind(0x7f));
        assert!(matches!(packet.body, Body::Unknown { .. }));
        assert_eq!(
            packet.encode(),
            raw,
            "an unknown body must survive a round trip"
        );
    }

    #[test]
    fn rejects_a_datagram_without_the_magic() {
        assert!(matches!(
            Packet::decode(b"not a djl packet at all"),
            Err(Error::BadMagic { .. })
        ));
    }

    #[test]
    fn rejects_a_short_datagram() {
        assert!(matches!(
            Packet::decode(&MAGIC),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn builds_a_keep_alive_a_deck_would_accept() {
        let built = Packet::new(
            DeviceName::new(DeviceName::CDJ_2000_NEXUS),
            DeviceKind::CDJ,
            Body::KeepAlive {
                device_number: 5,
                was_first_on_network: 0x02,
                mac: MacAddress([0xa0, 0xce, 0xc8, 0xe2, 0x26, 0xde]),
                ip: Ipv4Addr::new(169, 254, 99, 100),
                peer_count: 1,
                pad_31: [0; 3],
                flags: 0x01,
                trailing: 0x00,
            },
        );
        assert_eq!(
            built.encode(),
            KEEP_ALIVE,
            "must be indistinguishable from the real one"
        );
    }
}
