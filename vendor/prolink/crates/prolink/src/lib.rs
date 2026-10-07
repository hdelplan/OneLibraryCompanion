// Copyright (C) 2026 the prolink authors.
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.
//
// SPDX-License-Identifier: GPL-3.0-only

//! Pioneer Pro DJ Link, both directions.
//!
//! - **Consume**: see the media in CDJs on the network, browse their libraries
//!   the way the LINK button does, and stream their tracks.
//! - **Serve**: appear as another CDJ with a USB and an SD slot, and let real
//!   players browse and play a local rekordbox medium.
//!
//! # Passive by default
//!
//! [`discovery::Discovery`] transmits **nothing**. It can be run next to a live
//! rig without contending for a device number or disturbing anything, and it is
//! enough to see every device and to pull a player's database over NFS.
//!
//! Announcing is a separate, explicit step, because it is the one that carries
//! risk: [`virtual_cdj::VirtualCdj`] claims a device number and starts emitting
//! status. That is required for anything a peer has to *offer* us — slot
//! contents, tempo master, and being browsable at all.
//!
//! # What the other players have in their slots
//!
//! [`VirtualCdj::peer_media`] answers it, and it is filled from two sources
//! because the protocol splits the answer in two. Whether a slot holds anything
//! is published in status packets and nowhere else, so it costs nothing but
//! having announced; the volume label and the track and playlist counts come
//! only from a media query, which [`VirtualCdj::survey_media`] sends. A deck
//! answers for an empty slot too, with everything zeroed, so occupancy is read
//! from the status byte rather than inferred from the counts.
//!
//! # What a device must do to be browsable
//!
//! Learned by getting each one wrong in turn, and the order matters:
//!
//! 1. **Announce** on UDP 50000 — keep-alive at least, the claim chain to hold
//!    a number, and that number **must be in 1–4** (F45). Outside that range
//!    every later step still works and none of them is ever reached.
//! 2. **Emit status** on UDP 50002, unicast per peer at 200 ms, with the slot
//!    state set. Media presence is advertised there and nowhere else.
//! 3. **Answer media queries** with true track and playlist counts.
//! 4. **Serve NFS** — and this comes *before* dbserver. A portmapper on UDP/111
//!    is mandatory: with nothing there a deck retries `GETPORT` once a second
//!    indefinitely and never falls back to the well-known ports.
//! 5. **Answer the port query** on TCP 12523.
//! 6. **Serve dbserver** on the advertised port, and never answer an unknown
//!    request with an error.
//! 7. Optionally answer the settings query, for LOAD SETTINGS.

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

pub mod consume;
pub mod device;
pub mod discovery;
pub mod interface;
pub mod media;
pub mod monitor;
pub mod playback;
pub mod serve;
pub mod virtual_cdj;
pub mod volumes;

mod error;
mod socket;

pub use device::{Device, DeviceEvent, DeviceTable};
pub use discovery::Discovery;
pub use error::{Error, Result};
pub use interface::Interface;
pub use media::{MediaDescription, MediaSource};
pub use monitor::{Monitor, MonitorEvent, PlayerState};
pub use playback::{BeatPosition, Playback, PlaybackCell};
/// The beat-packet types that appear in [`monitor`]'s signatures.
pub use prolink_proto::beat::{Beat, BeatInBar, Pitch};
pub use serve::{MediaSet, Medium, VirtualPlayer, VirtualPlayerConfig};
pub use virtual_cdj::{
    Numbering, OBSERVER_NUMBER, PeerMedia, PeerSlot, VirtualCdj, VirtualCdjConfig,
};
pub use volumes::{Volume, rekordbox_volumes};

/// Re-exported so callers do not need a direct dependency on the codec crate
/// for the handful of its types that appear in this one's signatures.
pub use prolink_proto::{
    BrowsableDeviceNumber, DeviceKind, DeviceName, DeviceNumber, MacAddress, Slot,
};
