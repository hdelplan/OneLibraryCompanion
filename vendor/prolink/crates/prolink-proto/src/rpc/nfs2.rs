// SPDX-License-Identifier: GPL-3.0-only

//! NFS version 2 (RFC 1094) — program 100003 v2, UDP 2049.
//!
//! The procedures that matter for reading a track off a CDJ are two: `LOOKUP`,
//! which walks a path one component at a time because NFS has no notion of a
//! multi-component path, and `READ`, which pulls byte ranges. Together with
//! `GETATTR` those are the *only* three NFS procedures a real deck has ever
//! been seen to call — 55,968 calls across 38 captures, and not one `READDIR`,
//! `STATFS`, `READLINK` or write of any kind. A deck already knows every path
//! it wants, because it read them out of `export.pdb`, so it goes straight
//! from `MNT` to a chain of `LOOKUP`s.
//!
//! `READDIR` and `STATFS` are implemented anyway: `READDIR` because a client
//! that wants to enumerate a medium without parsing its database has no other
//! way, and `STATFS` because it is where the free-space numbers a Link Info
//! panel shows would come from. Neither has ever been exercised against
//! hardware in either direction, so both are RFC 1094 as written rather than
//! anything observed. Everything that writes is deliberately absent: a
//! rekordbox export is read-only and so are we.
//!
//! # The filehandle, and the place a CDJ breaks the spec
//!
//! RFC 1094 is unambiguous that a filehandle is 32 **opaque** bytes, echoed
//! back verbatim. A CDJ-2000NXS keeps only the leading **twelve** and
//! overwrites the remaining twenty with its own file reference (F28):
//!
//! ```text
//! served:   8a5edab282632443219e051e 4ade2d1d5bbc671c781051bf1437897cbdfea0f1
//! returned: 8a5edab282632443219e051e 03012d0000001b58000000000303010000000162
//!           |____ first 12 kept ____| |______ replaced by the player _______|
//! ```
//!
//! It is not arbitrary, and **it happens deck to deck**, with no code of ours
//! anywhere near it. Walking one deck's USB from another, the served handles
//! are three 32-bit identifiers followed by twenty zero bytes and the returned
//! ones keep exactly those twelve:
//!
//! ```text
//! MNT    '/C/'         served 012538a8 012538a8 012538a8 | 00 × 20
//! LOOKUP 'Contents'      sent 012538a8 012538a8 012538a8 | 0301…0266
//!                      served 012f1d54 012538a8 012538a8 | 00 × 20
//! LOOKUP 'FORMAT TEST'   sent 012f1d54 012538a8 012538a8 | 0301…0266
//!                      served 01301b94 012f1d54 012538a8 | 00 × 20
//! ```
//!
//! The three words read as `[self, parent, mount-root]` — the mount root's
//! leading byte was `01` for the USB export and `02` for the SD one on the
//! same deck. So twelve bytes survive because twelve bytes is a deck's entire
//! idea of what a handle *is*, and Pioneer's own server tolerates the rewrite
//! because it never looks past them. Across the corpus: 3066 calls to our
//! server and 372 deck-to-deck calls kept exactly the first twelve and
//! overwrote the rest; **zero** kept fewer.
//!
//! A server that trusts the spec browses perfectly and then fails at exactly
//! the moment a DJ loads a track, which is the worst possible time to find
//! out. No reference implementation could have caught it, because none of them
//! serve.
//!
//! That is why [`FileHandle`] and [`FileHandleKey`] are two types. A handle is
//! what travels; a key is the part of it a server may rely on. Key your handle
//! table on [`FileHandle::key`] and the bug is unwritable. Make the first
//! twelve bytes self-describing: bytes 12–31 are scratch space the client
//! owns.
//!
//! *Consequence for serving two media at once:* a handle is normally a hash of
//! the path, so two media sharing a root mint identical handles for the same
//! relative path — the root most obviously — and after truncation nothing
//! distinguishes them. Give each medium its own subtree.
//!
//! # Names are UTF-16LE
//!
//! Every filename here is [`xdr::Utf16LeString`], not ASCII. A single wrong
//! byte yields `NFSERR_NOENT` and nothing more helpful.
//!
//! # 32-bit offsets
//!
//! Sizes and offsets are 32-bit words, so NFSv2 cannot address past 4 GiB.
//! Fine for audio, but the ceiling is asserted rather than silently wrapped:
//! see [`checked_offset`] and [`Fattr::regular_file`].
//!
//! # Reads are a latency problem, not a throughput one
//!
//! CDJs stream rather than download: 38% of a 7.6 MB file touched during one
//! load plus thirty seconds of playback plus cue juggling, including a read of
//! the very last byte (F18). A server must therefore answer random-access
//! reads with low latency *during playback*; a stall is an audio dropout on
//! someone's deck, not a slow transfer. 75 MB lossless files have been served
//! and scrubbed without delay (F39).
//!
//! Read sizes are larger than the specification allows, and F19 understates
//! them. Deck to deck, the modal request is **9408** bytes (5097 of 7043),
//! then 8192 (1283), then 2048 (264), with first-reads of a file as large as
//! **28584** — answered in full, as one 28,656-byte datagram in about twenty
//! IP fragments. So 8192 is what RFC 1094 permits and what F19 measured in one
//! session, not a limit hardware respects in either direction. See
//! [`MAX_READ_PAYLOAD`]. Offsets are plain byte offsets and are not
//! block-aligned; a track read typically starts at 44, past a container
//! header.
//!
//! # Name matching is the caller's problem, and it is not byte equality
//!
//! A rekordbox medium is FAT32 and case-insensitive, and `export.pdb` does not
//! necessarily record a directory entry's case — the database says
//! `Gesaffelstein` where the directory is `GESAFFELSTEIN`. The pdb also stores
//! NFC where the filesystem reports NFD. A server comparing bytes answers
//! `NFSERR_NOENT` for files that are plainly there. Match exactly first, then
//! fall back to a case-folded NFC comparison, and always return the handle for
//! the name **as stored** (O6). This module hands over the bytes and takes no
//! view; the comparison belongs to whatever holds the filesystem.

use std::fmt;

use crate::rpc::Program;
use crate::rpc::xdr::{self, Utf16LeString};
use crate::{Error, Result};

/// The program number a CDJ answers NFS on (F10).
pub const PROGRAM: Program = Program::NFS;

/// The only version anything in this protocol speaks.
pub const VERSION: u32 = 2;

/// The standard NFS port, which is also where a real player answers (F6).
///
/// Unlike mountd's, this one is the registered number — but a deck still
/// discovers it through the portmapper and never falls back to it (F46).
pub const PORT: u16 = 2049;

/// A filehandle is exactly 32 bytes (RFC 1094).
pub const FHANDLE_LEN: usize = 32;

/// NFSv2's documented ceiling on a single `READ` payload, and the size F19
/// records real CDJs using.
///
/// **A limit to request, not a limit to expect.** See [`MAX_READ_PAYLOAD`]:
/// hardware exceeds this both ways.
pub const MAX_DATA: usize = 8192;

/// The largest `READ` payload this decoder will accept.
///
/// Deliberately *not* [`MAX_DATA`]. RFC 1094 caps a read at 8192 and F19
/// reports 8192-byte reads dominating one track load, but across the whole
/// corpus a deck asks its peer for **9408** more often than for 8192 (5097
/// calls against 1283, deck to deck) and asks for as much as **28584** on a
/// file's first read — and the serving deck answers in full, a 28,656-byte
/// datagram in about twenty IP fragments. Refusing those would drop real
/// traffic, so the decoder's ceiling is what a UDP datagram can physically
/// carry: 65535 less the IP and UDP headers.
///
/// A *server* is still free to answer short. A client must handle it: a short
/// read is normal and means "re-request the shortfall", not "end of file".
pub const MAX_READ_PAYLOAD: u32 = 65_507;

/// The largest file NFSv2 can describe, because `fattr.size` is 32 bits.
pub const MAX_FILE_SIZE: u64 = 0xffff_ffff;

/// The longest filename this decoder will accept before refusing to allocate.
pub const MAX_NAME: u32 = xdr::MAX_STRING;

/// Bytes a `READDIR` reply may claim, capped so a hostile count cannot make us
/// allocate. Real replies are a few kilobytes.
const MAX_DIR_ENTRIES: usize = 4096;

/// Narrow a 64-bit file position or size to the 32 bits NFSv2 has for it.
///
/// Fails rather than wrapping. A silently truncated offset reads the wrong
/// part of a file and a silently truncated size makes a client stop early or
/// never stop, and both present as corrupt audio rather than as an error.
pub fn checked_offset(offset: u64) -> Result<u32> {
    u32::try_from(offset).map_err(|_| Error::ImplausibleLength {
        what: "an NFSv2 file offset or size (offsets are 32-bit)",
        length: offset,
        limit: MAX_FILE_SIZE,
    })
}

/// An NFSv2 procedure number.
///
/// Meaningful only alongside program 100003: MOUNT's `MNT` is also `1`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Proc(pub u32);

impl Proc {
    /// Do nothing. The cheapest probe that a server is alive.
    pub const NULL: Self = Self(0);
    /// Attributes of one file.
    pub const GETATTR: Self = Self(1);
    /// Set attributes. Never answered; we are read-only.
    pub const SETATTR: Self = Self(2);
    /// Obsolete in RFC 1094 itself.
    pub const ROOT: Self = Self(3);
    /// Resolve one path component. By far the most frequent call.
    pub const LOOKUP: Self = Self(4);
    /// Read a symbolic link. A rekordbox medium has none.
    pub const READLINK: Self = Self(5);
    /// Read a byte range. This is where the audio is.
    pub const READ: Self = Self(6);
    /// Obsolete in RFC 1094 itself.
    pub const WRITECACHE: Self = Self(7);
    /// Write. Never answered.
    pub const WRITE: Self = Self(8);
    /// Create a file. Never answered.
    pub const CREATE: Self = Self(9);
    /// Remove a file. Never answered.
    pub const REMOVE: Self = Self(10);
    /// Rename. Never answered.
    pub const RENAME: Self = Self(11);
    /// Hard link. Never answered.
    pub const LINK: Self = Self(12);
    /// Symbolic link. Never answered.
    pub const SYMLINK: Self = Self(13);
    /// Create a directory. Never answered.
    pub const MKDIR: Self = Self(14);
    /// Remove a directory. Never answered.
    pub const RMDIR: Self = Self(15);
    /// List a directory.
    pub const READDIR: Self = Self(16);
    /// Filesystem statistics.
    pub const STATFS: Self = Self(17);

    /// A name for logs, or `None` for a procedure NFSv2 does not define.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::NULL => "NULL",
            Self::GETATTR => "GETATTR",
            Self::SETATTR => "SETATTR",
            Self::ROOT => "ROOT",
            Self::LOOKUP => "LOOKUP",
            Self::READLINK => "READLINK",
            Self::READ => "READ",
            Self::WRITECACHE => "WRITECACHE",
            Self::WRITE => "WRITE",
            Self::CREATE => "CREATE",
            Self::REMOVE => "REMOVE",
            Self::RENAME => "RENAME",
            Self::LINK => "LINK",
            Self::SYMLINK => "SYMLINK",
            Self::MKDIR => "MKDIR",
            Self::RMDIR => "RMDIR",
            Self::READDIR => "READDIR",
            Self::STATFS => "STATFS",
            _ => return None,
        })
    }
}

impl fmt::Debug for Proc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "nfs2::Proc({})", self.0),
        }
    }
}

/// An NFSv2 status code (RFC 1094 §2.3.1).
///
/// The MOUNT protocol reuses this numbering for its own `fhstatus`, which is
/// why [`crate::rpc::mount`] refers to it rather than defining its own.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Status(pub u32);

impl Status {
    /// Success. Modelled as the `Ok` side of [`NfsResult`], not as an error.
    pub const OK: Self = Self(0);
    /// Not owner.
    pub const PERM: Self = Self(1);
    /// No such file or directory. What a name-matching bug looks like from
    /// the outside, and it says nothing else (O6).
    pub const NOENT: Self = Self(2);
    /// I/O error.
    pub const IO: Self = Self(5);
    /// No such device or address.
    pub const NXIO: Self = Self(6);
    /// Permission denied. On `MNT` this means "try announcing first" rather
    /// than "give up": a player whose export list is scoped per host rather
    /// than to the whole link-local subnet would refuse an unannounced client
    /// (F12).
    pub const ACCES: Self = Self(13);
    /// File exists.
    pub const EXIST: Self = Self(17);
    /// No such device.
    pub const NODEV: Self = Self(19);
    /// Not a directory.
    pub const NOTDIR: Self = Self(20);
    /// Is a directory.
    pub const ISDIR: Self = Self(21);
    /// File too large.
    pub const FBIG: Self = Self(27);
    /// No space left on device.
    pub const NOSPC: Self = Self(28);
    /// Read-only filesystem. What every write procedure would deserve.
    pub const ROFS: Self = Self(30);
    /// Filename too long.
    pub const NAMETOOLONG: Self = Self(63);
    /// Directory not empty.
    pub const NOTEMPTY: Self = Self(66);
    /// Quota exceeded.
    pub const DQUOT: Self = Self(69);
    /// The filehandle no longer refers to anything. What a media swap looks
    /// like from the client side, and the signal to re-`MNT` rather than to
    /// give up. Also what a server that trusts the spec answers a real deck
    /// with, once the deck rewrites the handle (F28).
    pub const STALE: Self = Self(70);
    /// Write cache flushed.
    pub const WFLUSH: Self = Self(99);

    /// A name for logs, or `None` for a status NFSv2 does not define.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::OK => "NFS_OK",
            Self::PERM => "NFSERR_PERM",
            Self::NOENT => "NFSERR_NOENT",
            Self::IO => "NFSERR_IO",
            Self::NXIO => "NFSERR_NXIO",
            Self::ACCES => "NFSERR_ACCES",
            Self::EXIST => "NFSERR_EXIST",
            Self::NODEV => "NFSERR_NODEV",
            Self::NOTDIR => "NFSERR_NOTDIR",
            Self::ISDIR => "NFSERR_ISDIR",
            Self::FBIG => "NFSERR_FBIG",
            Self::NOSPC => "NFSERR_NOSPC",
            Self::ROFS => "NFSERR_ROFS",
            Self::NAMETOOLONG => "NFSERR_NAMETOOLONG",
            Self::NOTEMPTY => "NFSERR_NOTEMPTY",
            Self::DQUOT => "NFSERR_DQUOT",
            Self::STALE => "NFSERR_STALE",
            Self::WFLUSH => "NFSERR_WFLUSH",
            _ => return None,
        })
    }
}

impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "Status({})", self.0),
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// The result of one NFS procedure.
///
/// `Err(status)` is a **well-formed reply reporting a filesystem error**, not
/// a decoding failure — a distinction that matters because on a CDJ's screen
/// an error and an empty folder look identical, so the set of statuses handled
/// is a user-visible surface rather than an internal detail.
pub type NfsResult<T> = core::result::Result<T, ErrorStatus>;

/// A [`Status`] that is not `NFS_OK`.
///
/// Every NFSv2 reply is a status word followed by a body if and only if the
/// status is zero, so "an error whose code is zero" has no wire form at all:
/// it would claim success and then carry no `fattr`, which a client reads as a
/// truncated datagram rather than as an error. That is an easy state to reach
/// by accident — an errno-to-status mapping that falls through to zero — so
/// the inner value is private and [`ErrorStatus::new`] refuses `NFS_OK`.
///
/// The check therefore happens once, where a status is chosen, instead of
/// being owed by every encoder downstream.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorStatus(Status);

impl ErrorStatus {
    // Every [`Status`] but `OK`, so that choosing an error is always a
    // constant here rather than a fallible conversion at the call site.
    /// Not owner.
    pub const PERM: Self = Self(Status::PERM);
    /// No such file or directory.
    pub const NOENT: Self = Self(Status::NOENT);
    /// I/O error.
    pub const IO: Self = Self(Status::IO);
    /// No such device or address.
    pub const NXIO: Self = Self(Status::NXIO);
    /// Permission denied. On `MNT`, "try announcing first" (F12).
    pub const ACCES: Self = Self(Status::ACCES);
    /// File exists.
    pub const EXIST: Self = Self(Status::EXIST);
    /// No such device.
    pub const NODEV: Self = Self(Status::NODEV);
    /// Not a directory.
    pub const NOTDIR: Self = Self(Status::NOTDIR);
    /// Is a directory.
    pub const ISDIR: Self = Self(Status::ISDIR);
    /// File too large — a file past NFSv2's 32-bit ceiling, which is the one
    /// error a server on a modern filesystem can genuinely hit.
    pub const FBIG: Self = Self(Status::FBIG);
    /// No space left on device.
    pub const NOSPC: Self = Self(Status::NOSPC);
    /// Read-only filesystem, which every write procedure would deserve.
    pub const ROFS: Self = Self(Status::ROFS);
    /// Filename too long.
    pub const NAMETOOLONG: Self = Self(Status::NAMETOOLONG);
    /// Directory not empty.
    pub const NOTEMPTY: Self = Self(Status::NOTEMPTY);
    /// Quota exceeded.
    pub const DQUOT: Self = Self(Status::DQUOT);
    /// The handle refers to nothing. What a media swap looks like, and what a
    /// server that trusts the spec answers a real deck with (F28).
    pub const STALE: Self = Self(Status::STALE);
    /// Write cache flushed.
    pub const WFLUSH: Self = Self(Status::WFLUSH);

    /// `None` for `NFS_OK`, which is not an error and has a body.
    pub fn new(status: Status) -> Option<Self> {
        if status == Status::OK {
            None
        } else {
            Some(Self(status))
        }
    }

    /// The status word this puts on the wire. Never zero.
    pub fn status(self) -> Status {
        self.0
    }
}

impl From<ErrorStatus> for Status {
    fn from(status: ErrorStatus) -> Self {
        status.status()
    }
}

impl fmt::Debug for ErrorStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for ErrorStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// What kind of thing a filehandle refers to.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FType(pub u32);

impl FType {
    /// Non-file.
    pub const NON: Self = Self(0);
    /// A regular file.
    pub const REG: Self = Self(1);
    /// A directory.
    pub const DIR: Self = Self(2);
    /// A block device.
    pub const BLK: Self = Self(3);
    /// A character device.
    pub const CHR: Self = Self(4);
    /// A symbolic link.
    pub const LNK: Self = Self(5);

    /// A name for logs, or `None` for a type NFSv2 does not define.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::NON => "NFNON",
            Self::REG => "NFREG",
            Self::DIR => "NFDIR",
            Self::BLK => "NFBLK",
            Self::CHR => "NFCHR",
            Self::LNK => "NFLNK",
            _ => return None,
        })
    }
}

impl fmt::Debug for FType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "FType({})", self.0),
        }
    }
}

/// The 32 bytes RFC 1094 says are opaque and a CDJ says are twelve.
///
/// As a **client** this is a token: echo back exactly what the server gave,
/// never parse or normalise it. As a **server**, only [`FileHandle::key`] may
/// be relied on — see the module documentation for the capture that settles it
/// (F28).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileHandle(pub [u8; FHANDLE_LEN]);

/// The leading twelve bytes of a filehandle: the part a CDJ preserves.
///
/// A distinct type from [`FileHandle`] on purpose. A server's handle table is
/// keyed on this, so "I looked the handle up correctly" is a property of the
/// type rather than of remembering to slice. Twelve bytes of a truncated
/// SHA-256 of a path is ample to stay collision-free and deterministic.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileHandleKey(pub [u8; FileHandle::KEY_LEN]);

impl FileHandle {
    /// Bytes a filehandle occupies on the wire.
    pub const LEN: usize = FHANDLE_LEN;
    /// Bytes of it a CDJ preserves (F28).
    pub const KEY_LEN: usize = 12;

    /// A handle of all zeroes, for a caller building one field at a time.
    pub const ZERO: Self = Self([0; FHANDLE_LEN]);

    /// The leading four bytes as a big-endian word, which is what a CDJ puts
    /// in the matching [`Fattr::fileid`].
    ///
    /// True in 8285 of 8285 observed replies: a `GETATTR` or `READ` reply's
    /// `fileid` is the first word of the handle in the *call*, and a `LOOKUP`
    /// reply's is the first word of the handle it is *returning*. A server
    /// that derives `fileid` this way is consistent with the hardware for
    /// free.
    pub fn fileid(&self) -> u32 {
        let mut word = [0u8; 4];
        for (slot, byte) in word.iter_mut().zip(self.0.iter()) {
            *slot = *byte;
        }
        u32::from_be_bytes(word)
    }

    /// The part a server may rely on.
    pub fn key(&self) -> FileHandleKey {
        let mut key = [0u8; Self::KEY_LEN];
        for (slot, byte) in key.iter_mut().zip(self.0.iter()) {
            *slot = *byte;
        }
        FileHandleKey(key)
    }

    /// The whole handle.
    pub fn as_bytes(&self) -> &[u8; FHANDLE_LEN] {
        &self.0
    }

    /// Adopt exactly 32 bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let array: [u8; FHANDLE_LEN] = bytes.try_into().map_err(|_| {
            Error::malformed(
                0,
                format!(
                    "a filehandle is {FHANDLE_LEN} bytes, got {len}",
                    len = bytes.len()
                ),
            )
        })?;
        Ok(Self(array))
    }

    /// Build a handle from a key, zero-filling the twenty bytes a CDJ would
    /// overwrite anyway.
    pub fn from_key(key: FileHandleKey) -> Self {
        let mut bytes = [0u8; FHANDLE_LEN];
        for (slot, byte) in bytes.iter_mut().zip(key.0.iter()) {
            *slot = *byte;
        }
        Self(bytes)
    }
}

impl From<FileHandleKey> for FileHandle {
    fn from(key: FileHandleKey) -> Self {
        Self::from_key(key)
    }
}

impl From<FileHandle> for FileHandleKey {
    fn from(handle: FileHandle) -> Self {
        handle.key()
    }
}

/// Renders the twelve bytes a CDJ keeps, a separator, then the twenty it does
/// not — so a log makes the truncation visible instead of hiding it.
impl fmt::Debug for FileHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kept, rewritten) = self.0.split_at(Self::KEY_LEN);
        for byte in kept {
            write!(f, "{byte:02x}")?;
        }
        f.write_str("|")?;
        for byte in rewritten {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for FileHandleKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// An opaque position in a directory listing.
///
/// Opaque to the *client*; the server mints it, so a plain index is as good as
/// anything. All zeroes means "from the beginning".
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Cookie(pub [u8; 4]);

impl Cookie {
    /// "From the beginning."
    pub const START: Self = Self([0; 4]);
}

impl fmt::Debug for Cookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cookie:{:02x?}", self.0)
    }
}

/// NFSv2 file attributes: 17 consecutive 32-bit words, 68 bytes.
///
/// Only `ftype` and `size` are load-bearing. `size` decides how many `READ`s a
/// client issues and when it stops, so a server that reports it wrongly
/// truncates or hangs the transfer; everything else is decoded because it
/// costs nothing and a server must emit plausible values for all of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fattr {
    /// File, directory or something else.
    pub ftype: FType,
    /// Unix mode bits.
    pub mode: u32,
    /// Hard-link count.
    pub nlink: u32,
    /// Owner. Zero — a rekordbox export is ownerless as far as a deck cares.
    pub uid: u32,
    /// Group. Zero, likewise.
    pub gid: u32,
    /// **Load-bearing.** The file's length in bytes.
    pub size: u32,
    /// Preferred transfer block size.
    pub blocksize: u32,
    /// Device number, for a device node.
    pub rdev: u32,
    /// Blocks allocated.
    pub blocks: u32,
    /// Filesystem identifier.
    pub fsid: u32,
    /// Identifier unique within the filesystem.
    pub fileid: u32,
    /// Last access, seconds.
    pub atime_sec: u32,
    /// Last access, microseconds.
    pub atime_usec: u32,
    /// Last modification, seconds.
    pub mtime_sec: u32,
    /// Last modification, microseconds.
    pub mtime_usec: u32,
    /// Last status change, seconds.
    pub ctime_sec: u32,
    /// Last status change, microseconds.
    pub ctime_usec: u32,
}

impl Fattr {
    /// Bytes a `fattr` occupies on the wire.
    pub const WIRE_LEN: usize = 68;

    /// The block size a CDJ reports. 401 of 401 observed `LOOKUP` and
    /// `GETATTR` replies.
    pub const BLOCK_SIZE: u32 = 512;

    /// Mode bits a CDJ reports for a directory: `S_IFDIR` with `0666`.
    ///
    /// Not `0755`, and the execute bits a directory needs to be traversable
    /// are absent. Reproduced rather than corrected — 290 of 290 observed
    /// directory replies.
    pub const DIR_MODE: u32 = 0o040_666;

    /// Mode bits a CDJ reports for a file: `S_IFREG` with **no permission
    /// bits at all**.
    ///
    /// Every file on a deck's medium appears mode `000`, in 97 of 97 observed
    /// `LOOKUP` replies and 14 of 14 `GETATTR` replies. A client that checks
    /// permission bits before reading refuses every track; a real deck
    /// evidently does not check, since decks read from each other happily.
    ///
    /// Reproduced because the goal is to be indistinguishable from a player.
    /// The reference implementation served `0o100644` instead and a real
    /// CDJ-2000NXS loaded and played all four container formats from it
    /// (F39), so this particular field is not load-bearing — but a plausible
    /// substitution is exactly what has broken playback twice elsewhere, and
    /// the observed value costs nothing.
    pub const FILE_MODE: u32 = 0o100_000;

    /// `rdev` a CDJ reports — **1, not 0**, on a regular file. 401 of 401
    /// observed replies. Reproduced without being understood.
    pub const RDEV: u32 = 1;

    /// `fsid` a CDJ reports. The same `2` for both the `/B/` and `/C/`
    /// mounts, so it does not distinguish media. Reproduced without being
    /// understood.
    pub const FSID: u32 = 2;

    /// The timestamp a CDJ puts on every file: `1672531200`, which is
    /// 2023-01-01T00:00:00Z.
    ///
    /// Hard-coded in the firmware. All three of `atime`, `mtime` and `ctime`
    /// carry it, with zero microseconds, on every one of 401 observed replies
    /// — a deck never reports a real file time. Pass this to the constructors
    /// below to be byte-identical to a player.
    pub const EPOCH: u32 = 1_672_531_200;

    /// Attributes for a directory, shaped as a CDJ shapes them.
    ///
    /// A deck reports `size` 0 and `blocks` 1 for every directory, whatever it
    /// contains.
    pub fn directory(fileid: u32, mtime_sec: u32) -> Self {
        Self {
            ftype: FType::DIR,
            mode: Self::DIR_MODE,
            nlink: 1,
            uid: 0,
            gid: 0,
            size: 0,
            blocksize: Self::BLOCK_SIZE,
            rdev: Self::RDEV,
            blocks: 1,
            fsid: Self::FSID,
            fileid,
            atime_sec: mtime_sec,
            atime_usec: 0,
            mtime_sec,
            mtime_usec: 0,
            ctime_sec: mtime_sec,
            ctime_usec: 0,
        }
    }

    /// Attributes for a regular file, shaped as a CDJ shapes them.
    ///
    /// `size` is the one field here that is load-bearing: it decides how many
    /// `READ`s a client issues and when it stops, so reporting it wrongly
    /// truncates or hangs the transfer. It fails for a file past NFSv2's 4 GiB
    /// ceiling rather than reporting a wrapped size — see [`checked_offset`].
    ///
    /// `fileid` should be the leading four bytes of the handle that names this
    /// file: a deck does exactly that, in 8285 of 8285 observed replies. See
    /// [`FileHandle::fileid`].
    pub fn regular_file(fileid: u32, size: u64, mtime_sec: u32) -> Result<Self> {
        let size = checked_offset(size)?;
        Ok(Self {
            ftype: FType::REG,
            mode: Self::FILE_MODE,
            nlink: 1,
            uid: 0,
            gid: 0,
            size,
            blocksize: Self::BLOCK_SIZE,
            rdev: Self::RDEV,
            blocks: size.div_ceil(Self::BLOCK_SIZE),
            fsid: Self::FSID,
            fileid,
            atime_sec: mtime_sec,
            atime_usec: 0,
            mtime_sec,
            mtime_usec: 0,
            ctime_sec: mtime_sec,
            ctime_usec: 0,
        })
    }

    /// Whether this is a directory.
    pub fn is_directory(&self) -> bool {
        self.ftype == FType::DIR
    }

    /// Whether this is a regular file.
    pub fn is_regular_file(&self) -> bool {
        self.ftype == FType::REG
    }

    fn write(&self, out: &mut xdr::Writer) {
        for word in [
            self.ftype.0,
            self.mode,
            self.nlink,
            self.uid,
            self.gid,
            self.size,
            self.blocksize,
            self.rdev,
            self.blocks,
            self.fsid,
            self.fileid,
            self.atime_sec,
            self.atime_usec,
            self.mtime_sec,
            self.mtime_usec,
            self.ctime_sec,
            self.ctime_usec,
        ] {
            out.u32(word);
        }
    }

    fn read(input: &mut xdr::Reader<'_>) -> Result<Self> {
        Ok(Self {
            ftype: FType(input.u32()?),
            mode: input.u32()?,
            nlink: input.u32()?,
            uid: input.u32()?,
            gid: input.u32()?,
            size: input.u32()?,
            blocksize: input.u32()?,
            rdev: input.u32()?,
            blocks: input.u32()?,
            fsid: input.u32()?,
            fileid: input.u32()?,
            atime_sec: input.u32()?,
            atime_usec: input.u32()?,
            mtime_sec: input.u32()?,
            mtime_usec: input.u32()?,
            ctime_sec: input.u32()?,
            ctime_usec: input.u32()?,
        })
    }
}

/// A file or directory a `LOOKUP` resolved to: `diropokres` in RFC 1094.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileRef {
    /// The handle to use for it from here on.
    pub handle: FileHandle,
    /// Its attributes, so a client need not follow up with a `GETATTR`.
    pub attr: Fattr,
}

/// One byte range a `READ` returned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileData<'a> {
    /// The file's attributes after the read, and **a real CDJ does not fill
    /// them in.**
    ///
    /// Every one of 7884 `READ` replies from a CDJ-2000NXS in the corpus
    /// carries `type`, `mode`, `nlink`, `uid`, `gid`, `size`, `blocksize`,
    /// `rdev`, `blocks`, `fsid` and all three timestamps as **zero**, with
    /// only `fileid` populated — the leading four bytes of the handle from the
    /// call, in 7884 of 7884. A client must therefore not read anything out of
    /// this, and in particular must not take `size == 0` for end of file.
    /// `LOOKUP` and `GETATTR` replies do carry a complete, correct `fattr`;
    /// ask one of those instead.
    pub attr: Fattr,
    /// The bytes, borrowed from the datagram or from the server's buffer.
    ///
    /// May be shorter than requested — a server is entitled to return less,
    /// and a client must re-request the shortfall rather than assume the file
    /// ended.
    pub data: &'a [u8],
}

/// One entry in a directory listing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DirEntry {
    /// Matches the `fileid` in this entry's [`Fattr`].
    pub fileid: u32,
    /// UTF-16LE, as everywhere else in this protocol.
    pub name: Utf16LeString,
    /// Where to resume from, if the listing did not finish.
    pub cookie: Cookie,
}

/// What one `READDIR` returned.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Listing {
    /// The entries, in whatever order the server chose.
    pub entries: Vec<DirEntry>,
    /// Whether the directory is exhausted. When false, call again with the
    /// last entry's cookie.
    pub eof: bool,
}

/// Filesystem statistics, as `STATFS` reports them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FsStat {
    /// Optimal transfer size.
    pub tsize: u32,
    /// Block size the counts below are in.
    pub bsize: u32,
    /// Total blocks.
    pub blocks: u32,
    /// Free blocks.
    pub bfree: u32,
    /// Free blocks available to an unprivileged user.
    pub bavail: u32,
}

/// Arguments to `READ`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReadArgs {
    /// The file to read.
    pub handle: FileHandle,
    /// Where to start. 32-bit: see [`checked_offset`].
    pub offset: u32,
    /// How many bytes are wanted. Real CDJs ask for 8192 (F19); a client that
    /// wants to avoid IP fragmentation on a 1500-byte MTU asks for 1280.
    pub count: u32,
    /// Deprecated in RFC 1094 itself and ignored by every server. Sent as
    /// zero; parsed so the argument block is fully accounted for.
    pub total_count: u32,
}

impl ReadArgs {
    /// A read at a 64-bit position, failing past NFSv2's ceiling.
    pub fn at(handle: FileHandle, offset: u64, count: u32) -> Result<Self> {
        Ok(Self {
            handle,
            offset: checked_offset(offset)?,
            count,
            total_count: 0,
        })
    }
}

/// Arguments to `READDIR`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReadDirArgs {
    /// The directory to list.
    pub handle: FileHandle,
    /// Where to resume; [`Cookie::START`] for the beginning.
    pub cookie: Cookie,
    /// Maximum **reply size in bytes**, not a number of entries.
    pub count: u32,
}

/// One NFS call's arguments, dispatched on the procedure number.
///
/// Owns its names, which are at most a few hundred bytes. The reply side
/// borrows instead, because a `READ` payload is 8 KiB and copying it per call
/// is the one allocation worth avoiding on the serving path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Request {
    /// `NULL`: no arguments.
    Null,
    /// `GETATTR`.
    GetAttr(FileHandle),
    /// `LOOKUP`: resolve one component inside one directory.
    Lookup {
        /// The directory to look in.
        dir: FileHandle,
        /// The component, UTF-16LE.
        name: Utf16LeString,
    },
    /// `READ`.
    Read(ReadArgs),
    /// `READDIR`.
    ReadDir(ReadDirArgs),
    /// `STATFS`.
    StatFs(FileHandle),
    /// A procedure this crate does not model, including every write.
    ///
    /// Returned rather than raised so a server can answer `PROC_UNAVAIL`,
    /// which is a real answer — it is what a real CDJ gave a reference client
    /// that tried `READDIR`.
    Unknown {
        /// The procedure number asked for.
        procedure: Proc,
        /// Its argument block, undecoded.
        arguments: Vec<u8>,
    },
}

impl Request {
    /// Which procedure this is.
    pub fn procedure(&self) -> Proc {
        match self {
            Self::Null => Proc::NULL,
            Self::GetAttr(_) => Proc::GETATTR,
            Self::Lookup { .. } => Proc::LOOKUP,
            Self::Read(_) => Proc::READ,
            Self::ReadDir(_) => Proc::READDIR,
            Self::StatFs(_) => Proc::STATFS,
            Self::Unknown { procedure, .. } => *procedure,
        }
    }

    /// Encode the argument block that follows an RPC call header.
    pub fn encode_arguments(&self) -> Vec<u8> {
        let mut out = xdr::Writer::with_capacity(48);
        match self {
            Self::Null => {}
            Self::GetAttr(handle) | Self::StatFs(handle) => out.opaque_fixed(handle.as_bytes()),
            Self::Lookup { dir, name } => {
                out.opaque_fixed(dir.as_bytes());
                out.utf16le_string(name);
            }
            Self::Read(args) => {
                out.opaque_fixed(args.handle.as_bytes());
                out.u32(args.offset);
                out.u32(args.count);
                out.u32(args.total_count);
            }
            Self::ReadDir(args) => {
                out.opaque_fixed(args.handle.as_bytes());
                out.raw(&args.cookie.0);
                out.u32(args.count);
            }
            Self::Unknown { arguments, .. } => out.raw(arguments),
        }
        out.into_bytes()
    }

    /// Decode the argument block of a call to `procedure`.
    pub fn parse(procedure: Proc, arguments: &[u8]) -> Result<Self> {
        let mut input = xdr::Reader::new(arguments);
        Ok(match procedure {
            Proc::NULL => Self::Null,
            Proc::GETATTR => Self::GetAttr(read_handle(&mut input)?),
            Proc::STATFS => Self::StatFs(read_handle(&mut input)?),
            Proc::LOOKUP => Self::Lookup {
                dir: read_handle(&mut input)?,
                name: input.utf16le_string(MAX_NAME)?,
            },
            Proc::READ => Self::Read(ReadArgs {
                handle: read_handle(&mut input)?,
                offset: input.u32()?,
                count: input.u32()?,
                total_count: input.u32()?,
            }),
            Proc::READDIR => Self::ReadDir(ReadDirArgs {
                handle: read_handle(&mut input)?,
                cookie: read_cookie(&mut input)?,
                count: input.u32()?,
            }),
            other => Self::Unknown {
                procedure: other,
                arguments: arguments.to_vec(),
            },
        })
    }
}

fn read_handle(input: &mut xdr::Reader<'_>) -> Result<FileHandle> {
    FileHandle::parse(input.opaque_fixed(FHANDLE_LEN)?)
}

fn read_cookie(input: &mut xdr::Reader<'_>) -> Result<Cookie> {
    let bytes = input.opaque_fixed(4)?;
    let array: [u8; 4] = bytes
        .try_into()
        .map_err(|_| Error::malformed(0, "a READDIR cookie is four bytes"))?;
    Ok(Cookie(array))
}

/// One NFS reply's results, dispatched on the procedure that was called.
///
/// A reply is not self-describing — nothing in the bytes says which procedure
/// they answer — so [`Response::parse`] must be told, which is exactly the
/// bookkeeping the RPC XID exists to make possible.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Response<'a> {
    /// `NULL`: no results.
    Null,
    /// `GETATTR`.
    Attr(NfsResult<Fattr>),
    /// `LOOKUP`.
    Lookup(NfsResult<FileRef>),
    /// `READ`.
    Read(NfsResult<FileData<'a>>),
    /// `READDIR`.
    ReadDir(NfsResult<Listing>),
    /// `STATFS`.
    StatFs(NfsResult<FsStat>),
}

impl Response<'_> {
    /// Which procedure this answers.
    pub fn procedure(&self) -> Proc {
        match self {
            Self::Null => Proc::NULL,
            Self::Attr(_) => Proc::GETATTR,
            Self::Lookup(_) => Proc::LOOKUP,
            Self::Read(_) => Proc::READ,
            Self::ReadDir(_) => Proc::READDIR,
            Self::StatFs(_) => Proc::STATFS,
        }
    }

    /// The status word this reply carries, `NFS_OK` on success.
    pub fn status(&self) -> Status {
        match self {
            Self::Null => Status::OK,
            Self::Attr(result) => status_of(result.as_ref()),
            Self::Lookup(result) => status_of(result.as_ref()),
            Self::Read(result) => status_of(result.as_ref()),
            Self::ReadDir(result) => status_of(result.as_ref()),
            Self::StatFs(result) => status_of(result.as_ref()),
        }
    }

    /// Encode the result block that follows an RPC reply header.
    ///
    /// Every NFSv2 reply but `NULL`'s is a status word followed by a body iff
    /// the status is `NFS_OK`, which is why a failure encodes to exactly four
    /// bytes. `NULL` returns void and encodes to none at all.
    pub fn encode(&self) -> Vec<u8> {
        if matches!(self, Self::Null) {
            return Vec::new();
        }
        // Sized for the payload as well as the header: a READ reply is up to
        // 8 KiB and this is the path where a stall is an audio dropout (F18),
        // so it should not be a sequence of reallocations.
        let payload = match self {
            Self::Read(Ok(read)) => read.data.len() + 4,
            _ => 0,
        };
        let mut out = xdr::Writer::with_capacity(Fattr::WIRE_LEN + 8 + payload);
        out.u32(self.status().0);
        match self {
            Self::Attr(Ok(attr)) => attr.write(&mut out),
            Self::Lookup(Ok(found)) => {
                out.opaque_fixed(found.handle.as_bytes());
                found.attr.write(&mut out);
            }
            Self::Read(Ok(read)) => {
                read.attr.write(&mut out);
                out.opaque_var(read.data);
            }
            Self::ReadDir(Ok(listing)) => {
                for entry in &listing.entries {
                    out.bool(true);
                    out.u32(entry.fileid);
                    out.utf16le_string(&entry.name);
                    out.raw(&entry.cookie.0);
                }
                out.bool(false);
                out.bool(listing.eof);
            }
            Self::StatFs(Ok(stat)) => {
                for word in [stat.tsize, stat.bsize, stat.blocks, stat.bfree, stat.bavail] {
                    out.u32(word);
                }
            }
            // `NULL` has no results at all, and a failure is the status word
            // and nothing else — RFC 1094's unions carry a body only on
            // `NFS_OK`.
            Self::Null
            | Self::Attr(Err(_))
            | Self::Lookup(Err(_))
            | Self::Read(Err(_))
            | Self::ReadDir(Err(_))
            | Self::StatFs(Err(_)) => {}
        }
        out.into_bytes()
    }

    /// Decode the result block of a reply to `procedure`.
    pub fn parse(procedure: Proc, results: &[u8]) -> Result<Response<'_>> {
        if procedure == Proc::NULL {
            return Ok(Response::Null);
        }
        let mut input = xdr::Reader::new(results);
        let status = Status(input.u32()?);
        // `NFS_OK` is the only status with a body, so anything else is an
        // error and `ErrorStatus::new` cannot refuse it.
        if let Some(status) = ErrorStatus::new(status) {
            return Ok(match procedure {
                Proc::GETATTR => Response::Attr(Err(status)),
                Proc::LOOKUP => Response::Lookup(Err(status)),
                Proc::READ => Response::Read(Err(status)),
                Proc::READDIR => Response::ReadDir(Err(status)),
                Proc::STATFS => Response::StatFs(Err(status)),
                other => {
                    return Err(Error::malformed(
                        0,
                        format!("no reply decoder for NFS procedure {other:?}"),
                    ));
                }
            });
        }
        parse_ok(procedure, &mut input)
    }
}

fn parse_ok<'a>(procedure: Proc, input: &mut xdr::Reader<'a>) -> Result<Response<'a>> {
    Ok(match procedure {
        Proc::GETATTR => Response::Attr(Ok(Fattr::read(input)?)),
        Proc::LOOKUP => Response::Lookup(Ok(FileRef {
            handle: read_handle(input)?,
            attr: Fattr::read(input)?,
        })),
        Proc::READ => Response::Read(Ok(FileData {
            attr: Fattr::read(input)?,
            data: input.opaque_var(MAX_READ_PAYLOAD, "a READ payload")?,
        })),
        Proc::READDIR => Response::ReadDir(Ok(parse_listing(input)?)),
        Proc::STATFS => Response::StatFs(Ok(FsStat {
            tsize: input.u32()?,
            bsize: input.u32()?,
            blocks: input.u32()?,
            bfree: input.u32()?,
            bavail: input.u32()?,
        })),
        other => {
            return Err(Error::malformed(
                0,
                format!("no reply decoder for NFS procedure {other:?}"),
            ));
        }
    })
}

fn parse_listing(input: &mut xdr::Reader<'_>) -> Result<Listing> {
    let mut entries = Vec::new();
    while input.bool()? {
        entries.push(DirEntry {
            fileid: input.u32()?,
            name: input.utf16le_string(MAX_NAME)?,
            cookie: read_cookie(input)?,
        });
        // `>`, not `>=`: a listing of exactly the cap is legal.
        if entries.len() > MAX_DIR_ENTRIES {
            return Err(Error::ImplausibleLength {
                what: "a READDIR listing",
                length: u64::try_from(entries.len()).unwrap_or(u64::MAX),
                limit: u64::try_from(MAX_DIR_ENTRIES).unwrap_or(u64::MAX),
            });
        }
    }
    // Some servers omit the trailing eof word. Absent it, the listing is over:
    // an entry list that ended and no more bytes cannot mean "call again".
    let eof = if input.remaining() >= 4 {
        input.bool()?
    } else {
        true
    };
    Ok(Listing { entries, eof })
}

fn status_of<T>(result: core::result::Result<&T, &ErrorStatus>) -> Status {
    match result {
        Ok(_) => Status::OK,
        Err(status) => status.status(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handles from F28, verbatim. Everything about the filehandle design
    /// answers to these forty bytes.
    const SERVED: [u8; 32] = [
        0x8a, 0x5e, 0xda, 0xb2, 0x82, 0x63, 0x24, 0x43, 0x21, 0x9e, 0x05, 0x1e, 0x4a, 0xde, 0x2d,
        0x1d, 0x5b, 0xbc, 0x67, 0x1c, 0x78, 0x10, 0x51, 0xbf, 0x14, 0x37, 0x89, 0x7c, 0xbd, 0xfe,
        0xa0, 0xf1,
    ];
    const RETURNED: [u8; 32] = [
        0x8a, 0x5e, 0xda, 0xb2, 0x82, 0x63, 0x24, 0x43, 0x21, 0x9e, 0x05, 0x1e, 0x03, 0x01, 0x2d,
        0x00, 0x00, 0x00, 0x1b, 0x58, 0x00, 0x00, 0x00, 0x00, 0x03, 0x03, 0x01, 0x00, 0x00, 0x00,
        0x01, 0x62,
    ];

    /// F28, the finding this whole module is shaped around: a server that
    /// compares whole handles browses perfectly and fails when a DJ loads a
    /// track.
    #[test]
    fn a_cdj_rewrites_all_but_the_leading_twelve_bytes_of_a_filehandle() {
        let served = FileHandle(SERVED);
        let returned = FileHandle(RETURNED);
        assert_ne!(served, returned, "the whole handles differ");
        assert_eq!(
            served.key(),
            returned.key(),
            "but the twelve bytes a server may rely on are the same"
        );
        assert_eq!(
            served.key().0,
            [
                0x8a, 0x5e, 0xda, 0xb2, 0x82, 0x63, 0x24, 0x43, 0x21, 0x9e, 0x05, 0x1e
            ]
        );
    }

    #[test]
    fn a_filehandle_debug_shows_where_the_truncation_falls() {
        assert_eq!(
            format!("{:?}", FileHandle(SERVED)),
            "8a5edab282632443219e051e|4ade2d1d5bbc671c781051bf1437897cbdfea0f1"
        );
    }

    #[test]
    fn a_key_round_trips_through_a_zero_padded_handle() {
        let key = FileHandle(SERVED).key();
        let rebuilt = FileHandle::from_key(key);
        assert_eq!(rebuilt.key(), key);
        assert_eq!(
            rebuilt.as_bytes().get(12..),
            Some([0u8; 20].as_slice()),
            "the twenty bytes a deck overwrites are ours to leave empty"
        );
    }

    #[test]
    fn a_filehandle_must_be_exactly_thirty_two_bytes() {
        assert!(FileHandle::parse(&[0; 31]).is_err());
        assert!(FileHandle::parse(&[0; 33]).is_err());
        assert!(FileHandle::parse(&[0; 32]).is_ok());
    }

    /// A LOOKUP is the most frequent call a deck makes, and this is its exact
    /// argument layout: 32 bare handle bytes, then a UTF-16LE name whose
    /// prefix counts bytes.
    #[test]
    fn lookup_arguments_are_a_bare_handle_then_a_utf16le_name() {
        let request = Request::Lookup {
            dir: FileHandle(SERVED),
            name: Utf16LeString::new("Contents"),
        };
        let args = request.encode_arguments();
        assert_eq!(args.get(..32), Some(SERVED.as_slice()), "no length prefix");
        assert_eq!(
            args.get(32..36),
            Some(16u32.to_be_bytes().as_slice()),
            "eight characters announce sixteen bytes"
        );
        assert_eq!(
            args.get(36..52),
            Some(b"C\0o\0n\0t\0e\0n\0t\0s\0".as_slice())
        );
        assert_eq!(args.len(), 52, "16 is already aligned; no padding");
        assert_eq!(Request::parse(Proc::LOOKUP, &args).unwrap(), request);
    }

    #[test]
    fn read_arguments_carry_the_deprecated_total_count() {
        let request = Request::Read(ReadArgs::at(FileHandle(SERVED), 8192, 8192).unwrap());
        let args = request.encode_arguments();
        assert_eq!(args.len(), 32 + 12);
        assert_eq!(args.get(32..36), Some(8192u32.to_be_bytes().as_slice()));
        assert_eq!(
            args.get(36..40),
            Some(8192u32.to_be_bytes().as_slice()),
            "the size a real CDJ asks for (F19)"
        );
        assert_eq!(
            args.get(40..44),
            Some(0u32.to_be_bytes().as_slice()),
            "totalcount was deprecated in RFC 1094 itself"
        );
        assert_eq!(Request::parse(Proc::READ, &args).unwrap(), request);
    }

    /// "Assert the ceiling rather than silently wrapping."
    #[test]
    fn an_offset_past_four_gigabytes_is_refused_rather_than_wrapped() {
        let error = ReadArgs::at(FileHandle::ZERO, 0x1_0000_0000, 1280).unwrap_err();
        assert!(
            matches!(error, Error::ImplausibleLength { .. }),
            "{error:?}"
        );
        assert!(ReadArgs::at(FileHandle::ZERO, MAX_FILE_SIZE, 1).is_ok());
    }

    #[test]
    fn a_file_past_four_gigabytes_cannot_be_described() {
        assert!(Fattr::regular_file(1, MAX_FILE_SIZE + 1, 0).is_err());
        let attr = Fattr::regular_file(1, 7_633_531, 0).unwrap();
        assert_eq!(attr.size, 7_633_531, "the size F18 measured");
        assert_eq!(attr.blocks, 14_910, "7633531 / 512, rounded up");
        assert!(attr.is_regular_file());
    }

    #[test]
    fn a_fattr_is_seventeen_words() {
        let attr = Fattr::directory(2, 0x5f5e_1000);
        let response = Response::Attr(Ok(attr));
        let encoded = response.encode();
        assert_eq!(
            encoded.len(),
            4 + Fattr::WIRE_LEN,
            "a status word plus 68 bytes"
        );
        assert_eq!(
            Response::parse(Proc::GETATTR, &encoded).unwrap(),
            Response::Attr(Ok(attr))
        );
    }

    #[test]
    fn an_error_reply_is_four_bytes_and_nothing_else() {
        let encoded = Response::Lookup(Err(ErrorStatus::NOENT)).encode();
        assert_eq!(encoded, [0, 0, 0, 2]);
        assert_eq!(
            Response::parse(Proc::LOOKUP, &encoded).unwrap(),
            Response::Lookup(Err(ErrorStatus::NOENT))
        );
    }

    /// "Error, code zero" has no wire form: the status word would say success
    /// and no body would follow, which a client reads as a truncated datagram
    /// rather than as an error. An errno mapping that falls through to zero is
    /// an easy way to reach that, so the type refuses it.
    #[test]
    fn an_error_status_cannot_be_nfs_ok() {
        assert_eq!(ErrorStatus::new(Status::OK), None);
        assert_eq!(
            ErrorStatus::new(Status::NOENT),
            Some(ErrorStatus::NOENT),
            "and every non-zero status is one"
        );
        assert_eq!(
            ErrorStatus::new(Status(12_345)).map(ErrorStatus::status),
            Some(Status(12_345)),
            "including a status NFSv2 does not define"
        );
        // Which means every error reply this type can express re-parses.
        for status in [ErrorStatus::NOENT, ErrorStatus::STALE, ErrorStatus::ACCES] {
            let encoded = Response::Lookup(Err(status)).encode();
            assert_eq!(encoded.len(), 4);
            assert_eq!(
                Response::parse(Proc::LOOKUP, &encoded).unwrap(),
                Response::Lookup(Err(status))
            );
        }
    }

    #[test]
    fn a_stale_handle_is_told_apart_from_a_missing_name() {
        // Both are errors on the wire and they mean different things: STALE
        // tells a deck to re-MNT, NOENT that the file is gone.
        assert_eq!(Status::STALE.0, 70);
        assert_eq!(Status::NOENT.0, 2);
        assert_eq!(format!("{:?}", Status::STALE), "NFSERR_STALE");
    }

    #[test]
    fn a_lookup_reply_is_a_handle_then_attributes() {
        let found = FileRef {
            handle: FileHandle(SERVED),
            attr: Fattr::regular_file(9, 6_942_380, 0).unwrap(),
        };
        let encoded = Response::Lookup(Ok(found)).encode();
        assert_eq!(encoded.len(), 4 + 32 + Fattr::WIRE_LEN);
        assert_eq!(encoded.get(4..36), Some(SERVED.as_slice()));
        let parsed = Response::parse(Proc::LOOKUP, &encoded).unwrap();
        assert_eq!(parsed, Response::Lookup(Ok(found)));
        // The size F29 confirmed correct on hardware.
        let Response::Lookup(Ok(reference)) = parsed else {
            panic!("expected a lookup result");
        };
        assert_eq!(reference.attr.size, 6_942_380);
    }

    #[test]
    fn a_read_reply_borrows_its_payload() {
        let attr = Fattr::regular_file(1, 8192, 0).unwrap();
        let payload = vec![0xa5; 8192];
        let encoded = Response::Read(Ok(FileData {
            attr,
            data: &payload,
        }))
        .encode();
        assert_eq!(encoded.len(), 4 + Fattr::WIRE_LEN + 4 + 8192);
        let parsed = Response::parse(Proc::READ, &encoded).unwrap();
        let Response::Read(Ok(data)) = parsed else {
            panic!("expected a read result");
        };
        assert_eq!(data.data.len(), 8192, "the NFSv2 maximum (F19)");
        assert_eq!(data.attr, attr);
    }

    #[test]
    fn a_short_read_is_representable_because_a_server_may_return_less() {
        let attr = Fattr::regular_file(1, 100, 0).unwrap();
        let encoded = Response::Read(Ok(FileData {
            attr,
            data: &[1, 2, 3],
        }))
        .encode();
        let Response::Read(Ok(data)) = Response::parse(Proc::READ, &encoded).unwrap() else {
            panic!("expected a read result");
        };
        assert_eq!(data.data, &[1, 2, 3]);
        assert_eq!(data.attr.size, 100, "the file is longer than the reply");
    }

    #[test]
    fn a_read_payload_over_the_nfsv2_maximum_is_refused() {
        let mut out = xdr::Writer::new();
        out.u32(Status::OK.0);
        Fattr::directory(1, 0).write(&mut out);
        out.u32(0xffff_ffff);
        let error = Response::parse(Proc::READ, out.as_bytes()).unwrap_err();
        assert!(
            matches!(error, Error::ImplausibleLength { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_readdir_listing_round_trips_with_a_non_ascii_name() {
        let listing = Listing {
            entries: vec![
                DirEntry {
                    fileid: 3,
                    name: Utf16LeString::new("PIONEER"),
                    cookie: Cookie([0, 0, 0, 1]),
                },
                DirEntry {
                    fileid: 4,
                    name: Utf16LeString::new("カガミ"),
                    cookie: Cookie([0, 0, 0, 2]),
                },
            ],
            eof: true,
        };
        let encoded = Response::ReadDir(Ok(listing.clone())).encode();
        assert_eq!(
            Response::parse(Proc::READDIR, &encoded).unwrap(),
            Response::ReadDir(Ok(listing))
        );
    }

    #[test]
    fn an_empty_directory_is_not_an_error() {
        let encoded = Response::ReadDir(Ok(Listing {
            entries: Vec::new(),
            eof: true,
        }))
        .encode();
        assert_eq!(
            encoded,
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            "status, no-more-entries, eof"
        );
        let Response::ReadDir(Ok(listing)) = Response::parse(Proc::READDIR, &encoded).unwrap()
        else {
            panic!("expected a listing");
        };
        assert!(listing.entries.is_empty() && listing.eof);
    }

    #[test]
    fn readdir_arguments_carry_a_bare_four_byte_cookie() {
        let request = Request::ReadDir(ReadDirArgs {
            handle: FileHandle(SERVED),
            cookie: Cookie::START,
            count: 4096,
        });
        let args = request.encode_arguments();
        assert_eq!(args.len(), 32 + 4 + 4);
        assert_eq!(args.get(32..36), Some([0, 0, 0, 0].as_slice()));
        assert_eq!(args.get(36..40), Some(4096u32.to_be_bytes().as_slice()));
        assert_eq!(Request::parse(Proc::READDIR, &args).unwrap(), request);
    }

    #[test]
    fn statfs_reports_five_words() {
        let stat = FsStat {
            tsize: 8192,
            bsize: 512,
            blocks: 1_000_000,
            bfree: 500_000,
            bavail: 500_000,
        };
        let encoded = Response::StatFs(Ok(stat)).encode();
        assert_eq!(encoded.len(), 4 + 20);
        assert_eq!(
            Response::parse(Proc::STATFS, &encoded).unwrap(),
            Response::StatFs(Ok(stat))
        );
    }

    #[test]
    fn getattr_and_statfs_take_a_bare_handle() {
        for request in [
            Request::GetAttr(FileHandle(SERVED)),
            Request::StatFs(FileHandle(SERVED)),
        ] {
            let args = request.encode_arguments();
            assert_eq!(args, SERVED, "32 bytes, no prefix, no padding");
            assert_eq!(Request::parse(request.procedure(), &args).unwrap(), request);
        }
    }

    #[test]
    fn a_write_procedure_decodes_as_unknown_so_it_can_be_refused_politely() {
        let request = Request::parse(Proc::WRITE, &[1, 2, 3, 4]).unwrap();
        assert_eq!(request.procedure(), Proc::WRITE);
        assert!(matches!(request, Request::Unknown { .. }));
        assert_eq!(request.encode_arguments(), [1, 2, 3, 4]);
    }

    #[test]
    fn a_truncated_lookup_is_truncation_not_garbage() {
        let error = Request::parse(Proc::LOOKUP, &[0; 20]).unwrap_err();
        assert!(error.is_truncated(), "{error:?}");
    }

    #[test]
    fn null_carries_nothing_in_either_direction() {
        // Not even a status word: RFC 1094's NFSPROC_NULL returns void, and a
        // stray zero here would be decoded as a one-field result by anything
        // expecting one.
        assert!(Request::Null.encode_arguments().is_empty());
        assert!(Response::Null.encode().is_empty());
        assert_eq!(Request::parse(Proc::NULL, &[]).unwrap(), Request::Null);
        assert_eq!(Response::parse(Proc::NULL, &[]).unwrap(), Response::Null);
    }

    #[test]
    fn the_observed_ports_and_program_numbers() {
        // F6/F10: three devices, same numbers.
        assert_eq!(PROGRAM.0, 100_003);
        assert_eq!(VERSION, 2);
        assert_eq!(PORT, 2049);
        assert_eq!(MAX_DATA, 8192);
    }
}
