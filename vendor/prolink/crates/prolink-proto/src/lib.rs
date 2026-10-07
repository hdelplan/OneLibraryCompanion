// Copyright (C) 2026 the prolink authors.
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.
//
// SPDX-License-Identifier: GPL-3.0-only

//! Wire codecs for the Pioneer Pro DJ Link protocol.
//!
//! Pure encoding and decoding: no sockets, no filesystem, no clock. Everything
//! here is a function from bytes to values and back, so the whole protocol
//! surface is testable against captured traffic without a network — which is
//! how it is tested, against 33 pcapng captures of two CDJ-2000NXS running
//! firmware 1.44.
//!
//! # The four wire formats
//!
//! | Module | Transport | Carries |
//! |---|---|---|
//! | [`djl`] | UDP 50000, broadcast | Discovery, device-number claiming, keep-alive |
//! | [`status`] | UDP 50002, unicast | Player status, media queries, device settings |
//! | [`dbserver`] | TCP 1051 | The metadata protocol the LINK button drives |
//! | [`rpc`] | UDP 111 / 2049 / 48276 | ONC RPC v2, the portmapper, MOUNT and NFSv2 |
//!
//! [`analysis`] is not a wire format but a set of transforms: rekordbox writes
//! a track's analysis one way and dbserver puts it on the wire another, and
//! three of the five blobs change layout in between.
//!
//! # Endianness is not uniform, and that is the point
//!
//! Two conventions coexist and they are opposites:
//!
//! - **dbserver** strings are UTF-16 **big**-endian, length-prefixed in
//!   *characters* including the terminating NUL.
//! - **NFS** paths and names are UTF-16 **little**-endian, length-prefixed in
//!   *bytes*.
//!
//! They must never share a helper. [`dbserver::encode_string`] and
//! [`rpc::xdr::Writer::utf16le_string`] are deliberately separate.
//!
//! # Provenance
//!
//! Every constant here was observed on the wire. Where a value is reproduced
//! without being understood, the doc comment says so and cites the finding
//! number (`F<n>`, `C<n>`, `O<n>`) in the research record that establishes it.
//! Several fields are copied verbatim precisely because substituting a
//! plausible zero has broken playback (F33, F35).

// Tests are allowed to panic: an assertion *is* the failure mode, and a test
// that carefully propagated errors would report them as passes.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::as_conversions
    )
)]

pub mod analysis;
pub mod beat;
pub mod dbserver;
pub mod device;
pub mod djl;
pub mod rpc;
pub mod status;

mod error;
mod status_templates;

pub use device::{BrowsableDeviceNumber, DeviceKind, DeviceName, DeviceNumber, MacAddress};
pub use error::{Error, Result};

/// Encode a `binrw` value into a fresh buffer.
///
/// Infallible by construction: the only two things `binrw` can fail on are a
/// short write and an argument it cannot satisfy, and a `Vec` never runs out of
/// room while every type in this crate writes with `()` arguments. Centralised
/// so the reasoning is written down once instead of at every call site.
pub(crate) fn to_bytes<T>(value: &T) -> Vec<u8>
where
    T: binrw::BinWrite + for<'a> binrw::meta::WriteEndian,
    for<'a> <T as binrw::BinWrite>::Args<'a>: Default,
{
    let mut out = std::io::Cursor::new(Vec::new());
    #[expect(
        clippy::expect_used,
        reason = "writing to a Vec cannot fail; see the doc comment"
    )]
    binrw::BinWrite::write(value, &mut out).expect("encoding into a Vec cannot fail");
    out.into_inner()
}

/// The 10-byte magic every Pro DJ Link datagram starts with, on all three UDP
/// ports.
pub const MAGIC: [u8; 10] = *b"Qspt1WmJOL";

/// UDP port carrying discovery, device-number claiming and keep-alive.
pub const DISCOVERY_PORT: u16 = 50000;

/// UDP port carrying beat and sync packets.
pub const BEAT_PORT: u16 = 50001;

/// UDP port carrying player status, media queries and device settings.
pub const STATUS_PORT: u16 = 50002;

/// A media slot, numbered as the status packets number them.
///
/// A newtype rather than an enum because slot bytes outside 0–4 do turn up in
/// the corpus, and a decoder that refuses them would drop a whole packet over
/// one field it does not need.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slot(pub u8);

impl Slot {
    /// No slot / nothing loaded.
    pub const NONE: Self = Self(0);
    /// The CD drive.
    pub const CD: Self = Self(1);
    /// The SD card slot.
    pub const SD: Self = Self(2);
    /// The USB slot.
    pub const USB: Self = Self(3);
    /// A rekordbox collection reached over the network.
    pub const REKORDBOX: Self = Self(4);

    /// The lowercase name used on the command line and in log output.
    pub fn name(self) -> &'static str {
        match self {
            Self::NONE => "none",
            Self::CD => "cd",
            Self::SD => "sd",
            Self::USB => "usb",
            Self::REKORDBOX => "rekordbox",
            _ => "unknown",
        }
    }

    /// Parse a slot from its [`name`](Self::name), accepting `rb` for
    /// `rekordbox`.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "none" => Self::NONE,
            "cd" => Self::CD,
            "sd" => Self::SD,
            "usb" => Self::USB,
            "rb" | "rekordbox" => Self::REKORDBOX,
            _ => return None,
        })
    }
}

impl core::fmt::Debug for Slot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::NONE | Self::CD | Self::SD | Self::USB | Self::REKORDBOX => {
                f.write_str(self.name())
            }
            Self(raw) => write!(f, "Slot({raw:#04x})"),
        }
    }
}

impl core::fmt::Display for Slot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, f)
    }
}
