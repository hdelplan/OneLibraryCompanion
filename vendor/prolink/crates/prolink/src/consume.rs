// SPDX-License-Identifier: GPL-3.0-only

//! Browsing and streaming from the CDJs on the network.
//!
//! Two independent paths to the same media, and which one is available depends
//! on whether we have announced ourselves:
//!
//! - **ONC RPC / NFSv2** works passively. A CDJ exports to the whole link-local
//!   subnet, so a host that has never announced is inside the permitted set by
//!   default (F11, F12). That is enough to pull `export.pdb` and to read a
//!   track's bytes.
//! - **dbserver** needs a device number of 6 or lower, which means announcing.
//!   It is the only way to get album art out of a player: a real CDJ never asks
//!   NFS for an image.
//!
//! # Loading a track uses both, and in this order
//!
//! The two halves meet at one string. [`DbClient::track_info`] answers with a
//! path already relative to a mount root and — in the argument slot that is
//! zero on every other menu item ever captured — the file's size (F31). That
//! path goes straight into [`NfsClient::open`], and the bytes come back over
//! `READ`:
//!
//! ```text
//! DbClient::track_info(slot, id)  ─►  "/Contents/…/track.mp3", 7 633 531 bytes
//! NfsClient::mount_slot(slot)     ─►  the root filehandle
//! NfsClient::open(&mount, path)   ─►  a handle and the size again, from LOOKUP
//! NfsClient::read_range(…)        ─►  audio
//! ```
//!
//! A deck does not download the file; it streams, touching about 38% of a
//! 7.6 MB MP3 during one load plus thirty seconds of playback (F18). Prefer
//! [`NfsClient::read_range`] to [`NfsClient::read_file`] for audio, and reserve
//! the whole-file pull for `export.pdb`.
//!
//! # Passive first
//!
//! Everything under [`nfs`] works without transmitting on any Pro DJ Link port.
//! [`dbclient`] does not, and says so in its types: [`DbClient::connect`] takes
//! a `BrowsableDeviceNumber`, which only a virtual CDJ that has claimed a
//! number in 1–4 can produce (F45).

pub mod dbclient;
pub mod nfs;

pub use dbclient::{Analysis, DbClient, DbConfig, TrackInfo, TrackMetadata};
pub use nfs::{Mount, NfsClient, NfsConfig, Progress, ReadSize, RemoteFile};
