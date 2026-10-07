// SPDX-License-Identifier: GPL-3.0-only

//! ONC RPC v2, the portmapper, MOUNT and NFSv2 — UDP 111, 48276 and 2049.
//!
//! This is how audio actually moves. A CDJ serves the contents of its SD card
//! and USB stick over NFS version 2, and a deck loading a track off a peer
//! reads the audio file itself — progressively, seeking on demand, ~38% of a
//! 7.6 MB MP3 touched during one load plus thirty seconds of playback (F18).
//! dbserver supplies metadata, waveforms and cues; the samples come over
//! `READ`. Nothing in the pre-hardware literature states this.
//!
//! Everything here works in both directions. We are a client — mounting a
//! player's export, walking to a file one `LOOKUP` per component, streaming
//! reads — and we are a server, because a real deck will play from us if we
//! answer the same calls. Each procedure therefore has all four halves: build
//! a call, parse a call, build a reply, parse a reply.
//!
//! ```text
//! rpc          RPC v2 framing: the call and reply headers, AUTH_UNIX
//! rpc::xdr     XDR, and Pioneer's UTF-16LE deviation from it
//! rpc::portmap program 100000 v2, UDP 111   — GETPORT, DUMP
//! rpc::mount   program 100005 v1, UDP 48276 — MNT, UMNT, EXPORT
//! rpc::nfs2    program 100003 v2, UDP 2049  — LOOKUP, READ, GETATTR, …
//! ```
//!
//! # Five things that are easy to get wrong
//!
//! **Names are UTF-16 little-endian, counted in bytes.** Not the ASCII that
//! standard NFS uses. See [`xdr`], which explains it at length; it is the
//! single most important non-standard fact about this path, and it is why a
//! stock NFS library is no use.
//!
//! **The `AUTH_UNIX` stamp is neither a magic constant nor a nonce.** It is a
//! fixed sequence indexed by the number of RPC calls a device has made since
//! power-on, identical across devices and across a decade of firmware — the
//! same xid always carries the same stamp, in 9947 recurrences across separate
//! captures with no exceptions. That supersedes both published readings; see
//! [`AuthUnix::stamp`] and [`STAMP_SEQUENCE`]. The rest of the credential is
//! exactly as documented — `machine_name=""`, `uid=0`, `gid=0`, no
//! supplementary gids, in 56,966 of 56,966 calls.
//!
//! **A portmapper on UDP/111 is mandatory to be browsable at all.** With
//! nothing on 111, a deck retries `GETPORT` once a second indefinitely, never
//! falls back to the well-known 48276 and 2049 even when both are bound and
//! idle, never opens the dbserver port query on 12523, and so never lists us
//! (F46). The observed order of a real load is the reverse of what the
//! literature implies:
//!
//! ```text
//! media query 0x05 ─► portmap GETPORT ─► MNT ─► 12523 ─► dbserver ─► READ
//!      t=7.6s              t=44.09s              t=44.11s          t=52s
//! ```
//!
//! **A CDJ does not treat the filehandle as opaque.** RFC 1094 says 32 bytes
//! echoed back verbatim; a CDJ-2000NXS keeps the leading twelve and overwrites
//! the rest with its own file reference (F28). A server that trusts the spec
//! browses perfectly and fails at the moment a DJ loads a track. See
//! [`FileHandle`] and [`FileHandleKey`].
//!
//! **Offsets and sizes are 32-bit**, so NFSv2 cannot address past 4 GiB. The
//! ceiling is asserted rather than silently wrapped — see
//! [`nfs2::checked_offset`].
//!
//! # Credentials are parsed, not enforced
//!
//! A player exports its media to the whole link-local subnet
//! (`169.254.0.0/255.255.0.0`), which is why a host that has never announced
//! itself can still read a deck's files (F11, F12). Our server decodes
//! credentials for the record and acts on none of them: being stricter than
//! the hardware we are impersonating could only make us the reason a real deck
//! fails.
//!
//! # Provenance
//!
//! The call direction was validated against 8415 real calls through the Kaitai
//! schema `ksy/prolink_rpc.ksy`; the reply direction against two CDJ-2000NXS
//! running firmware 1.44 across the `S10*` and `S24*` serve sessions.

pub mod mount;
pub mod nfs2;
pub mod portmap;
pub mod xdr;

use std::fmt;

use crate::{Error, Result};

pub use nfs2::{FileHandle, FileHandleKey};

/// The only version of ONC RPC anything here speaks (RFC 1057).
pub const VERSION: u32 = 2;

/// `msg_type` for a call.
const MSG_CALL: u32 = 0;
/// `msg_type` for a reply.
const MSG_REPLY: u32 = 1;

/// `reply_stat` values.
const MSG_ACCEPTED: u32 = 0;
const MSG_DENIED: u32 = 1;

/// A call's correlation token.
///
/// Echoed verbatim in the reply and otherwise meaningless. Notably it is *not*
/// a sequence number a server may validate: a client is free to pick any
/// value, and a retransmission deliberately reuses the original so that a late
/// first reply and the retry's reply are recognisable as duplicates of one
/// another rather than as two answers.
///
/// A CDJ nonetheless makes it look like one. It starts at `1` at boot and
/// increments by one per call from **a single global counter** — shared across
/// all three programs and across every source port it uses, not per socket and
/// not random. Two captures are monotonically non-decreasing end to end.
/// Reproducing that would make us look more like a player; relying on it in a
/// server would not, since nothing in RFC 1057 obliges any other client to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Xid(pub u32);

impl fmt::Debug for Xid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "xid:{:#010x}", self.0)
    }
}

/// An RPC program number.
///
/// A newtype rather than an enum: the three programs below are the ones a CDJ
/// runs, but a `DUMP` from some other device may list `100024` (`status`) or
/// anything else, and a decoder that refused an unfamiliar number would take
/// out the listing it was asked for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Program(pub u32);

impl Program {
    /// The portmapper, the only program at a well-known port.
    pub const PORTMAP: Self = Self(100_000);
    /// NFS.
    pub const NFS: Self = Self(100_003);
    /// The MOUNT protocol.
    pub const MOUNT: Self = Self(100_005);
    /// The NFS status monitor. Never seen on a CDJ; listed by other hosts.
    pub const STATUS: Self = Self(100_024);

    /// The name `rpcinfo` prints, or `None` for a program we do not know.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::PORTMAP => "portmapper",
            Self::NFS => "nfs",
            Self::MOUNT => "mountd",
            Self::STATUS => "status",
            _ => return None,
        })
    }
}

impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "Program({})", self.0),
        }
    }
}

/// Byte-level transport, for a mapping's `protocol` field.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct IpProtocol(pub u32);

impl IpProtocol {
    /// TCP. Never used by any Pro DJ Link RPC program.
    pub const TCP: Self = Self(6);
    /// UDP. Everything here.
    pub const UDP: Self = Self(17);

    /// The name `rpcinfo` prints, or `None` for a protocol we do not know.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::TCP => "tcp",
            Self::UDP => "udp",
            _ => return None,
        })
    }
}

impl fmt::Debug for IpProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "IpProtocol({})", self.0),
        }
    }
}

/// The authentication flavour of a credential or verifier (RFC 1057 §7.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthFlavor(pub u32);

impl AuthFlavor {
    /// No authentication; an empty body. Every verifier we have observed, in
    /// both directions.
    pub const NULL: Self = Self(0);
    /// The credential a real player sends, and the one we send.
    pub const UNIX: Self = Self(1);
    /// A server-minted short-hand for a previously sent `AUTH_UNIX`. Never
    /// seen here.
    pub const SHORT: Self = Self(2);
    /// DES. Never seen here.
    pub const DES: Self = Self(3);

    /// A name for logs, or `None` for a flavour we have never observed.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::NULL => "AUTH_NULL",
            Self::UNIX => "AUTH_UNIX",
            Self::SHORT => "AUTH_SHORT",
            Self::DES => "AUTH_DES",
            _ => return None,
        })
    }
}

impl fmt::Debug for AuthFlavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "AuthFlavor({})", self.0),
        }
    }
}

/// An `opaque_auth`: a flavour and an uninterpreted body (RFC 1057 §7.2).
///
/// The body is left undecoded because its shape depends on the flavour and
/// because nothing in this protocol acts on it. [`AuthUnix::parse`] decodes it
/// when someone wants to look.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Auth<'a> {
    /// Which flavour the body is in.
    pub flavor: AuthFlavor,
    /// The body, at most [`xdr::MAX_AUTH_BODY`] bytes.
    pub body: &'a [u8],
}

impl Auth<'_> {
    /// The empty `AUTH_NULL` credential, and every verifier we have observed.
    pub const NULL: Auth<'static> = Auth {
        flavor: AuthFlavor::NULL,
        body: &[],
    };
}

impl<'a> Auth<'a> {
    /// An `AUTH_UNIX` credential over an already-encoded body.
    pub fn unix(body: &'a [u8]) -> Self {
        Self {
            flavor: AuthFlavor::UNIX,
            body,
        }
    }

    fn write(&self, out: &mut xdr::Writer) {
        out.u32(self.flavor.0);
        out.opaque_var(self.body);
    }

    fn read(input: &mut xdr::Reader<'a>) -> Result<Self> {
        let flavor = AuthFlavor(input.u32()?);
        let body = input.opaque_var(xdr::MAX_AUTH_BODY, "an opaque_auth body")?;
        Ok(Self { flavor, body })
    }
}

/// The `AUTH_UNIX` credential body (RFC 1057 §9.2).
///
/// A CDJ's exports are readable by the whole link-local subnet, so these are
/// not really checked — but both working reference clients send `AUTH_UNIX`,
/// nobody has demonstrated `AUTH_NULL` working, and the one recorded attempt
/// hit `NFSERR_ACCES` from a library that defaults to `AUTH_NULL`. Sending
/// `AUTH_UNIX` is the cheap, known-good choice.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct AuthUnix {
    /// **A fixed sequence indexed by the call number since boot** — not a
    /// magic constant, and not a nonce either.
    ///
    /// Two readings of this field have been published and both are wrong. The
    /// pre-hardware literature calls it a magic constant that clients copy to
    /// look like a real CDJ, citing `0x967b8703`; C8 corrected that to "a
    /// per-call nonce, its value arbitrary", because consecutive calls carry
    /// different values. Neither survives the whole corpus.
    ///
    /// Across 56,102 Pioneer-originated calls, **9947 xids recur in two or
    /// more separate captures and every one carries the same stamp both
    /// times — zero disagreements.** Two physically different CDJ-2000NXS
    /// units agree with each other, and both agree with a 2016 capture of
    /// entirely different hardware on xids 1 through 9. Since a deck's xid is
    /// a boot-relative counter starting at 1, the stamp is a function of how
    /// many RPC calls the device has made since power-on. Presumably a
    /// pseudo-random generator seeded identically at boot — that mechanism is
    /// inference; the mapping is measurement. See [`STAMP_SEQUENCE`].
    ///
    /// C8's *practical* advice survives intact and is now better grounded: a
    /// server must never validate this, and a client may send anything. What
    /// changes is that a test may assert an exact stamp for a given call
    /// index, and that `0x967b8703` really is a constant — the first entry.
    pub stamp: u32,
    /// Empty on every call ever observed.
    pub machine_name: String,
    /// Zero on every call ever observed.
    pub uid: u32,
    /// Zero on every call ever observed.
    pub gid: u32,
    /// Empty on every call ever observed.
    pub gids: Vec<u32>,
}

/// The `AUTH_UNIX` stamp a Pioneer device sends on its **nth** RPC call since
/// power-on, for the first forty calls.
///
/// A table of observed values, not a formula — the generator behind it is not
/// known. Every entry was witnessed in two to four independent captures and by
/// two or three physically distinct devices, with no entry ever disagreeing
/// with itself. Entry one is the `0x967b8703` the literature calls magic; it
/// is magic only in the sense that a freshly booted deck's first call carries
/// it, which is why a client hard-coding it works.
///
/// Nothing validates the stamp, so this exists for exactness rather than
/// correctness: a client that walks this table looks like a deck that has just
/// been switched on. Past the fortieth call we have no table, and any value
/// will do. See [`stamp_for_xid`].
pub const STAMP_SEQUENCE: [u32; 40] = [
    0x967b_8703,
    0x9922_e112,
    0xa492_1306,
    0xdc1a_c513,
    0x2a99_4a03,
    0x0d98_8520,
    0xf062_9c32,
    0xbc69_0310,
    0xf0af_010a,
    0x177a_4420,
    0xe0f3_9f14,
    0xb0b6_3114,
    0x6432_2e08,
    0x561e_c900,
    0xd040_9301,
    0x0e05_c008,
    0xceb5_1a0d,
    0x9250_8c07,
    0xb8be_fb0d,
    0xa95e_e233,
    0xfec9_9811,
    0xb08c_e104,
    0xe8ef_9718,
    0x3c8e_f516,
    0xf156_f120,
    0xef2d_6300,
    0x44a2_ed03,
    0x9cc9_8c03,
    0x44f3_b503,
    0x8d96_1401,
    0x1fde_921f,
    0x2087_c50b,
    0x1a37_e602,
    0xf0bc_7107,
    0x66ba_6b07,
    0x1497_0216,
    0x6d58_c300,
    0xa29d_ce09,
    0x70d2_b810,
    0xad14_5926,
];

/// The stamp a freshly booted Pioneer device sends on its very first RPC call,
/// and the value one reference client hard-codes believing it to be magic.
pub const STAMP_FIRST_CALL: u32 = 0x967b_8703;

/// The stamp a Pioneer device would send with `xid`, for an xid inside
/// [`STAMP_SEQUENCE`].
///
/// `None` past the end of the observed table, and for `xid` zero — a deck's
/// counter starts at one. Nothing inspects the stamp, so `None` is a licence
/// to send anything rather than a problem.
pub fn stamp_for_xid(xid: Xid) -> Option<u32> {
    let index = usize::try_from(xid.0.checked_sub(1)?).ok()?;
    STAMP_SEQUENCE.get(index).copied()
}

impl AuthUnix {
    /// The credential a player sends: an empty machine name, root, no
    /// supplementary groups, and `stamp` — for which [`stamp_for_xid`] gives
    /// the value a real deck would have used.
    pub fn cdj(stamp: u32) -> Self {
        Self {
            stamp,
            machine_name: String::new(),
            uid: 0,
            gid: 0,
            gids: Vec::new(),
        }
    }

    /// Encode the body. Wrap the result in [`Auth::unix`] to use it.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = xdr::Writer::with_capacity(24);
        out.u32(self.stamp);
        out.ascii_string(&self.machine_name);
        out.u32(self.uid);
        out.u32(self.gid);
        out.u32_array(&self.gids);
        out.into_bytes()
    }

    /// Decode a body carried by an [`Auth`] whose flavour is
    /// [`AuthFlavor::UNIX`].
    pub fn parse(body: &[u8]) -> Result<Self> {
        let mut input = xdr::Reader::new(body);
        Ok(Self {
            stamp: input.u32()?,
            machine_name: input.ascii_string(255)?,
            uid: input.u32()?,
            gid: input.u32()?,
            // RFC 1057 §9.2 declares `unsigned int gids<10>`; RFC 5531 raised
            // it to 16, which is what every implementation actually allows.
            // A player sends none at all.
            gids: input.u32_array(16)?,
        })
    }
}

/// One RPC call.
///
/// Borrows its arguments from the datagram it was parsed out of, or from
/// whatever buffer the caller encoded them into, so parsing a call allocates
/// nothing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Call<'a> {
    /// The correlation token to echo back.
    pub xid: Xid,
    /// Which program is being called.
    pub program: Program,
    /// Which version of it.
    pub version: u32,
    /// Which procedure. Procedure numbers **collide across programs** —
    /// MOUNT's `MNT` and NFS's `GETATTR` are both `1` — so this is only
    /// meaningful together with [`Call::program`].
    pub procedure: u32,
    /// `AUTH_UNIX` on every call observed, in both directions.
    pub credential: Auth<'a>,
    /// `AUTH_NULL` with an empty body on every call observed.
    pub verifier: Auth<'a>,
    /// The procedure's argument block, undecoded. Hand it to the matching
    /// program module.
    pub arguments: &'a [u8],
}

impl<'a> Call<'a> {
    /// A call with the header a player sends: RPC v2 and an `AUTH_NULL`
    /// verifier.
    pub fn new(
        xid: Xid,
        program: Program,
        version: u32,
        procedure: u32,
        credential: Auth<'a>,
        arguments: &'a [u8],
    ) -> Self {
        Self {
            xid,
            program,
            version,
            procedure,
            credential,
            verifier: Auth::NULL,
            arguments,
        }
    }

    /// Encode this call as one datagram.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = xdr::Writer::with_capacity(64 + self.arguments.len());
        out.u32(self.xid.0);
        out.u32(MSG_CALL);
        out.u32(VERSION);
        out.u32(self.program.0);
        out.u32(self.version);
        out.u32(self.procedure);
        self.credential.write(&mut out);
        self.verifier.write(&mut out);
        out.raw(self.arguments);
        out.into_bytes()
    }

    /// Decode one datagram as a call.
    ///
    /// Fails when `msg_type` is not `CALL` or the RPC version is not 2, rather
    /// than decoding either into something plausible. On our ports that
    /// traffic belongs to somebody else and dropping it is the correct answer.
    /// An unrecognised *program* or *procedure* is not an error here: a server
    /// that can read the header can answer `PROG_UNAVAIL` or `PROC_UNAVAIL`,
    /// which is a real answer and what a player expects when it probes.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut input = xdr::Reader::new(data);
        let xid = Xid(input.u32()?);
        let msg_type = input.u32()?;
        if msg_type != MSG_CALL {
            return Err(Error::malformed(
                4,
                format!("expected an RPC call (msg_type 0), got msg_type {msg_type}"),
            ));
        }
        let rpc_version = input.u32()?;
        if rpc_version != VERSION {
            return Err(Error::malformed(
                8,
                format!("unsupported RPC version {rpc_version}, expected {VERSION}"),
            ));
        }
        let program = Program(input.u32()?);
        let version = input.u32()?;
        let procedure = input.u32()?;
        let credential = Auth::read(&mut input)?;
        let verifier = Auth::read(&mut input)?;
        Ok(Self {
            xid,
            program,
            version,
            procedure,
            credential,
            verifier,
            arguments: input.rest(),
        })
    }
}

/// A non-success `accept_stat` (RFC 1057 §9).
///
/// A newtype because a server may invent one; the five below are everything
/// the standard defines past `SUCCESS`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AcceptStat(pub u32);

impl AcceptStat {
    /// The procedure ran. Modelled as [`Accepted::Success`], not as this.
    pub const SUCCESS: Self = Self(0);
    /// This host does not run that program.
    pub const PROG_UNAVAIL: Self = Self(1);
    /// It runs the program but not that version. Modelled as
    /// [`Accepted::ProgMismatch`], which carries the range.
    pub const PROG_MISMATCH: Self = Self(2);
    /// The program does not implement that procedure. What a real CDJ answers
    /// a `READDIR` with, so it is a normal thing for a client to handle rather
    /// than a broken server.
    pub const PROC_UNAVAIL: Self = Self(3);
    /// The arguments did not decode.
    pub const GARBAGE_ARGS: Self = Self(4);
    /// The server failed for a reason of its own.
    pub const SYSTEM_ERR: Self = Self(5);

    /// A name for logs, or `None` for a status we have never observed.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::SUCCESS => "SUCCESS",
            Self::PROG_UNAVAIL => "PROG_UNAVAIL",
            Self::PROG_MISMATCH => "PROG_MISMATCH",
            Self::PROC_UNAVAIL => "PROC_UNAVAIL",
            Self::GARBAGE_ARGS => "GARBAGE_ARGS",
            Self::SYSTEM_ERR => "SYSTEM_ERR",
            _ => return None,
        })
    }
}

impl fmt::Debug for AcceptStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "AcceptStat({})", self.0),
        }
    }
}

/// Why a server refused to run a procedure at all (RFC 1057 §9).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RejectStat(pub u32);

impl RejectStat {
    /// The RPC version is not one this server speaks.
    pub const RPC_MISMATCH: Self = Self(0);
    /// The credential was refused.
    pub const AUTH_ERROR: Self = Self(1);
}

impl fmt::Debug for RejectStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::RPC_MISMATCH => f.write_str("RPC_MISMATCH"),
            Self::AUTH_ERROR => f.write_str("AUTH_ERROR"),
            Self(raw) => write!(f, "RejectStat({raw})"),
        }
    }
}

/// Which way a credential was refused (RFC 1057 §9).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthStat(pub u32);

impl AuthStat {
    /// Good credential, but the server does not know the caller.
    pub const BADCRED: Self = Self(1);
    /// The client must begin a new session.
    pub const REJECTEDCRED: Self = Self(2);
    /// The verifier did not decode.
    pub const BADVERF: Self = Self(3);
    /// The verifier expired or was replayed.
    pub const REJECTEDVERF: Self = Self(4);
    /// The server refuses this security flavour.
    pub const TOOWEAK: Self = Self(5);
}

impl fmt::Debug for AuthStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BADCRED => f.write_str("AUTH_BADCRED"),
            Self::REJECTEDCRED => f.write_str("AUTH_REJECTEDCRED"),
            Self::BADVERF => f.write_str("AUTH_BADVERF"),
            Self::REJECTEDVERF => f.write_str("AUTH_REJECTEDVERF"),
            Self::TOOWEAK => f.write_str("AUTH_TOOWEAK"),
            Self(raw) => write!(f, "AuthStat({raw})"),
        }
    }
}

/// What became of an accepted call.
///
/// The three shapes the wire union actually has: `SUCCESS` is followed by the
/// procedure's results, `PROG_MISMATCH` by a version range, and every other
/// status by nothing at all. Modelling it this way is what stops a caller
/// reading results out of a failure or a version range out of a success.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Accepted<'a> {
    /// The procedure ran; these are its results, undecoded.
    Success(&'a [u8]),
    /// The server runs this program but not this version, and supports
    /// `low` through `high`. Worth distinguishing from
    /// [`AcceptStat::PROG_UNAVAIL`]: one is our bug, the other is a device
    /// that cannot do what we asked.
    ProgMismatch {
        /// Lowest version supported.
        low: u32,
        /// Highest version supported.
        high: u32,
    },
    /// Any other `accept_stat` — all of which RFC 1057 gives no body.
    Failed(FailureStat),
}

/// An `accept_stat` whose wire form is the status word and nothing else.
///
/// That is every value except the two that carry something: `SUCCESS`, which
/// is followed by the procedure's results, and `PROG_MISMATCH`, which is
/// followed by a version range. Both have their own [`Accepted`] variant.
///
/// The inner value is private and [`FailureStat::new`] refuses those two,
/// because the alternative was a public constructor that could build a reply
/// our own decoder rejects: `PROG_MISMATCH` with no version range is a
/// twenty-four-byte datagram that every conformant client reads as truncated.
/// This is the "make illegal states unrepresentable" case in miniature — the
/// check happens once, here, and no encoder below can forget it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FailureStat(AcceptStat);

impl FailureStat {
    /// This host does not run that program.
    pub const PROG_UNAVAIL: Self = Self(AcceptStat::PROG_UNAVAIL);
    /// The program does not implement that procedure. What a real CDJ answers
    /// a `READDIR` with, and what we answer a write with.
    pub const PROC_UNAVAIL: Self = Self(AcceptStat::PROC_UNAVAIL);
    /// The arguments did not decode.
    pub const GARBAGE_ARGS: Self = Self(AcceptStat::GARBAGE_ARGS);
    /// The server failed for a reason of its own.
    pub const SYSTEM_ERR: Self = Self(AcceptStat::SYSTEM_ERR);

    /// `None` for `SUCCESS` and `PROG_MISMATCH`, which carry a body and so
    /// cannot be represented as a bare status.
    pub fn new(stat: AcceptStat) -> Option<Self> {
        match stat {
            AcceptStat::SUCCESS | AcceptStat::PROG_MISMATCH => None,
            other => Some(Self(other)),
        }
    }

    /// The `accept_stat` word this puts on the wire.
    pub fn stat(self) -> AcceptStat {
        self.0
    }
}

impl fmt::Debug for FailureStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl Accepted<'_> {
    /// The `accept_stat` word this variant puts on the wire.
    pub fn stat(&self) -> AcceptStat {
        match self {
            Self::Success(_) => AcceptStat::SUCCESS,
            Self::ProgMismatch { .. } => AcceptStat::PROG_MISMATCH,
            Self::Failed(stat) => stat.stat(),
        }
    }
}

/// Why a call was denied outright (RFC 1057 §9).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Denied {
    /// The server does not speak this version of RPC itself.
    RpcMismatch {
        /// Lowest RPC version supported.
        low: u32,
        /// Highest RPC version supported.
        high: u32,
    },
    /// The credential was refused.
    AuthError(AuthStat),
    /// A `reject_stat` outside the two the standard defines, and so with no
    /// body.
    Other(OtherReject),
}

/// A `reject_stat` outside the two RFC 1057 defines.
///
/// Both defined values carry a body — `RPC_MISMATCH` a version range and
/// `AUTH_ERROR` an [`AuthStat`] — and both have their own [`Denied`] variant,
/// so as with [`FailureStat`] the inner value is private and cannot be one of
/// them. Nothing has ever sent us one of these: there is not a single
/// `MSG_DENIED` reply in the corpus.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct OtherReject(RejectStat);

impl OtherReject {
    /// `None` for the two values that carry a body.
    pub fn new(stat: RejectStat) -> Option<Self> {
        match stat {
            RejectStat::RPC_MISMATCH | RejectStat::AUTH_ERROR => None,
            other => Some(Self(other)),
        }
    }

    /// The `reject_stat` word this puts on the wire.
    pub fn stat(self) -> RejectStat {
        self.0
    }
}

impl fmt::Debug for OtherReject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// One RPC reply.
///
/// A denied reply carries no verifier, which is why the two states are
/// separate variants rather than a struct with optional fields.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reply<'a> {
    /// `MSG_ACCEPTED`: the server recognised the call, whatever came of it.
    Accepted {
        /// Echoed from the call.
        xid: Xid,
        /// `AUTH_NULL` with an empty body in everything observed, in both
        /// directions. A client has nothing to check it against.
        verifier: Auth<'a>,
        /// What became of the call.
        status: Accepted<'a>,
    },
    /// `MSG_DENIED`: the server would not run the call at all.
    Denied {
        /// Echoed from the call.
        xid: Xid,
        /// Why.
        reason: Denied,
    },
}

impl<'a> Reply<'a> {
    /// A successful reply carrying `results`, with the verifier a player
    /// sends.
    pub fn success(xid: Xid, results: &'a [u8]) -> Self {
        Self::Accepted {
            xid,
            verifier: Auth::NULL,
            status: Accepted::Success(results),
        }
    }

    /// An accepted reply reporting a non-success status — `PROC_UNAVAIL` for a
    /// procedure we do not implement, `PROG_UNAVAIL` for a program that is not
    /// this port's, `GARBAGE_ARGS` for arguments that did not decode.
    ///
    /// Takes a [`FailureStat`] rather than an [`AcceptStat`] so that
    /// `PROG_MISMATCH`, which needs a version range after it, cannot be sent
    /// from here without one.
    pub fn failed(xid: Xid, stat: FailureStat) -> Self {
        Self::Accepted {
            xid,
            verifier: Auth::NULL,
            status: Accepted::Failed(stat),
        }
    }

    /// An accepted reply reporting that this program is served at a different
    /// version, with the range that is served.
    pub fn prog_mismatch(xid: Xid, low: u32, high: u32) -> Self {
        Self::Accepted {
            xid,
            verifier: Auth::NULL,
            status: Accepted::ProgMismatch { low, high },
        }
    }

    /// The correlation token, whichever state this reply is in.
    pub fn xid(&self) -> Xid {
        match *self {
            Self::Accepted { xid, .. } | Self::Denied { xid, .. } => xid,
        }
    }

    /// The procedure's results, or `None` for any reply that is not a success.
    ///
    /// An empty result block and a failure are different things, so this
    /// returns `Some(&[])` for a procedure like `NULL` that succeeds with no
    /// output.
    pub fn results(&self) -> Option<&'a [u8]> {
        match self {
            Self::Accepted {
                status: Accepted::Success(results),
                ..
            } => Some(results),
            _ => None,
        }
    }

    /// Encode this reply as one datagram.
    ///
    /// Every reply this type can express is well formed — the two unions that
    /// carry a body have their own variants, so the body cannot go missing.
    pub fn encode(&self) -> Vec<u8> {
        // Sized for the header plus whatever results ride along, because the
        // biggest of those is an 8 KiB READ payload on the path a stall turns
        // into an audio dropout (F18).
        let mut out = xdr::Writer::with_capacity(32 + self.results().map_or(0, <[u8]>::len));
        match self {
            Self::Accepted {
                xid,
                verifier,
                status,
            } => {
                out.u32(xid.0);
                out.u32(MSG_REPLY);
                out.u32(MSG_ACCEPTED);
                verifier.write(&mut out);
                out.u32(status.stat().0);
                match status {
                    Accepted::Success(results) => out.raw(results),
                    Accepted::ProgMismatch { low, high } => {
                        out.u32(*low);
                        out.u32(*high);
                    }
                    Accepted::Failed(_) => {}
                }
            }
            Self::Denied { xid, reason } => {
                out.u32(xid.0);
                out.u32(MSG_REPLY);
                out.u32(MSG_DENIED);
                match reason {
                    Denied::RpcMismatch { low, high } => {
                        out.u32(RejectStat::RPC_MISMATCH.0);
                        out.u32(*low);
                        out.u32(*high);
                    }
                    Denied::AuthError(stat) => {
                        out.u32(RejectStat::AUTH_ERROR.0);
                        out.u32(stat.0);
                    }
                    Denied::Other(stat) => out.u32(stat.stat().0),
                }
            }
        }
        out.into_bytes()
    }

    /// Decode one datagram as a reply.
    ///
    /// Fails when `msg_type` is not `REPLY`, which is how a call on the
    /// client's socket is told apart from an answer to it. A well-formed reply
    /// reporting an error is *not* a failure here — it decodes into
    /// [`Accepted::Failed`] or [`Reply::Denied`], because "the server said no"
    /// and "that was not a reply" are different findings.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut input = xdr::Reader::new(data);
        let xid = Xid(input.u32()?);
        let msg_type = input.u32()?;
        if msg_type != MSG_REPLY {
            return Err(Error::malformed(
                4,
                format!("expected an RPC reply (msg_type 1), got msg_type {msg_type}"),
            ));
        }
        let reply_stat = input.u32()?;
        match reply_stat {
            MSG_DENIED => {
                let reject = RejectStat(input.u32()?);
                let reason = match reject {
                    RejectStat::RPC_MISMATCH => Denied::RpcMismatch {
                        low: input.u32()?,
                        high: input.u32()?,
                    },
                    RejectStat::AUTH_ERROR => Denied::AuthError(AuthStat(input.u32()?)),
                    // The two arms above are the only ones RejectStat defines,
                    // so `new` cannot refuse what is left.
                    other => Denied::Other(OtherReject(other)),
                };
                Ok(Self::Denied { xid, reason })
            }
            MSG_ACCEPTED => {
                let verifier = Auth::read(&mut input)?;
                let stat = AcceptStat(input.u32()?);
                let status = match stat {
                    AcceptStat::SUCCESS => Accepted::Success(input.rest()),
                    AcceptStat::PROG_MISMATCH => Accepted::ProgMismatch {
                        low: input.u32()?,
                        high: input.u32()?,
                    },
                    // Likewise: the two arms above are exactly the values
                    // `FailureStat::new` rejects.
                    other => Accepted::Failed(FailureStat(other)),
                };
                Ok(Self::Accepted {
                    xid,
                    verifier,
                    status,
                })
            }
            other => Err(Error::malformed(
                8,
                format!("invalid reply_stat {other}, expected 0 or 1"),
            )),
        }
    }
}

/// Either direction of one datagram, for a dissector that does not know which
/// it is holding.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Message<'a> {
    /// A call.
    Call(Call<'a>),
    /// A reply.
    Reply(Reply<'a>),
}

impl<'a> Message<'a> {
    /// Decode one datagram, dispatching on `msg_type`.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        // Read the discriminator through a reader over the whole datagram, so
        // that a runt reports the offset it actually ran out at. A reader over
        // `data[4..]` would report offsets relative to that slice, and for a
        // datagram shorter than four bytes would describe a slice that does
        // not exist.
        let mut header = xdr::Reader::new(data);
        let _xid = header.u32()?;
        let msg_type = header.u32()?;
        match msg_type {
            MSG_CALL => Call::parse(data).map(Message::Call),
            MSG_REPLY => Reply::parse(data).map(Message::Reply),
            other => Err(Error::malformed(
                4,
                format!("msg_type {other} is neither a call nor a reply"),
            )),
        }
    }

    /// The correlation token, whichever direction this is.
    pub fn xid(&self) -> Xid {
        match self {
            Self::Call(call) => call.xid,
            Self::Reply(reply) => reply.xid(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A portmap `GETPORT` asking for mountd, built the way a player builds
    /// one. Header offsets checked field by field against RFC 1057 §8.
    #[test]
    fn a_call_header_is_laid_out_as_the_standard_says() {
        let credential = AuthUnix::cdj(STAMP_FIRST_CALL).encode();
        let args =
            portmap::Request::GetPort(portmap::Mapping::query(Program::MOUNT, 1, IpProtocol::UDP))
                .encode_arguments();
        let call = Call::new(
            Xid(0x1122_3344),
            Program::PORTMAP,
            portmap::VERSION,
            portmap::Proc::GETPORT.0,
            Auth::unix(&credential),
            &args,
        );
        let raw = call.encode();
        assert_eq!(raw.get(0..4), Some([0x11, 0x22, 0x33, 0x44].as_slice()));
        assert_eq!(
            raw.get(4..8),
            Some([0, 0, 0, 0].as_slice()),
            "msg_type CALL"
        );
        assert_eq!(raw.get(8..12), Some([0, 0, 0, 2].as_slice()), "rpcvers 2");
        assert_eq!(raw.get(12..16), Some(100_000u32.to_be_bytes().as_slice()));
        assert_eq!(raw.get(16..20), Some(2u32.to_be_bytes().as_slice()));
        assert_eq!(raw.get(20..24), Some(3u32.to_be_bytes().as_slice()));
        // Credential: AUTH_UNIX, a 20-byte body (stamp, empty name, uid, gid,
        // zero gids), then the AUTH_NULL verifier with an empty body.
        assert_eq!(raw.get(24..28), Some(1u32.to_be_bytes().as_slice()));
        assert_eq!(raw.get(28..32), Some(20u32.to_be_bytes().as_slice()));
        assert_eq!(raw.get(52..56), Some([0, 0, 0, 0].as_slice()), "verf NULL");
        assert_eq!(raw.get(56..60), Some([0, 0, 0, 0].as_slice()), "empty body");
        assert_eq!(raw.len(), 60 + 16, "header plus four GETPORT words");
    }

    #[test]
    fn a_call_round_trips_with_auth_unix() {
        let credential = AuthUnix::cdj(STAMP_FIRST_CALL).encode();
        let call = Call::new(
            Xid(1),
            Program::NFS,
            2,
            nfs2::Proc::READ.0,
            Auth::unix(&credential),
            &[1, 2, 3, 4],
        );
        let raw = call.encode();
        let parsed = Call::parse(&raw).unwrap();
        assert_eq!(parsed, call);
        assert_eq!(parsed.credential.flavor, AuthFlavor::UNIX);
        assert_eq!(parsed.verifier.flavor, AuthFlavor::NULL);
        assert!(parsed.verifier.body.is_empty());
        assert_eq!(parsed.arguments, &[1, 2, 3, 4]);
    }

    /// The twenty bytes a real player's `AUTH_UNIX` body occupies, in 56,966
    /// of 56,966 observed calls. Only the first word ever moves.
    #[test]
    fn the_auth_unix_body_a_player_sends_is_twenty_bytes() {
        let body = AuthUnix::cdj(STAMP_FIRST_CALL).encode();
        assert_eq!(
            body,
            [
                0x96, 0x7b, 0x87, 0x03, // stamp, the first call since boot
                0x00, 0x00, 0x00, 0x00, // machine_name "", zero-length
                0x00, 0x00, 0x00, 0x00, // uid 0
                0x00, 0x00, 0x00, 0x00, // gid 0
                0x00, 0x00, 0x00, 0x00, // no supplementary gids
            ]
        );
        let parsed = AuthUnix::parse(&body).unwrap();
        assert_eq!(parsed.stamp, STAMP_FIRST_CALL);
        assert_eq!(parsed.machine_name, "");
        assert_eq!((parsed.uid, parsed.gid), (0, 0));
        assert!(parsed.gids.is_empty());
    }

    #[test]
    fn only_the_stamp_varies_between_two_calls() {
        // A decoder that treated the stamp as a constant would have matched
        // every other field and still been wrong.
        let first = AuthUnix::cdj(STAMP_SEQUENCE[0]).encode();
        let second = AuthUnix::cdj(STAMP_SEQUENCE[1]).encode();
        assert_ne!(first.get(..4), second.get(..4));
        assert_eq!(first.get(4..), second.get(4..));
    }

    /// The correction to C8. These stamps are not arbitrary: each was seen in
    /// two to four independent captures and on two or three physically
    /// distinct devices, and 9947 xids recurring across separate captures
    /// never once disagreed with themselves.
    #[test]
    fn the_stamp_is_a_fixed_sequence_indexed_by_the_call_number_since_boot() {
        assert_eq!(stamp_for_xid(Xid(1)), Some(STAMP_FIRST_CALL));
        assert_eq!(stamp_for_xid(Xid(1)), Some(0x967b_8703));
        assert_eq!(stamp_for_xid(Xid(2)), Some(0x9922_e112));
        assert_eq!(stamp_for_xid(Xid(9)), Some(0xf0af_010a));
        assert_eq!(stamp_for_xid(Xid(40)), Some(0xad14_5926));
    }

    #[test]
    fn a_call_index_outside_the_observed_table_has_no_stamp_to_reproduce() {
        // Not an error: nothing validates the stamp, so past the table any
        // value will do and saying so is more honest than extrapolating.
        assert_eq!(stamp_for_xid(Xid(41)), None);
        assert_eq!(
            stamp_for_xid(Xid(0)),
            None,
            "a deck's counter starts at one, so there is no zeroth call"
        );
        assert_eq!(stamp_for_xid(Xid(u32::MAX)), None);
    }

    #[test]
    fn an_accepted_reply_round_trips() {
        let raw = Reply::success(Xid(0xabcd), &[0xde, 0xad, 0xbe, 0xef]).encode();
        let reply = Reply::parse(&raw).unwrap();
        assert_eq!(reply.xid(), Xid(0xabcd));
        assert_eq!(reply.results(), Some([0xde, 0xad, 0xbe, 0xef].as_slice()));
        assert_eq!(
            raw.get(0..12),
            Some([0, 0, 0xab, 0xcd, 0, 0, 0, 1, 0, 0, 0, 0].as_slice()),
            "xid, msg_type REPLY, reply_stat MSG_ACCEPTED"
        );
        assert_eq!(
            raw.get(12..20),
            Some([0, 0, 0, 0, 0, 0, 0, 0].as_slice()),
            "AUTH_NULL verifier with an empty body"
        );
    }

    #[test]
    fn a_null_procedure_succeeds_with_no_results_which_is_not_a_failure() {
        let raw = Reply::success(Xid(7), &[]).encode();
        let reply = Reply::parse(&raw).unwrap();
        assert_eq!(
            reply.results(),
            Some([].as_slice()),
            "an empty result block and a failure are different things"
        );
    }

    #[test]
    fn proc_unavail_is_a_real_answer_and_carries_no_body() {
        let raw = Reply::failed(Xid(1), FailureStat::PROC_UNAVAIL).encode();
        assert_eq!(
            raw.len(),
            24,
            "xid, msg_type, reply_stat, the two verifier words, the status — \
             and nothing after it"
        );
        let reply = Reply::parse(&raw).unwrap();
        assert!(matches!(
            reply,
            Reply::Accepted {
                status: Accepted::Failed(FailureStat::PROC_UNAVAIL),
                ..
            }
        ));
        assert_eq!(reply.results(), None);
    }

    /// A reply this type can build must be one our own decoder accepts.
    ///
    /// `PROG_MISMATCH` is followed by a version range and `AUTH_ERROR` by an
    /// `auth_stat`, so putting either in the "bare status" arm would emit a
    /// datagram every conformant client reads as truncated. The constructors
    /// refuse them, which is why that arm needs no runtime check.
    #[test]
    fn a_status_that_carries_a_body_cannot_be_sent_as_a_bare_status() {
        assert_eq!(FailureStat::new(AcceptStat::PROG_MISMATCH), None);
        assert_eq!(
            FailureStat::new(AcceptStat::SUCCESS),
            None,
            "SUCCESS is followed by the results, so it is not a failure either"
        );
        assert_eq!(
            FailureStat::new(AcceptStat::PROC_UNAVAIL),
            Some(FailureStat::PROC_UNAVAIL)
        );
        assert_eq!(
            FailureStat::new(AcceptStat(99)).map(FailureStat::stat),
            Some(AcceptStat(99)),
            "an unfamiliar status has no body either, so it is representable"
        );

        assert_eq!(OtherReject::new(RejectStat::RPC_MISMATCH), None);
        assert_eq!(OtherReject::new(RejectStat::AUTH_ERROR), None);
        assert_eq!(
            OtherReject::new(RejectStat(7)).map(OtherReject::stat),
            Some(RejectStat(7))
        );
    }

    #[test]
    fn prog_mismatch_carries_the_supported_version_range() {
        let raw = Reply::Accepted {
            xid: Xid(1),
            verifier: Auth::NULL,
            status: Accepted::ProgMismatch { low: 2, high: 3 },
        }
        .encode();
        let reply = Reply::parse(&raw).unwrap();
        assert!(matches!(
            reply,
            Reply::Accepted {
                status: Accepted::ProgMismatch { low: 2, high: 3 },
                ..
            }
        ));
    }

    #[test]
    fn a_denied_reply_has_no_verifier() {
        let raw = Reply::Denied {
            xid: Xid(1),
            reason: Denied::AuthError(AuthStat::BADCRED),
        }
        .encode();
        assert_eq!(
            raw.len(),
            20,
            "xid, msg_type, reply_stat, reject_stat, auth_stat — and no \
             verifier, which is why the two states are separate variants"
        );
        assert_eq!(
            Reply::parse(&raw).unwrap(),
            Reply::Denied {
                xid: Xid(1),
                reason: Denied::AuthError(AuthStat::BADCRED),
            }
        );
    }

    #[test]
    fn an_rpc_mismatch_denial_round_trips() {
        let reply = Reply::Denied {
            xid: Xid(9),
            reason: Denied::RpcMismatch { low: 2, high: 2 },
        };
        assert_eq!(Reply::parse(&reply.encode()).unwrap(), reply);
    }

    #[test]
    fn a_call_is_not_mistaken_for_a_reply() {
        let raw = Call::new(Xid(1), Program::PORTMAP, 2, 0, Auth::NULL, &[]).encode();
        let error = Reply::parse(&raw).unwrap_err();
        assert!(!error.is_truncated(), "{error:?}");
        assert!(matches!(error, Error::Malformed { at: 4, .. }));
    }

    #[test]
    fn a_reply_is_not_mistaken_for_a_call() {
        let raw = Reply::success(Xid(1), &[]).encode();
        assert!(matches!(
            Call::parse(&raw),
            Err(Error::Malformed { at: 4, .. })
        ));
    }

    #[test]
    fn an_rpc_v1_or_v3_call_is_refused_rather_than_half_decoded() {
        let mut raw = Call::new(Xid(1), Program::NFS, 2, 0, Auth::NULL, &[]).encode();
        raw.splice(8..12, 3u32.to_be_bytes());
        assert!(matches!(
            Call::parse(&raw),
            Err(Error::Malformed { at: 8, .. })
        ));
    }

    #[test]
    fn an_unknown_program_still_decodes_so_a_server_can_answer_prog_unavail() {
        let raw = Call::new(Xid(1), Program(999_999), 4, 12, Auth::NULL, &[]).encode();
        let call = Call::parse(&raw).unwrap();
        assert_eq!(call.program, Program(999_999));
        assert_eq!(call.procedure, 12);
    }

    #[test]
    fn a_message_dispatches_on_msg_type() {
        let call = Call::new(Xid(1), Program::NFS, 2, 0, Auth::NULL, &[]).encode();
        assert!(matches!(Message::parse(&call), Ok(Message::Call(_))));
        let reply = Reply::success(Xid(1), &[]).encode();
        assert!(matches!(Message::parse(&reply), Ok(Message::Reply(_))));
    }

    #[test]
    fn a_runt_datagram_is_truncation_not_garbage() {
        let error = Call::parse(&[0, 0, 0, 1]).unwrap_err();
        assert!(error.is_truncated(), "{error:?}");
    }

    #[test]
    fn an_oversized_auth_body_is_refused_before_allocating() {
        // RFC 1057 caps an opaque_auth body at 400 bytes.
        let mut raw = Vec::new();
        raw.extend_from_slice(&0u32.to_be_bytes()); // xid
        raw.extend_from_slice(&MSG_CALL.to_be_bytes());
        raw.extend_from_slice(&VERSION.to_be_bytes());
        raw.extend_from_slice(&Program::NFS.0.to_be_bytes());
        raw.extend_from_slice(&2u32.to_be_bytes());
        raw.extend_from_slice(&1u32.to_be_bytes());
        raw.extend_from_slice(&AuthFlavor::UNIX.0.to_be_bytes());
        raw.extend_from_slice(&0xffff_ffffu32.to_be_bytes());
        let error = Call::parse(&raw).unwrap_err();
        assert!(
            matches!(error, Error::ImplausibleLength { limit: 400, .. }),
            "{error:?}"
        );
    }

    #[test]
    fn program_numbers_are_the_three_a_cdj_runs() {
        // F10: rpcinfo against a CDJ-2000NXS with a stick inserted.
        assert_eq!(Program::PORTMAP.0, 100_000);
        assert_eq!(Program::NFS.0, 100_003);
        assert_eq!(Program::MOUNT.0, 100_005);
        assert_eq!(format!("{:?}", Program::MOUNT), "mountd");
        assert_eq!(format!("{:?}", Program(42)), "Program(42)");
    }
}

/// Whole datagrams captured off real hardware, decoded end to end.
///
/// A committed fixture floor. Everything above proves our encoder and our
/// decoder agree with each other, which is not the same as agreeing with a
/// CDJ — the reference implementation had an encoder and a decoder that agreed
/// perfectly on a bug that only showed on non-ASCII input (O6). These are the
/// bytes two CDJ-2000NXS running firmware 1.44 actually sent, plus two from a
/// 2016 capture of different Pioneer hardware, and they live here as literals
/// so the layout is pinned by evidence rather than by whatever happens to be
/// in a capture directory today.
///
/// Where a datagram is marked *deck to deck*, both the call and the reply are
/// genuine Pioneer bytes with no code of ours anywhere in the exchange.
#[cfg(test)]
mod captured {
    use super::*;
    use crate::rpc::xdr::Utf16LeString;

    /// Decode a hex literal, ignoring whitespace so a long datagram can be
    /// wrapped for reading. Panics on a malformed one, which is a broken test
    /// rather than a broken codec.
    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            digits.len().is_multiple_of(2),
            "a hex literal needs an even number of digits, got {}",
            digits.len()
        );
        digits
            .chunks_exact(2)
            .map(|pair| {
                let byte: String = pair.iter().collect();
                u8::from_str_radix(&byte, 16).expect("a hex literal must be hex")
            })
            .collect()
    }

    /// Split a captured call into its RPC header and the program's arguments.
    fn call(raw: &[u8]) -> Call<'_> {
        let parsed = Call::parse(raw).expect("a captured call must decode");
        // True of all 56,966 calls in the corpus, in both directions.
        assert_eq!(parsed.credential.flavor, AuthFlavor::UNIX);
        assert_eq!(parsed.credential.body.len(), 20);
        assert_eq!(parsed.verifier, Auth::NULL);
        parsed
    }

    /// The results of a captured reply, proving it was accepted first.
    fn results(raw: &[u8]) -> &[u8] {
        let parsed = Reply::parse(raw).expect("a captured reply must decode");
        match parsed {
            Reply::Accepted { verifier, .. } => assert_eq!(verifier, Auth::NULL),
            Reply::Denied { .. } => panic!("no MSG_DENIED reply exists in the corpus"),
        }
        parsed.results().expect("every captured reply is a SUCCESS")
    }

    // -- portmap ----------------------------------------------------------

    /// Deck to deck. `S13-format-ground-truth`, frames 91–92: deck B asks
    /// deck A's portmapper for mountd and is told 48276 — the number F6
    /// records three different devices agreeing on.
    #[test]
    fn a_real_getport_for_mountd_is_answered_with_48276() {
        let raw = hex(
            "000000010000000000000002000186a00000000200000003000000010000001496\
             7b8703000000000000000000000000000000000000000000000000000186a50000\
             00010000001100000000",
        );
        let parsed = call(&raw);
        assert_eq!(
            raw.len(),
            76,
            "every GETPORT call in the corpus is 76 bytes"
        );
        assert_eq!(parsed.xid, Xid(1), "a deck's first call after power-on");
        assert_eq!(parsed.program, Program::PORTMAP);
        assert_eq!((parsed.version, parsed.procedure), (2, 3));
        // The stamp is entry one of the boot sequence, not a nonce.
        let credential = AuthUnix::parse(parsed.credential.body).unwrap();
        assert_eq!(Some(credential.stamp), stamp_for_xid(parsed.xid));
        assert_eq!(credential.stamp, 0x967b_8703);
        assert_eq!(credential.machine_name, "");
        assert_eq!((credential.uid, credential.gid), (0, 0));
        assert!(credential.gids.is_empty());

        let request =
            portmap::Request::parse(portmap::Proc(parsed.procedure), parsed.arguments).unwrap();
        assert_eq!(
            request,
            portmap::Request::GetPort(portmap::Mapping::query(Program::MOUNT, 1, IpProtocol::UDP))
        );
        assert_eq!(parsed.encode(), raw, "and it re-encodes byte for byte");

        let reply = hex("0000000100000001000000000000000000000000000000000000bc94");
        assert_eq!(reply.len(), 28, "every GETPORT reply is 28 bytes");
        assert_eq!(
            portmap::Response::parse(portmap::Proc::GETPORT, results(&reply)).unwrap(),
            portmap::Response::GetPort(Some(mount::PIONEER_PORT))
        );
        assert_eq!(Reply::parse(&reply).unwrap().encode(), reply);
    }

    /// Deck to deck, the very next exchange: mountd first, then nfsd. A deck
    /// asks for nothing else, ever.
    #[test]
    fn a_real_getport_for_nfsd_is_answered_with_2049() {
        let raw = hex(
            "000000020000000000000002000186a00000000200000003000000010000001499\
             22e112000000000000000000000000000000000000000000000000000186a30000\
             00020000001100000000",
        );
        let parsed = call(&raw);
        assert_eq!(parsed.xid, Xid(2));
        assert_eq!(
            AuthUnix::parse(parsed.credential.body).unwrap().stamp,
            0x9922_e112,
            "entry two of the sequence, on a deck's second call"
        );
        assert_eq!(
            portmap::Request::parse(portmap::Proc(parsed.procedure), parsed.arguments).unwrap(),
            portmap::Request::GetPort(portmap::Mapping::query(Program::NFS, 2, IpProtocol::UDP))
        );
        let reply = hex("00000002000000010000000000000000000000000000000000000801");
        assert_eq!(
            portmap::Response::parse(portmap::Proc::GETPORT, results(&reply)).unwrap(),
            portmap::Response::GetPort(Some(nfs2::PORT))
        );
    }

    /// A CDJ-2000NXS's own registration table, from `S04-media-insert`. This
    /// is the reply behind F10's `rpcinfo` output, and the order is the
    /// deck's own.
    #[test]
    fn a_real_portmap_dump_lists_exactly_nfs_mountd_and_portmapper() {
        let reply = hex(
            "10000001000000010000000000000000000000000000000000000001000186a300\
             000002000000110000080100000001000186a5000000010000001100 00bc940000\
             0001000186a00000000200000011000000 6f00000000",
        );
        let decoded = portmap::Response::parse(portmap::Proc::DUMP, results(&reply)).unwrap();
        assert_eq!(
            decoded,
            portmap::Response::Dump(
                portmap::cdj_registrations(portmap::PORT, mount::PIONEER_PORT, nfs2::PORT).to_vec()
            ),
            "the table `cdj_registrations` publishes is the one a deck publishes, \
             in the same order"
        );
        assert_eq!(
            decoded.encode(),
            results(&reply),
            "and our encoder reproduces it byte for byte"
        );
    }

    /// The smallest accepted reply there is: 24 bytes and no results. A
    /// CDJ-2000NXS answering a portmap `NULL`.
    #[test]
    fn a_real_null_reply_is_twenty_four_bytes_of_header() {
        let reply = hex("100000000000000100000000000000000000000000000000");
        assert_eq!(reply.len(), 24);
        assert_eq!(
            results(&reply),
            &[] as &[u8],
            "void results, which is not the same as a failure"
        );
        assert_eq!(Reply::parse(&reply).unwrap().encode(), reply);
    }

    // -- MOUNT ------------------------------------------------------------

    /// Deck to deck, `S13` frames 95–96. The one MOUNT call a real deck
    /// makes (F37), and the six bytes F12 quotes as `raw=2f0043002f00`.
    #[test]
    fn a_real_mnt_of_the_usb_export_carries_a_utf16le_path() {
        let raw = hex(
            "000000030000000000000002000186a5000000010000000100000001000000 14a4\
             921306000000000000000000000000000000000000000000000000 000000062f00\
             43002f000011",
        );
        let parsed = call(&raw);
        assert_eq!(raw.len(), 72);
        assert_eq!(parsed.program, Program::MOUNT);
        assert_eq!((parsed.version, parsed.procedure), (1, 1));
        let request =
            mount::Request::parse(mount::Proc(parsed.procedure), parsed.arguments).unwrap();
        assert_eq!(request, mount::Request::Mnt(Utf16LeString::new("/C/")));
        assert_eq!(
            request.path().map(Utf16LeString::len_bytes),
            Some(6),
            "three characters, six bytes — the prefix counts bytes"
        );
        assert_eq!(mount::slot_for_export("/C/"), Some(crate::Slot::USB));

        // The deck left `0011` in the XDR padding rather than the zeroes RFC
        // 4506 asks for. A parsed `Call` carries its argument block opaquely,
        // so the whole datagram still re-encodes byte for byte...
        assert_eq!(parsed.encode(), raw);
        assert_eq!(
            parsed.arguments.get(10..),
            Some([0x00, 0x11].as_slice()),
            "including the padding, whatever it holds"
        );
        // ...but re-encoding from the *decoded* request writes the standard
        // zeroes, so a round trip through `mount::Request` differs there and
        // must be compared field by field rather than byte by byte.
        let ours = request.encode_arguments();
        assert_eq!(ours.len(), parsed.arguments.len());
        assert_eq!(ours.get(..10), parsed.arguments.get(..10));
        assert_eq!(ours.get(10..), Some([0x00, 0x00].as_slice()));

        // The reply: a deck's own handle is three 32-bit words and 20 zeroes.
        let reply = hex(
            "000000030000000100000000000000000000000000000000000000000125 38a801\
             2538a8012538a80000000000000000000000000000000000000000",
        );
        assert_eq!(reply.len(), 60);
        let handle = match mount::Response::parse(mount::Proc::MNT, results(&reply)).unwrap() {
            mount::Response::Mnt(Ok(handle)) => handle,
            other => panic!("expected a root filehandle, got {other:?}"),
        };
        assert_eq!(
            handle.as_bytes().get(12..),
            Some([0u8; 20].as_slice()),
            "a deck fills only the twelve bytes it considers a handle"
        );
        assert_eq!(handle.fileid(), 0x0125_38a8);
        assert_eq!(
            mount::Response::Mnt(Ok(handle)).encode(),
            results(&reply),
            "and our encoder reproduces the reply body byte for byte"
        );
    }

    /// Deck to deck, `S15a-sd-alone`. SD is `/B/` (F37), and the deck's mount
    /// root differs from the USB one only in its leading byte.
    #[test]
    fn a_real_mnt_of_the_sd_export_differs_by_one_character() {
        let raw = hex(
            "000017fc0000000000000002000186a5000000010000000100000001000000149c\
             0fcd05000000000000000000000000000000000000000000000000000000062f00\
             42002f000011",
        );
        let parsed = call(&raw);
        let request =
            mount::Request::parse(mount::Proc(parsed.procedure), parsed.arguments).unwrap();
        assert_eq!(request, mount::Request::Mnt(Utf16LeString::new("/B/")));
        assert_eq!(mount::slot_for_export("/B/"), Some(crate::Slot::SD));

        let reply = hex(
            "000017fc00000001000000000000000000000000000000000000000002253 8a802\
             2538a8022538a80000000000000000000000000000000000000000",
        );
        let handle = match mount::Response::parse(mount::Proc::MNT, results(&reply)).unwrap() {
            mount::Response::Mnt(Ok(handle)) => handle,
            other => panic!("expected a root filehandle, got {other:?}"),
        };
        assert_eq!(
            handle.fileid(),
            0x0225_38a8,
            "02… for the SD mount root against 01… for the USB one, same deck"
        );
    }

    /// C6, from the 2016 capture: the same slot, a different spelling, and no
    /// trailing slash. This is why [`mount::slot_for_export`] matches on the
    /// prefix instead of the whole string.
    #[test]
    fn a_real_mnt_of_c_slash_export_proves_the_spelling_varies() {
        let raw = hex(
            "000000050000000000000002000186a5000000010000000100000001000000142a\
             994a03000000000000000000000000000000000000000000000000000000122f00\
             43002f004500580050004f00520054000004",
        );
        let parsed = call(&raw);
        let request =
            mount::Request::parse(mount::Proc(parsed.procedure), parsed.arguments).unwrap();
        let path = request.path().unwrap();
        assert_eq!(path.to_string_lossy(), "/C/EXPORT");
        assert_eq!(path.len_bytes(), 18, "nine characters, eighteen bytes");
        assert_eq!(
            mount::slot_for_export("/C/EXPORT"),
            Some(crate::Slot::USB),
            "a matcher keyed on the whole string would miss this"
        );
    }

    /// C9 and F37: real players do call `UMNT`, once per slot, after an
    /// eject. Deck to deck, `S13` frames 53203–53204.
    #[test]
    fn a_real_umnt_follows_the_eject_and_is_answered_with_nothing() {
        let raw = hex(
            "000017f90000000000000002000186a5000000010000000300000001000000147a\
             c50e0c000000000000000000000000000000000000000000000000000000062f00\
             43002f003cd2",
        );
        let parsed = call(&raw);
        assert_eq!(parsed.procedure, 3);
        assert_eq!(
            mount::Request::parse(mount::Proc(parsed.procedure), parsed.arguments).unwrap(),
            mount::Request::Umnt(Utf16LeString::new("/C/"))
        );
        assert_eq!(
            raw.get(70..),
            Some([0x3c, 0xd2].as_slice()),
            "more uninitialised padding, and a different value from the MNT"
        );

        let reply = hex("000017f90000000100000000000000000000000000000000");
        assert_eq!(reply.len(), 24);
        assert_eq!(results(&reply), &[] as &[u8]);
        assert_eq!(
            mount::Response::parse(mount::Proc::UMNT, results(&reply)).unwrap(),
            mount::Response::Umnt
        );
    }

    /// C7, on target hardware: within one reply the path is UTF-16LE and the
    /// group is ASCII. The group is the whole link-local subnet, which is the
    /// mechanism behind passive access (F11, F12). Only the populated slot is
    /// listed — this deck had a stick and no SD card, and there is no `/B/`.
    #[test]
    fn a_real_export_reply_mixes_utf16le_and_ascii_in_one_structure() {
        let reply = hex(
            "10000001000000010000000000000000000000000000000000000001000000062f\
             0043002f00000000000001000000173136392e3235342e302e302f3235352e3235\
             352e302e30000000000000000000",
        );
        let decoded = mount::Response::parse(mount::Proc::EXPORT, results(&reply)).unwrap();
        assert_eq!(
            decoded,
            mount::Response::Export(vec![mount::Export::new(
                "/C/",
                &[mount::Export::LINK_LOCAL_SUBNET]
            )])
        );
        let mount::Response::Export(exports) = &decoded else {
            panic!("expected an export listing");
        };
        let export = exports.first().unwrap();
        assert_eq!(export.path.as_bytes(), hex("2f0043002f00"), "F12's raw=…");
        assert_eq!(export.path.len_bytes(), 6, "six bytes for three characters");
        assert_eq!(
            export.groups.first().map(String::len),
            Some(23),
            "and 23 bytes for 23 characters, because the group is not UTF-16"
        );
        assert_eq!(
            decoded.encode(),
            results(&reply),
            "byte for byte, both encodings in one reply"
        );
    }

    /// F12's caveat, from the 2016 capture: a device that scopes its export
    /// per host rather than to the whole subnet, which is the case that would
    /// refuse an unannounced client.
    #[test]
    fn a_real_export_reply_may_name_individual_hosts_instead_of_a_subnet() {
        let reply = hex(
            "00000002000000010000000000000000000000000000000000000001000000122f\
             0043002f004500580050004f00520054000000000000010000001f3136392e3235\
             342e3234342e3138312f3235352e3235352e3235352e32353500000000000000000\
             1000000122f0043002f004500580050004f00520054000000000000010000001f31\
             36392e3235342e3139322e3131322f3235352e3235352e3235352e3235350000000\
             00000000000",
        );
        let mount::Response::Export(exports) =
            mount::Response::parse(mount::Proc::EXPORT, results(&reply)).unwrap()
        else {
            panic!("expected an export listing");
        };
        assert_eq!(exports.len(), 2, "one entry per peer that has mounted");
        for export in &exports {
            assert_eq!(export.path.to_string_lossy(), "/C/EXPORT");
        }
        assert_eq!(
            exports
                .iter()
                .filter_map(|e| e.groups.first())
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "169.254.244.181/255.255.255.255",
                "169.254.192.112/255.255.255.255"
            ],
            "decoding these as UTF-16LE is what produced CJK mojibake"
        );
    }

    // -- NFS --------------------------------------------------------------

    /// Deck to deck, `S13` frames 742–743: the first `LOOKUP` of a path walk,
    /// and the shape of every one after it.
    #[test]
    fn a_real_lookup_walks_one_component_and_returns_a_handle_and_attributes() {
        let raw = hex(
            "000000040000000000000002000186a300000002000000040000000100000014dc\
             1ac513000000000000000000000000000000000000000000000000012538a80125\
             38a8012538a80301000000001b5800000000010301000000026600000010430 06f\
             006e00740065006e0074007300",
        );
        let parsed = call(&raw);
        assert_eq!(parsed.program, Program::NFS);
        assert_eq!((parsed.version, parsed.procedure), (2, 4));
        let request = nfs2::Request::parse(nfs2::Proc(parsed.procedure), parsed.arguments).unwrap();
        let nfs2::Request::Lookup { dir, name } = &request else {
            panic!("expected a LOOKUP, got {request:?}");
        };
        assert_eq!(name.to_string_lossy(), "Contents");
        assert_eq!(name.len_bytes(), 16, "eight characters, sixteen bytes");
        // F28 in the wild, deck to deck. The deck was served
        // `012538a8 012538a8 012538a8` followed by twenty zeroes in the MNT
        // reply above, and sends back those twelve bytes with twenty of its
        // own in their place.
        assert_eq!(
            dir.as_bytes().get(..12),
            Some(hex("012538a8012538a8012538a8").as_slice())
        );
        assert_ne!(
            dir.as_bytes().get(12..),
            Some([0u8; 20].as_slice()),
            "the twenty bytes the deck overwrote are its own bookkeeping"
        );
        assert_eq!(
            parsed.encode(),
            raw,
            "sixteen is a multiple of four, so there is no padding to differ over"
        );

        let reply = hex(
            "00000004000000010000000000000000000000000000000000000000012f1d5401\
             2538a8012538a8000000000000000000000000000000000000000000000002 0000\
             41b60000000100000000000000000000000000000200000000010000000100000002\
             012f1d5463b0cd000000000063b0cd000000000063b0cd0000000000",
        );
        let nfs2::Response::Lookup(Ok(found)) =
            nfs2::Response::parse(nfs2::Proc::LOOKUP, results(&reply)).unwrap()
        else {
            panic!("expected a successful lookup");
        };
        assert!(found.attr.is_directory());
        assert_eq!(
            found.attr.mode,
            nfs2::Fattr::DIR_MODE,
            "0o40666, not 0o40755"
        );
        assert_eq!(found.attr.rdev, nfs2::Fattr::RDEV, "1, not 0");
        assert_eq!(found.attr.fsid, nfs2::Fattr::FSID);
        assert_eq!(found.attr.blocksize, nfs2::Fattr::BLOCK_SIZE);
        assert_eq!(found.attr.mtime_sec, nfs2::Fattr::EPOCH);
        assert_eq!(
            found.attr.size, 0,
            "a deck reports every directory as empty"
        );
        assert_eq!(found.attr.blocks, 1);
        assert_eq!(
            found.attr.fileid,
            found.handle.fileid(),
            "a LOOKUP's fileid is the first word of the handle it returns"
        );
        assert_eq!(
            nfs2::Response::Lookup(Ok(found)).encode(),
            results(&reply),
            "byte for byte"
        );
        assert_eq!(
            nfs2::Fattr::directory(found.handle.fileid(), nfs2::Fattr::EPOCH),
            found.attr,
            "and our synthesised directory attributes are the deck's, field for field"
        );
    }

    /// O6's encoding trap, on real bytes: a track whose name is Japanese.
    /// Anything that assumes ASCII with interleaved zeroes corrupts this.
    #[test]
    fn a_real_lookup_of_a_japanese_filename_decodes_as_utf16le() {
        let raw = hex(
            "000000860000000000000002000186a30000000200000004000000010000001449\
             97101400000000000000000000000000000000000000000000000 08d518a807bb1\
             94a16b4070bd0301d20000001b580000000003030100000002cc00000026300032\
             002e00200041006b0069006200610020002d002000ab30ac30df302e006d007000\
             33008930",
        );
        let parsed = call(&raw);
        let nfs2::Request::Lookup { name, .. } =
            nfs2::Request::parse(nfs2::Proc(parsed.procedure), parsed.arguments).unwrap()
        else {
            panic!("expected a LOOKUP");
        };
        assert_eq!(name.to_string_lossy(), "02. Akiba - カガミ.mp3");
        assert_eq!(name.len_bytes(), 38, "19 characters, 38 bytes");
        assert_eq!(
            name.as_bytes().get(24..30),
            Some(hex("ab30ac30df30").as_slice()),
            "カガミ as three little-endian code units"
        );
        assert_eq!(
            Utf16LeString::new("02. Akiba - カガミ.mp3").as_bytes(),
            name.as_bytes(),
            "and our encoder produces exactly what the deck sent"
        );
    }

    /// The error path, from `S10f-serve-to-cdj` — the very `LOOKUP` that O6
    /// was chased down from. A failed reply is 28 bytes: no handle, no
    /// attributes, just the status.
    #[test]
    fn a_real_failed_lookup_is_a_status_and_nothing_else() {
        let reply = hex("00000026000000010000000000000000000000000000000000000002");
        assert_eq!(reply.len(), 28);
        assert_eq!(
            nfs2::Response::parse(nfs2::Proc::LOOKUP, results(&reply)).unwrap(),
            nfs2::Response::Lookup(Err(nfs2::ErrorStatus::NOENT))
        );
    }

    /// Deck to deck, `S13` frames 798–799. The fully decoded `fattr` every
    /// synthesised one is modelled on.
    #[test]
    fn a_real_getattr_reply_is_a_status_and_seventeen_words() {
        let raw = hex(
            "000000080000000000000002000186a30000000200000001000000010000001 4bc\
             690310000000000000000000000000000000000000000000000000013218440131\
             1954012538a80000000000000000000000000000000000000000",
        );
        let parsed = call(&raw);
        assert_eq!(
            raw.len(),
            92,
            "every GETATTR call in the corpus is 92 bytes"
        );
        let nfs2::Request::GetAttr(handle) =
            nfs2::Request::parse(nfs2::Proc(parsed.procedure), parsed.arguments).unwrap()
        else {
            panic!("expected a GETATTR");
        };
        assert_eq!(parsed.encode(), raw, "a bare handle needs no padding");

        let reply = hex(
            "000000080000000100000000000000000000000000000000000000000000000100\
             008000000000010000000000000000006974940000020000000001000034bb0000\
             00020132184463b0cd000000000063b0cd000000000063b0cd0000000000",
        );
        assert_eq!(reply.len(), 96, "24 header + 4 status + 68 fattr");
        let nfs2::Response::Attr(Ok(attr)) =
            nfs2::Response::parse(nfs2::Proc::GETATTR, results(&reply)).unwrap()
        else {
            panic!("expected attributes");
        };
        assert!(attr.is_regular_file());
        assert_eq!(
            attr.mode,
            nfs2::Fattr::FILE_MODE,
            "0o100000 — S_IFREG with no permission bits at all"
        );
        assert_eq!((attr.nlink, attr.uid, attr.gid), (1, 0, 0));
        assert_eq!(attr.size, 6_911_124);
        assert_eq!(attr.blocks, 13_499, "6911124 / 512, rounded up");
        assert_eq!(attr.blocksize, nfs2::Fattr::BLOCK_SIZE);
        assert_eq!(attr.rdev, nfs2::Fattr::RDEV);
        assert_eq!(attr.fsid, nfs2::Fattr::FSID);
        assert_eq!(attr.fileid, handle.fileid(), "the first word of the handle");
        for time in [attr.atime_sec, attr.mtime_sec, attr.ctime_sec] {
            assert_eq!(time, nfs2::Fattr::EPOCH, "2023-01-01T00:00:00Z, hard-coded");
        }
        assert_eq!(nfs2::Response::Attr(Ok(attr)).encode(), results(&reply));

        // And the same values our constructor synthesises, given the deck's
        // own fileid, size and epoch.
        let ours =
            nfs2::Fattr::regular_file(handle.fileid(), 6_911_124, nfs2::Fattr::EPOCH).unwrap();
        assert_eq!(
            ours, attr,
            "our synthesised fattr is the deck's, field for field"
        );
    }

    /// Deck to deck, `S06-load-and-play` frame 1036 — one of the reads that
    /// answered F18. Every READ call in the corpus is exactly 104 bytes.
    #[test]
    fn a_real_read_call_is_always_a_hundred_and_four_bytes() {
        let raw = hex(
            "0000000c0000000000000002000186a30000000200000006000000010000001 4b0\
             b631140000000000000000000000000000000000000000000000000134155401341\
             300012b2810000000000000000000000000000000000000000000000c8d00002000\
             00000000",
        );
        let parsed = call(&raw);
        assert_eq!(raw.len(), 104);
        let nfs2::Request::Read(args) =
            nfs2::Request::parse(nfs2::Proc(parsed.procedure), parsed.arguments).unwrap()
        else {
            panic!("expected a READ");
        };
        assert_eq!(args.offset, 3213);
        assert_eq!(args.count, 8192, "the NFSv2 maximum (F19)");
        assert_eq!(
            args.total_count, 0,
            "deprecated in RFC 1094 itself; zero in all 53,322 READ calls"
        );
        assert_eq!(parsed.encode(), raw);
    }

    /// The read that shows 8192 is not a ceiling: a deck asked its peer for
    /// 28,556 bytes and got them, in one datagram of about twenty IP
    /// fragments. A decoder capped at [`nfs2::MAX_DATA`] would reject the
    /// reply.
    #[test]
    fn a_real_read_may_ask_for_far_more_than_the_nfsv2_maximum() {
        let raw = hex(
            "0000025c0000000000000002000186a30000000200000006000000010000001 4ff\
             fbb503000000000000000000000000000000000000000000000000012542e401311\
             954012538a800000000000000000000000000000000000000000000002c00006f8c\
             00000000",
        );
        let parsed = call(&raw);
        let nfs2::Request::Read(args) =
            nfs2::Request::parse(nfs2::Proc(parsed.procedure), parsed.arguments).unwrap()
        else {
            panic!("expected a READ");
        };
        assert_eq!(
            args.offset, 44,
            "past a container header, not block-aligned"
        );
        assert_eq!(args.count, 28_556);
        assert!(
            args.count > u32::try_from(nfs2::MAX_DATA).unwrap(),
            "and the serving deck answered it in full"
        );
        assert!(args.count <= nfs2::MAX_READ_PAYLOAD);
    }

    /// The first hundred bytes of a real 8292-byte `READ` reply — status,
    /// `fattr`, and the payload's length prefix. Quoted this far and no
    /// further because the rest is 8192 bytes of somebody's MP3, which is
    /// stood in for below with zeroes; only the header is captured, and only
    /// the header is asserted on.
    #[test]
    fn a_real_read_reply_carries_an_empty_fattr_and_only_a_fileid() {
        let head = hex(
            "0000000c00000001000000000000000000000000000000000000000000000000\
             0000000000000000000000000000000000000000000000000000000000000000\
             0000000001341554000000000000000000000000000000000000000000000000\
             00002000",
        );
        assert_eq!(
            head.len(),
            100,
            "24 header + 4 status + 68 fattr + 4 length"
        );
        let mut body = head.clone();
        body.extend(std::iter::repeat_n(0u8, 8192));
        let nfs2::Response::Read(Ok(data)) =
            nfs2::Response::parse(nfs2::Proc::READ, results(&body)).unwrap()
        else {
            panic!("expected a read result");
        };
        assert_eq!(data.data.len(), 8192);
        // The whole point: a deck fills in nothing but the fileid.
        assert_eq!(data.attr.ftype, nfs2::FType::NON);
        assert_eq!(data.attr.mode, 0);
        assert_eq!(
            data.attr.size, 0,
            "and size zero here does NOT mean the file is empty"
        );
        assert_eq!(data.attr.blocksize, 0);
        assert_eq!(data.attr.mtime_sec, 0);
        assert_eq!(
            data.attr.fileid, 0x0134_1554,
            "the only populated field, and it is the call handle's first word"
        );
    }

    /// F28, from `S10e-serve-to-cdj`: the handle we served in an `MNT` reply
    /// and the handle the deck sent back in the next `LOOKUP`. This is the
    /// pair `docs/FINDINGS.md` quotes, and the reason a server keys its table
    /// on twelve bytes.
    #[test]
    fn a_real_deck_returns_a_handle_we_never_served() {
        let served = FileHandle::parse(&hex(
            "8a5edab282632443219e051e4ade2d1d5bbc671c781051bf1437897cbdfea0f1",
        ))
        .unwrap();
        let returned = FileHandle::parse(&hex(
            "8a5edab282632443219e051e03012d0000001b58000000000303010000000162",
        ))
        .unwrap();
        assert_ne!(served, returned, "a spec-conformant server sees a stranger");
        assert_eq!(
            served.key(),
            returned.key(),
            "and the twelve bytes it may rely on are untouched"
        );
        assert_eq!(served.fileid(), returned.fileid());
    }
}

/// Every parser here, hammered with malformed input.
///
/// This is a network input path reachable by anyone on the link, and the
/// workspace forbids panicking outside tests precisely so that a hostile
/// datagram costs an `Err` rather than the process. Reasoning that no reachable
/// input can panic is not the same as trying a few million of them, so this
/// tries them: truncations, single-byte mutations and corrupted length
/// prefixes, derived from the real datagrams above and from pseudo-random
/// noise.
///
/// The assertion is only "it returned". Whether it returned `Ok` or `Err` is
/// the business of the tests above; what is being pinned here is that a peer
/// cannot take the process down, and that no length prefix can make us
/// allocate before we have checked it.
#[cfg(test)]
mod fuzz {
    use super::*;

    /// A deterministic generator, so a failure reproduces. `binrw` and
    /// `thiserror` are the only dependencies this crate has, so there is no
    /// `rand` to reach for and none is wanted: a seeded sequence is what makes
    /// a red test actionable.
    struct Lcg(u64);

    impl Lcg {
        fn next_u32(&mut self) -> u32 {
            // Numerical Recipes' constants; any full-period LCG will do.
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            u32::try_from(self.0 >> 32).unwrap_or(0)
        }

        fn next_byte(&mut self) -> u8 {
            u8::try_from(self.next_u32() & 0xff).unwrap_or(0)
        }

        fn below(&mut self, limit: usize) -> usize {
            if limit == 0 {
                0
            } else {
                usize::try_from(self.next_u32()).unwrap_or(0) % limit
            }
        }
    }

    /// Push `data` through every entry point this crate exposes.
    fn parse_every_way(data: &[u8]) {
        let _ = Message::parse(data);
        let _ = Reply::parse(data);
        let _ = AuthUnix::parse(data);

        if let Ok(call) = Call::parse(data) {
            // A well-formed header: hand the arguments to every program, since
            // a server on the wrong port receives exactly this.
            for procedure in 0..20u32 {
                let _ = portmap::Request::parse(portmap::Proc(procedure), call.arguments);
                let _ = mount::Request::parse(mount::Proc(procedure), call.arguments);
                let _ = nfs2::Request::parse(nfs2::Proc(procedure), call.arguments);
            }
        }
        // And as results, which is the client's side of the same problem.
        for procedure in 0..20u32 {
            let _ = portmap::Response::parse(portmap::Proc(procedure), data);
            let _ = mount::Response::parse(mount::Proc(procedure), data);
            let _ = nfs2::Response::parse(nfs2::Proc(procedure), data);
        }

        // The XDR primitives directly, since a caller may reach them.
        let mut reader = xdr::Reader::new(data);
        let _ = reader.u32();
        let _ = reader.u64();
        let _ = reader.opaque_var(xdr::MAX_STRING, "fuzz");
        let _ = reader.utf16le_string(xdr::MAX_STRING);
        let _ = reader.ascii_string(xdr::MAX_STRING);
        let _ = reader.u32_array(16);
        let _ = reader.opaque_fixed(nfs2::FHANDLE_LEN);
        let _ = xdr::Utf16LeString::from_bytes(data).to_string_lossy();
    }

    #[test]
    fn no_random_datagram_can_panic_a_parser() {
        let mut rng = Lcg(0x5eed_1e55);
        for _ in 0..20_000 {
            let len = rng.below(160);
            let bytes: Vec<u8> = (0..len).map(|_| rng.next_byte()).collect();
            parse_every_way(&bytes);
        }
    }

    /// One well-formed datagram per shape a peer can send us, to be broken.
    fn valid_datagrams() -> Vec<Vec<u8>> {
        let credential = AuthUnix::cdj(STAMP_FIRST_CALL).encode();
        vec![
            Call::new(
                Xid(1),
                Program::PORTMAP,
                2,
                portmap::Proc::GETPORT.0,
                Auth::unix(&credential),
                &portmap::Request::GetPort(portmap::Mapping::query(
                    Program::MOUNT,
                    1,
                    IpProtocol::UDP,
                ))
                .encode_arguments(),
            )
            .encode(),
            Call::new(
                Xid(2),
                Program::MOUNT,
                1,
                mount::Proc::MNT.0,
                Auth::unix(&credential),
                &mount::Request::Mnt(xdr::Utf16LeString::new("/C/")).encode_arguments(),
            )
            .encode(),
            Call::new(
                Xid(3),
                Program::NFS,
                2,
                nfs2::Proc::LOOKUP.0,
                Auth::unix(&credential),
                &nfs2::Request::Lookup {
                    dir: FileHandle::ZERO,
                    name: xdr::Utf16LeString::new("Contents"),
                }
                .encode_arguments(),
            )
            .encode(),
            Reply::success(
                Xid(3),
                &nfs2::Response::Lookup(Ok(nfs2::FileRef {
                    handle: FileHandle::ZERO,
                    attr: nfs2::Fattr::directory(1, nfs2::Fattr::EPOCH),
                }))
                .encode(),
            )
            .encode(),
            Reply::success(
                Xid(4),
                &mount::Response::Export(vec![mount::Export::new(
                    "/C/",
                    &[mount::Export::LINK_LOCAL_SUBNET],
                )])
                .encode(),
            )
            .encode(),
            Reply::success(
                Xid(5),
                &portmap::Response::Dump(
                    portmap::cdj_registrations(111, mount::PIONEER_PORT, nfs2::PORT).to_vec(),
                )
                .encode(),
            )
            .encode(),
            Reply::success(
                Xid(6),
                &nfs2::Response::ReadDir(Ok(nfs2::Listing {
                    entries: vec![nfs2::DirEntry {
                        fileid: 1,
                        name: xdr::Utf16LeString::new("PIONEER"),
                        cookie: nfs2::Cookie([0, 0, 0, 1]),
                    }],
                    eof: true,
                }))
                .encode(),
            )
            .encode(),
        ]
    }

    /// Random noise rarely gets past a header check, so most of the work is
    /// done here: start from datagrams that *are* valid and break them one
    /// byte at a time.
    #[test]
    fn no_mutation_of_a_valid_datagram_can_panic_a_parser() {
        let mut rng = Lcg(0xc0ff_ee42);
        for seed in &valid_datagrams() {
            // Every truncation, which is the commonest real malformation.
            for cut in 0..=seed.len() {
                parse_every_way(seed.get(..cut).unwrap_or(seed));
            }
            // Every single-byte position, flipped to a spread of values —
            // including the 0xff that turns a length prefix hostile.
            for position in 0..seed.len() {
                for value in [0x00, 0x01, 0x7f, 0x80, 0xfe, 0xff] {
                    let mut mutated = seed.clone();
                    if let Some(slot) = mutated.get_mut(position) {
                        *slot = value;
                    }
                    parse_every_way(&mutated);
                }
            }
            // And a few thousand multi-byte scrambles per seed.
            for _ in 0..2_000 {
                let mut mutated = seed.clone();
                for _ in 0..=rng.below(6) {
                    let position = rng.below(mutated.len().max(1));
                    if let Some(slot) = mutated.get_mut(position) {
                        *slot = rng.next_byte();
                    }
                }
                parse_every_way(&mutated);
            }
        }
    }

    /// The property that must survive every port of this code: a length prefix
    /// claiming four gigabytes costs a parse failure, not four gigabytes.
    ///
    /// Asserted at the offsets where a length prefix actually lives, so the
    /// test fails if the cap is ever removed. The sweep below is the companion
    /// to it and only checks that nothing panics — worth having, but it would
    /// pass with every cap deleted, which is why this one exists.
    #[test]
    fn a_hostile_length_prefix_is_refused_rather_than_allocated() {
        // A LOOKUP call: 24 header + 8 credential head + 20 body + 8 verifier
        // + 32 filehandle puts the name's length prefix at 92.
        let credential = AuthUnix::cdj(STAMP_FIRST_CALL).encode();
        let arguments = nfs2::Request::Lookup {
            dir: FileHandle::ZERO,
            name: xdr::Utf16LeString::new("Contents"),
        }
        .encode_arguments();
        let mut call = Call::new(
            Xid(1),
            Program::NFS,
            2,
            nfs2::Proc::LOOKUP.0,
            Auth::unix(&credential),
            &arguments,
        )
        .encode();
        assert_eq!(call.get(92..96), Some(16u32.to_be_bytes().as_slice()));
        if let Some(slot) = call.get_mut(92..96) {
            slot.copy_from_slice(&0xffff_ffffu32.to_be_bytes());
        }
        let parsed = Call::parse(&call).expect("the header is still intact");
        let error = nfs2::Request::parse(nfs2::Proc::LOOKUP, parsed.arguments)
            .expect_err("a four-gigabyte name must be refused");
        assert!(
            matches!(error, Error::ImplausibleLength { limit, .. } if limit == u64::from(nfs2::MAX_NAME)),
            "expected a capped refusal, got {error:?}"
        );
        assert!(
            !error.is_truncated(),
            "and it is not truncation: more bytes would not help"
        );

        // The credential body's own prefix, at offset 28, is capped at RFC
        // 1057's 400 bytes by the framing layer itself.
        let mut call = Call::new(
            Xid(1),
            Program::NFS,
            2,
            nfs2::Proc::LOOKUP.0,
            Auth::unix(&credential),
            &arguments,
        )
        .encode();
        if let Some(slot) = call.get_mut(28..32) {
            slot.copy_from_slice(&0xffff_ffffu32.to_be_bytes());
        }
        assert!(
            matches!(
                Call::parse(&call),
                Err(Error::ImplausibleLength { limit: 400, .. })
            ),
            "an oversized opaque_auth body must be refused before allocating"
        );
    }

    /// The companion sweep: hostile prefixes at *every* offset, checking only
    /// that nothing panics or hangs. See the test above for the assertion that
    /// the caps are doing anything.
    #[test]
    fn no_length_prefix_at_any_offset_can_panic_a_parser() {
        let credential = AuthUnix::cdj(STAMP_FIRST_CALL).encode();
        let seed = Call::new(
            Xid(1),
            Program::NFS,
            2,
            nfs2::Proc::LOOKUP.0,
            Auth::unix(&credential),
            &nfs2::Request::Lookup {
                dir: FileHandle::ZERO,
                name: xdr::Utf16LeString::new("Contents"),
            }
            .encode_arguments(),
        )
        .encode();

        for position in 0..seed.len().saturating_sub(4) {
            for claim in [0xffff_ffffu32, 0x8000_0000, 0x0010_0000, 0x0000_ffff] {
                let mut mutated = seed.clone();
                if let Some(slot) = mutated.get_mut(position..position + 4) {
                    slot.copy_from_slice(&claim.to_be_bytes());
                }
                parse_every_way(&mutated);
            }
        }
    }
}

/// Every procedure driven through all four halves, in order.
///
/// This layer exists twice over: we build calls and parse replies as a client,
/// and parse calls and build replies as a server. A codec can be
/// self-consistent in each direction separately and still have the two
/// disagree — an argument encoder that writes a field the argument parser does
/// not read is invisible to any test that only round-trips one of them.
///
/// So each test here runs the whole loop a real exchange runs:
///
/// ```text
/// client builds a call ─► server parses it ─► server builds a reply ─► client parses that
/// ```
///
/// and checks that what the client asked for is what the server saw, and that
/// what the server answered is what the client got. The captured-bytes tests
/// above are what tie this to a CDJ; this is what ties the two directions to
/// each other.
#[cfg(test)]
mod loopback {
    use super::*;
    use crate::rpc::xdr::Utf16LeString;

    const HANDLE: FileHandle = FileHandle([7; nfs2::FHANDLE_LEN]);

    /// Put a call on the wire and take it off again, as a server would.
    fn exchange_call<'a>(
        program: Program,
        version: u32,
        procedure: u32,
        arguments: &'a [u8],
        datagram: &'a mut Vec<u8>,
    ) -> Call<'a> {
        let credential = AuthUnix::cdj(STAMP_FIRST_CALL).encode();
        *datagram = Call::new(
            Xid(0x0350_0001),
            program,
            version,
            procedure,
            Auth::unix(&credential),
            arguments,
        )
        .encode();
        let parsed = Call::parse(datagram).expect("a call we built must parse");
        assert_eq!(parsed.program, program);
        assert_eq!(parsed.version, version);
        assert_eq!(parsed.procedure, procedure);
        assert_eq!(parsed.xid, Xid(0x0350_0001));
        parsed
    }

    /// Put a reply on the wire and take it off again, as a client would.
    fn exchange_reply<'a>(results: &'a [u8], datagram: &'a mut Vec<u8>) -> &'a [u8] {
        *datagram = Reply::success(Xid(0x0350_0001), results).encode();
        let parsed = Reply::parse(datagram).expect("a reply we built must parse");
        assert_eq!(parsed.xid(), Xid(0x0350_0001));
        parsed.results().expect("a success carries results")
    }

    #[test]
    fn every_portmap_procedure_survives_a_full_exchange() {
        let cases = [
            (portmap::Request::Null, portmap::Response::Null),
            (
                portmap::Request::GetPort(portmap::Mapping::query(
                    Program::MOUNT,
                    1,
                    IpProtocol::UDP,
                )),
                portmap::Response::GetPort(Some(mount::PIONEER_PORT)),
            ),
            (
                portmap::Request::GetPort(portmap::Mapping::query(
                    Program::NFS,
                    2,
                    IpProtocol::UDP,
                )),
                // The empty slot's honest answer, and the one most easily
                // mistaken for a failure.
                portmap::Response::GetPort(None),
            ),
            (
                portmap::Request::Dump,
                portmap::Response::Dump(
                    portmap::cdj_registrations(portmap::PORT, mount::PIONEER_PORT, nfs2::PORT)
                        .to_vec(),
                ),
            ),
        ];

        for (request, response) in cases {
            let arguments = request.encode_arguments();
            let mut wire = Vec::new();
            let call = exchange_call(
                Program::PORTMAP,
                portmap::VERSION,
                request.procedure().0,
                &arguments,
                &mut wire,
            );
            let seen = portmap::Request::parse(portmap::Proc(call.procedure), call.arguments)
                .expect("the server must decode what the client encoded");
            assert_eq!(seen, request, "the call did not survive the wire");

            let results = response.encode();
            let mut wire = Vec::new();
            let got = portmap::Response::parse(
                portmap::Proc(call.procedure),
                exchange_reply(&results, &mut wire),
            )
            .expect("the client must decode what the server encoded");
            assert_eq!(got, response, "the reply did not survive the wire");
        }
    }

    #[test]
    fn every_mount_procedure_survives_a_full_exchange() {
        let cases = [
            (mount::Request::Null, mount::Response::Null),
            (
                mount::Request::Mnt(Utf16LeString::new(mount::EXPORT_USB)),
                mount::Response::Mnt(Ok(HANDLE)),
            ),
            (
                // An unaligned path, so the padding is exercised too.
                mount::Request::Mnt(Utf16LeString::new("/C/EXPORT")),
                mount::Response::Mnt(Err(nfs2::ErrorStatus::ACCES)),
            ),
            (
                mount::Request::Umnt(Utf16LeString::new(mount::EXPORT_SD)),
                mount::Response::Umnt,
            ),
            (mount::Request::UmntAll, mount::Response::UmntAll),
            (
                mount::Request::Dump,
                mount::Response::Dump(vec![mount::MountEntry {
                    hostname: "169.254.99.100".to_owned(),
                    directory: Utf16LeString::new(mount::EXPORT_USB),
                }]),
            ),
            (
                mount::Request::Export,
                mount::Response::Export(vec![
                    mount::Export::new(mount::EXPORT_SD, &[mount::Export::LINK_LOCAL_SUBNET]),
                    mount::Export::new(mount::EXPORT_USB, &[mount::Export::LINK_LOCAL_SUBNET]),
                ]),
            ),
        ];

        for (request, response) in cases {
            let arguments = request.encode_arguments();
            let mut wire = Vec::new();
            let call = exchange_call(
                Program::MOUNT,
                mount::VERSION,
                request.procedure().0,
                &arguments,
                &mut wire,
            );
            let seen = mount::Request::parse(mount::Proc(call.procedure), call.arguments)
                .expect("the server must decode what the client encoded");
            assert_eq!(seen, request, "the call did not survive the wire");

            let results = response.encode();
            let mut wire = Vec::new();
            let got = mount::Response::parse(
                mount::Proc(call.procedure),
                exchange_reply(&results, &mut wire),
            )
            .expect("the client must decode what the server encoded");
            assert_eq!(got, response, "the reply did not survive the wire");
        }
    }

    /// One call and one plausible answer for every NFS procedure we model,
    /// plus the two error shapes that matter and one we refuse.
    fn nfs_exchanges(payload: &[u8]) -> Vec<(nfs2::Request, nfs2::Response<'_>)> {
        let attr = nfs2::Fattr::regular_file(1, 7_633_531, nfs2::Fattr::EPOCH)
            .expect("a 7 MB file is well inside the 32-bit ceiling");
        vec![
            (nfs2::Request::Null, nfs2::Response::Null),
            (
                nfs2::Request::GetAttr(HANDLE),
                nfs2::Response::Attr(Ok(attr)),
            ),
            (
                nfs2::Request::GetAttr(HANDLE),
                // A stale handle is what a media swap looks like from here.
                nfs2::Response::Attr(Err(nfs2::ErrorStatus::STALE)),
            ),
            (
                nfs2::Request::Lookup {
                    dir: HANDLE,
                    // Unaligned and non-ASCII at once: 19 characters, 38
                    // bytes, two of padding.
                    name: Utf16LeString::new("02. Akiba - カガミ.mp3"),
                },
                nfs2::Response::Lookup(Ok(nfs2::FileRef {
                    handle: HANDLE,
                    attr,
                })),
            ),
            (
                nfs2::Request::Lookup {
                    dir: HANDLE,
                    name: Utf16LeString::new("Contents"),
                },
                nfs2::Response::Lookup(Err(nfs2::ErrorStatus::NOENT)),
            ),
            (
                nfs2::Request::Read(nfs2::ReadArgs::at(HANDLE, 8192, 8192).unwrap()),
                nfs2::Response::Read(Ok(nfs2::FileData {
                    attr,
                    data: payload,
                })),
            ),
            (
                nfs2::Request::ReadDir(nfs2::ReadDirArgs {
                    handle: HANDLE,
                    cookie: nfs2::Cookie::START,
                    count: 4096,
                }),
                nfs2::Response::ReadDir(Ok(nfs2::Listing {
                    entries: vec![
                        nfs2::DirEntry {
                            fileid: 2,
                            name: Utf16LeString::new("PIONEER"),
                            cookie: nfs2::Cookie([0, 0, 0, 1]),
                        },
                        nfs2::DirEntry {
                            fileid: 3,
                            name: Utf16LeString::new("カガミ"),
                            cookie: nfs2::Cookie([0, 0, 0, 2]),
                        },
                    ],
                    eof: false,
                })),
            ),
            (
                nfs2::Request::StatFs(HANDLE),
                nfs2::Response::StatFs(Ok(nfs2::FsStat {
                    tsize: 8192,
                    bsize: 512,
                    blocks: 1_000_000,
                    bfree: 0,
                    bavail: 0,
                })),
            ),
            (
                // A write, which we refuse — but the argument block must still
                // survive intact, because that is what lets a server answer
                // PROC_UNAVAIL rather than drop the datagram.
                nfs2::Request::Unknown {
                    procedure: nfs2::Proc::WRITE,
                    arguments: vec![1, 2, 3, 4, 5, 6, 7, 8],
                },
                nfs2::Response::Null,
            ),
        ]
    }

    #[test]
    fn every_nfs_procedure_survives_a_full_exchange() {
        let payload = vec![0x5a; 1280];
        for (request, response) in nfs_exchanges(&payload) {
            let arguments = request.encode_arguments();
            let mut wire = Vec::new();
            let call = exchange_call(
                Program::NFS,
                nfs2::VERSION,
                request.procedure().0,
                &arguments,
                &mut wire,
            );
            let seen = nfs2::Request::parse(nfs2::Proc(call.procedure), call.arguments)
                .expect("the server must decode what the client encoded");
            assert_eq!(seen, request, "the call did not survive the wire");

            if matches!(request, nfs2::Request::Unknown { .. }) {
                continue; // No reply codec, by design: it gets PROC_UNAVAIL.
            }
            let results = response.encode();
            let mut wire = Vec::new();
            let got = nfs2::Response::parse(
                nfs2::Proc(call.procedure),
                exchange_reply(&results, &mut wire),
            )
            .expect("the client must decode what the server encoded");
            assert_eq!(got, response, "the reply did not survive the wire");
        }
    }

    /// The refusals a server actually sends. None appears in the corpus — no
    /// captured reply is anything but SUCCESS — so these are RFC 1057 as
    /// written, and this is the only check they get.
    #[test]
    fn every_refusal_a_server_can_send_survives_the_wire() {
        let refusals = [
            Reply::failed(Xid(1), FailureStat::PROG_UNAVAIL),
            Reply::failed(Xid(1), FailureStat::PROC_UNAVAIL),
            Reply::failed(Xid(1), FailureStat::GARBAGE_ARGS),
            Reply::failed(Xid(1), FailureStat::SYSTEM_ERR),
            Reply::Accepted {
                xid: Xid(1),
                verifier: Auth::NULL,
                status: Accepted::ProgMismatch { low: 2, high: 2 },
            },
            Reply::Denied {
                xid: Xid(1),
                reason: Denied::AuthError(AuthStat::TOOWEAK),
            },
            Reply::Denied {
                xid: Xid(1),
                reason: Denied::RpcMismatch { low: 2, high: 2 },
            },
            Reply::prog_mismatch(Xid(1), 2, 2),
            Reply::Denied {
                xid: Xid(1),
                reason: Denied::Other(
                    OtherReject::new(RejectStat(9)).expect("9 is not a defined reject_stat"),
                ),
            },
        ];
        for refusal in refusals {
            let wire = refusal.encode();
            let parsed = Reply::parse(&wire).expect("a refusal we built must parse");
            assert_eq!(parsed, refusal);
            assert_eq!(
                parsed.results(),
                None,
                "a refusal carries no results, and a caller must not find any"
            );
            assert_eq!(parsed.encode(), wire, "and it re-encodes byte for byte");
        }
    }

    /// A listing of exactly the decoder's cap is legal and must survive.
    ///
    /// The caps exist to stop a hostile datagram allocating without bound, not
    /// to limit what a peer may legitimately say. Checked with `>=` they were
    /// off by one, so our own encoder produced listings our own decoder
    /// refused — a bug no round-trip of a *small* listing could find.
    #[test]
    fn a_listing_of_exactly_the_cap_survives_and_one_more_does_not() {
        // portmap DUMP, cap 512.
        let mapping = portmap::Mapping::registered(Program::NFS, 2, IpProtocol::UDP, nfs2::PORT);
        for (count, should_parse) in [(512usize, true), (513, false)] {
            let listing = portmap::Response::Dump(vec![mapping; count]);
            let parsed = portmap::Response::parse(portmap::Proc::DUMP, &listing.encode());
            assert_eq!(
                parsed.is_ok(),
                should_parse,
                "a portmap DUMP of {count} mappings"
            );
        }

        // MOUNT EXPORT, cap 64 — both the export list and each group list.
        for (count, should_parse) in [(64usize, true), (65, false)] {
            let listing = mount::Response::Export(vec![
                mount::Export::new(
                    "/C/",
                    &["169.254.0.0/255.255.0.0"]
                );
                count
            ]);
            let parsed = mount::Response::parse(mount::Proc::EXPORT, &listing.encode());
            assert_eq!(parsed.is_ok(), should_parse, "an EXPORT of {count} entries");

            let groups = vec!["169.254.0.0/255.255.0.0"; count];
            let listing = mount::Response::Export(vec![mount::Export::new("/C/", &groups)]);
            let parsed = mount::Response::parse(mount::Proc::EXPORT, &listing.encode());
            assert_eq!(
                parsed.is_ok(),
                should_parse,
                "an export with {count} groups"
            );
        }

        // MOUNT DUMP, cap 64.
        let entry = mount::MountEntry {
            hostname: "169.254.99.100".to_owned(),
            directory: Utf16LeString::new("/C/"),
        };
        for (count, should_parse) in [(64usize, true), (65, false)] {
            let listing = mount::Response::Dump(vec![entry.clone(); count]);
            let parsed = mount::Response::parse(mount::Proc::DUMP, &listing.encode());
            assert_eq!(parsed.is_ok(), should_parse, "a MOUNT DUMP of {count}");
        }
    }
}
