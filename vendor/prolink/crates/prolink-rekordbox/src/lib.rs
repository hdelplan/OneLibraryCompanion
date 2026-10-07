// SPDX-License-Identifier: GPL-3.0-only

//! Readers for the files on a rekordbox device export.
//!
//! What rekordbox writes to a USB stick or SD card, and what a Pro DJ Link
//! server has to read back in order to browse and serve it. No network and no
//! I/O policy: every entry point here takes bytes a caller has already obtained,
//! because on this protocol they usually arrive over NFS from a device we do not
//! control rather than from a local file.
//!
//! | Module | File | |
//! |---|---|---|
//! | [`pdb`] | `PIONEER/rekordbox/export.pdb` | tracks, artists, playlists — the DeviceSQL database |
//! | [`anlz`] | `PIONEER/USBANLZ/**/ANLZ####.DAT`, `.EXT` | beat grid, cues, waveforms, VBR seek index |
//! | [`settings`] | `PIONEER/*SETTING*.DAT` | the utility settings a deck adopts from a medium |
//! | [`library`] | — | the pdb's foreign keys joined into browsable tracks |
//!
//! # Endianness is not uniform, and that is the point
//!
//! `export.pdb` is **little**-endian. The ANLZ files it points at are
//! **big**-endian. A path is UTF-16BE inside an ANLZ file and UTF-16LE inside
//! the pdb, and both differ from how the same path travels over the wire. These
//! must never share a helper; the one time two of them were conflated, the two
//! errors cancelled exactly for ASCII and the bug surfaced months later, on
//! someone's non-ASCII album name, as a failed track load (O6).
//!
//! # Three things that have cost this project sessions
//!
//! - **The UTF-16 string form starts at `offset + 4` and is little-endian.**
//!   See [`string`]. A round-trip test cannot catch getting this wrong.
//! - **Row offset `0x5a` of a track is the container**, not padding. See
//!   [`pdb::Container`] and F34.
//! - **The pdb header's sequence counter changes under you.** See
//!   [`pdb::stable_digest`] and F13.
//!
//! # Provenance
//!
//! Non-obvious constants carry the finding that establishes them — `F<n>` for a
//! finding, `C<n>` for a correction to the pre-hardware literature, `O<n>` for
//! an observation. Where a value is reproduced without being understood, or
//! inferred rather than measured, the doc comment says so in those words.
//!
//! The pdb reader is pinned against a real 675 KB, 165-page export; the ANLZ
//! and settings readers are not, because no such file was available, and their
//! module docs say so.

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
// docs.rs builds with `--cfg docsrs` (see `package.metadata.docs.rs`) so that
// the items behind `settings-detail` are documented with the feature they need
// shown beside them. `doc_cfg` is nightly-only, which is why it is gated: an
// ordinary `cargo doc` never sees this.
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod anlz;
pub mod library;
pub mod mytags;
pub mod pdb;
pub mod settings;
pub mod string;

mod error;

pub use anlz::{AnlzFile, FourCc, Tag};
pub use error::{Error, HexBytes, Result};
pub use library::{HistoryPlaylist, Library, Playlist, Summary, Track};
pub use pdb::{Container, PageType, Pdb, StableDigest, stable_digest};
pub use settings::SettingsFile;
pub use string::DeviceSqlString;
