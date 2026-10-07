// SPDX-License-Identifier: GPL-3.0-only

//! Reading a player's media: the ONC RPC, MOUNT and NFSv2 client.
//!
//! This is the passive half of consuming. A CDJ exports its SD card and its USB
//! stick to the whole link-local subnet (`169.254.0.0/255.255.0.0`), so a host
//! that has never announced itself is inside the permitted set by default (F11,
//! F12) and can pull `export.pdb` and every audio file behind it without
//! touching UDP 50000 at all.
//!
//! ```text
//! GETPORT(mountd) ─► GETPORT(nfsd) ─► MNT '/C/' ─► LOOKUP × n ─► READ × n ─► UMNT
//!   udp/111           udp/111          udp/48276     udp/2049     udp/2049
//! ```
//!
//! # Ports are discovered even though they never move
//!
//! mountd answers on **48276** and nfsd on **2049** on every device anyone has
//! looked at — three independent observations across three devices (F6). They
//! are still discovered rather than assumed, because 48276 is not a registered
//! number and because a `GETPORT` costs one datagram. When a program is *not*
//! registered, a bare `GETPORT` cannot tell "this device runs no RPC at all"
//! from "it runs RPC and exports nothing", so a failed lookup falls back to a
//! `DUMP` and the error says which of the two it was. Both ports are resolved
//! once, at connect: the C++ port re-resolved them on every mount and paid two
//! extra round trips per medium for it.
//!
//! # A retransmission reuses its xid, deliberately
//!
//! This runs over UDP, so a lost datagram is a normal event and a dead peer
//! must time out rather than hang. Every call is retried
//! [`NfsConfig::attempts`] times with the **same** xid — RFC 1057's own advice,
//! and the reason is that a late first reply and the retry's reply are then
//! recognisable as duplicates of one another rather than as two answers. A
//! reply carrying any other xid is discarded and the wait continues, because it
//! answers a call we have already given up on and decoding it here would read
//! one procedure's results as another's.
//!
//! The reference implementation measured **842 `READ`s with 0 retries at 1459
//! KiB/s** pulling a 1 MB `export.pdb`, so on a quiet link the retry path is
//! dead code — which is exactly why it is tested deliberately rather than left
//! to be exercised in the field.
//!
//! # One call in flight, and why that is not the slow choice it looks like
//!
//! Both reference clients pipeline four reads at once. This one does not, and
//! the arithmetic is the reason: their 1459 KiB/s came from a window of four
//! **1280-byte** reads, and a single **8192-byte** read moves 6.4× the payload
//! per round trip. Sequential-at-8192 therefore beats windowed-at-1280 on the
//! same link, while keeping the retry rule small enough to be obviously
//! correct. Windowing on top of 8192 would multiply again and is untried here;
//! it is the first thing to reach for if a 75 MB track ever feels slow.
//!
//! # Read size is the one real tuning decision
//!
//! Real CDJs ask for **8192** bytes, the NFSv2 maximum, and rely on IP
//! fragmentation: the reply is one ~8.3 KB datagram in six fragments on a
//! 1500-byte MTU, and losing any one of them loses the whole read (F19).
//! [`ReadSize::UNFRAGMENTED`] is 1280, which fits a single frame — safe, and
//! **6.4× more round trips** for the same bytes. The default here is
//! [`ReadSize::CDJ`], because the point of this library is to behave like the
//! hardware and because a link that cannot reassemble fragments will also fail
//! the first `READ` a real deck makes. Switch to 1280 when a network is
//! dropping fragments; nothing else about the client changes.
//!
//! A short read is normal and means "ask for the shortfall", never "the file
//! ended". Nothing in a `READ` reply says where the end is: a real CDJ sends a
//! `fattr` that is **entirely zero apart from `fileid`** in 7884 of 7884 replies
//! (see [`nfs2::FileData::attr`]), so `size == 0` there is not end of file and a
//! client that believed it would truncate every transfer. The size comes from
//! the `LOOKUP` or `GETATTR` that opened the file, which is why [`RemoteFile`]
//! exists and carries it.
//!
//! # Directory handles are cached because a player's handle table is finite
//!
//! The single most expensive lesson in the reference port. A CDJ keeps a
//! bounded table of the handles it has issued and starts answering
//! `NFSERR_STALE` once that table churns: re-walking `PIONEER/Artwork/000NN`
//! from the root for every cover minted roughly **2300 handles** across one
//! medium and **495 of 576 fetches failed**, all on the last lookup of the
//! path. A real CDJ uses **four** distinct directory handles across
//! forty-eight lookups, because it remembers the directory it is in. So does
//! [`NfsClient`]: every directory a walk passes through is cached per mount, so
//! a path whose folder is already known costs **one** `LOOKUP` instead of four,
//! and a leaf that misses drops that cached folder and re-walks from the root
//! exactly once — so a file that is genuinely gone fails rather than looping.
//! Where that still is not enough — the medium was swapped — the answer is
//! [`ErrorStatus::STALE`], and [`NfsClient::refresh`] is how to recover.
//!
//! # Filehandles travel back verbatim
//!
//! A CDJ keeps only the leading twelve bytes of a handle it was served and
//! overwrites the rest (F28) — that is a *server's* problem. As a client the
//! rule is the plain one: echo back exactly the 32 bytes the peer gave, never
//! parse or normalise them.
//!
//! # `NFSERR_ACCES` on `MNT` means "announce first", not "give up"
//!
//! One device in a published capture scoped its export to two per-host entries
//! rather than to the whole subnet. Against that device an unannounced client
//! is refused, and the remedy is [`crate::VirtualCdj`] rather than an error to
//! the user (F12). The `EXPORT` listing names the permitted hosts, so it also
//! diagnoses the refusal.
//!
//! # Names are spelled differently in the database and on the medium
//!
//! `export.pdb` says `Gesaffelstein` where the directory is `GESAFFELSTEIN`,
//! and it stores NFC where a FAT32 driver reports NFD (`カガミ`). A server
//! comparing bytes answers `NFSERR_NOENT` for a file that is plainly there;
//! `crate::serve::vfs` documents that side. As a *client* we cannot list a
//! directory to find out — a real deck answers `READDIR` with `PROC_UNAVAIL` —
//! so [`NfsClient::walk`] retries a `NOENT` component through the handful of
//! spellings the two sides are known to disagree on, and gives up with the
//! status the peer actually sent.

use std::collections::{BTreeSet, HashMap};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::num::NonZeroU8;
use std::time::Duration;

use prolink_proto::Slot;
use prolink_proto::rpc::nfs2::{self, ErrorStatus, Fattr, FileHandle, Status};
use prolink_proto::rpc::xdr::Utf16LeString;
use prolink_proto::rpc::{
    self, Accepted, Auth, AuthUnix, Call, Denied, IpProtocol, Program, Reply, Xid, mount, portmap,
};
use tokio::net::UdpSocket;
use tracing::{debug, trace, warn};
use unicode_normalization::UnicodeNormalization;

use crate::interface::Interface;
use crate::socket::{self, MAX_DATAGRAM};
use crate::{Error, Result};

/// Where a rekordbox medium keeps its database, relative to a mount root.
///
/// The one file worth pulling whole: about 1 MB on a typical stick, and the
/// only way to learn what is on the medium without a dbserver session.
pub const EXPORT_PDB: &str = "/PIONEER/rekordbox/export.pdb";

/// How many bytes one `READ` asks for.
///
/// A newtype so the NFSv2 ceiling is proven once, at construction, rather than
/// re-checked or — worse — silently truncated into the 32-bit count field.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReadSize(u32);

impl ReadSize {
    /// **8192** — the NFSv2 maximum, and what real CDJs ask for (F19).
    ///
    /// The reply is one datagram of roughly 8.3 KB, which a 1500-byte MTU
    /// carries as six IP fragments; lose any one of them and the whole read is
    /// lost. Hardware does this anyway, in both directions, so a link that
    /// cannot carry it cannot carry a real track load either.
    pub const CDJ: Self = Self(8192);

    /// **1280** — small enough that a reply fits one frame.
    ///
    /// What the reference client defaults to, measured at 1459 KiB/s with four
    /// reads in flight. **6.4× more round trips** than [`ReadSize::CDJ`] for
    /// the same file. Worth choosing on a link that is dropping fragments and
    /// nowhere else.
    pub const UNFRAGMENTED: Self = Self(1280);

    /// A read size, or `None` outside `1..=8192`.
    ///
    /// The upper bound is [`nfs2::MAX_DATA`], what RFC 1094 permits a client to
    /// ask for. Hardware *answers* more than that — a deck has been seen asking
    /// a peer for 28584 — but asking for more than the specification allows is
    /// a good way to find a server that refuses.
    pub fn new(bytes: u32) -> Option<Self> {
        let limit = u32::try_from(nfs2::MAX_DATA).unwrap_or(u32::MAX);
        (bytes > 0 && bytes <= limit).then_some(Self(bytes))
    }

    /// The count to put in a `READ` call.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl Default for ReadSize {
    fn default() -> Self {
        Self::CDJ
    }
}

impl core::fmt::Debug for ReadSize {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}B", self.0)
    }
}

/// Timeouts, retries and read size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NfsConfig {
    /// Bytes per `READ`. See [`ReadSize`] for the trade-off.
    pub read_size: ReadSize,
    /// How long to wait for one reply before retransmitting.
    ///
    /// Per *attempt*, not per call: a call gives up after
    /// [`NfsConfig::attempts`] of these. The two reference clients disagree by
    /// a factor of six here — 2 s × 6 and 250 ms × 8 — and neither measured it.
    /// 500 ms × 4 sits between them: generous for a link-local segment where a
    /// real read completes in under a millisecond, and short enough that a dead
    /// peer is noticed in two seconds rather than twelve.
    pub timeout: Duration,
    /// How many times one call may be sent, retransmissions included.
    ///
    /// Non-zero by type: a call that is never sent is not a policy, it is a
    /// hang with extra steps.
    pub attempts: NonZeroU8,
    /// Where the portmapper answers.
    ///
    /// [`portmap::PORT`] — 111 — on every real device, and configurable only so
    /// that a test can stand a server up without root. Nothing else in this
    /// module takes a port: the other two are discovered.
    pub portmap_port: u16,
}

impl Default for NfsConfig {
    fn default() -> Self {
        Self {
            read_size: ReadSize::default(),
            timeout: Duration::from_millis(500),
            attempts: NonZeroU8::new(4).unwrap_or(NonZeroU8::MIN),
            portmap_port: portmap::PORT,
        }
    }
}

/// The two ports a `GETPORT` pair resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RpcPorts {
    /// Where MOUNT answers. 48276 on every observed device (F6).
    pub mount: u16,
    /// Where NFS answers. 2049, the registered number, but still discovered.
    pub nfs: u16,
}

/// One mounted export, and proof that `MNT` succeeded.
///
/// Holds the root filehandle every later `LOOKUP` starts from, so there is no
/// way to walk a path on an export that was never mounted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    export: String,
    slot: Option<Slot>,
    root: FileHandle,
}

impl Mount {
    /// The export path, exactly as it was mounted.
    ///
    /// Worth keeping verbatim: one capture shows the same player mounting
    /// `/C/` on one peer and `/C/EXPORT` on another (C6).
    pub fn export(&self) -> &str {
        &self.export
    }

    /// Which slot this export is, matched on the drive-letter prefix.
    ///
    /// `None` for a path no documented slot claims — `/A/` is used by no
    /// observed client and is presumed internal.
    pub fn slot(&self) -> Option<Slot> {
        self.slot
    }

    /// The root filehandle.
    pub fn root(&self) -> FileHandle {
        self.root
    }
}

/// A regular file on a peer's medium, with the size a read needs.
///
/// Only ever built from a `LOOKUP` or `GETATTR` that reported
/// [`nfs2::FType::REG`], so "this is a file and I know how long it is" is a
/// property of the type. That matters because a `READ` reply from a real CDJ
/// carries no usable size at all, and a client that tried to take one from
/// there would stop at the first read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    path: String,
    handle: FileHandle,
    size: u64,
}

impl RemoteFile {
    /// The path as it was asked for.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The handle, to be echoed back verbatim.
    pub fn handle(&self) -> FileHandle {
        self.handle
    }

    /// The file's length in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }
}

/// How far a whole-file pull has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Bytes read so far, contiguous from the start.
    pub read: u64,
    /// Bytes the file holds, from the `LOOKUP` that opened it.
    pub total: u64,
}

impl Progress {
    /// A fraction in `0.0..=1.0`, or `1.0` for a zero-length file.
    pub fn fraction(self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        let read = u32::try_from(self.read.min(self.total)).unwrap_or(u32::MAX);
        let total = u32::try_from(self.total).unwrap_or(u32::MAX);
        f64::from(read) / f64::from(total)
    }
}

/// What the client has done since it connected.
///
/// `retries` is the interesting one: the reference implementation's 1 MB pull
/// took 842 `READ`s and needed none, so anything above zero here is a link
/// worth looking at rather than a normal cost of doing business. `lookups` is
/// the other: a walk that is hitting the directory cache issues one per file,
/// and one that is not issues one per component and will eventually exhaust a
/// player's handle table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NfsStats {
    /// Datagrams sent, counting each retransmission.
    pub datagrams: u64,
    /// Retransmissions, i.e. datagrams beyond the first attempt of a call.
    pub retries: u64,
    /// Calls that exhausted every attempt.
    pub timeouts: u64,
    /// `LOOKUP` calls issued, spelling retries included.
    pub lookups: u64,
    /// `READ` calls issued.
    pub reads: u64,
    /// Payload bytes received by `READ`.
    pub bytes: u64,
}

/// An ONC RPC / NFSv2 client pointed at one player.
///
/// One call is outstanding at a time; see the module documentation for why
/// that is not the throughput decision it looks like.
#[derive(Debug)]
pub struct NfsClient {
    socket: UdpSocket,
    peer: Ipv4Addr,
    ports: RpcPorts,
    config: NfsConfig,
    buffer: Vec<u8>,
    /// Directory handles already resolved, keyed by export and path. See the
    /// module documentation: without this a player's handle table churns and
    /// starts answering `NFSERR_STALE` to everything.
    directories: HashMap<(String, String), FileHandle>,
    /// The next xid. A deck's starts at 1 at boot and counts up from a single
    /// global counter shared across all three programs; ours does the same, so
    /// that [`rpc::stamp_for_xid`] can give the credential stamp a freshly
    /// booted player would have sent.
    xid: u32,
    stats: NfsStats,
}

/// The first announced piece of the head.
///
/// Eight NFS reads at the 8 KB a CDJ serves, which is about as small as is
/// worth announcing: below this the round trips dominate and above it a decoder
/// waits on bytes it does not need yet.
pub const LEAD: u64 = 64 * 1024;

/// One range of a file, to be fetched as a unit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FetchStep {
    /// Offset into the file.
    pub offset: u64,
    /// How many bytes. Never zero, and never past the end.
    pub len: u64,
}

/// Order the ranges of a progressive fetch: **head, then tail, then middle**.
///
/// A track that has to be downloaded before it can be played takes as long to
/// load as it takes to download. Fetching it in this order lets playback start
/// once the head is down, with the rest arriving behind the playhead.
///
/// The order is not arbitrary and neither part of it is optional:
///
/// * **Head first** because that is the runway. Nothing can start without it.
/// * **Tail second** because of the containers that keep their index at the
///   end. MP3 decodes happily from the front, but M4A and MP4 commonly put the
///   `moov` atom last, and a decoder cannot open one *at all* without it — so
///   deferring the tail to its natural position would mean AAC never plays.
/// * **Middle last**, in order, because that is the direction the playhead
///   travels.
///
/// Ranges never overlap and always cover the file exactly, so a caller can
/// treat completion as "every step done" without reasoning about gaps.
///
/// The head's first step is capped at [`LEAD`], so that a decoder waiting to
/// open the file waits for that rather than for the whole head.
#[must_use]
pub fn progressive_plan(size: u64, head_bytes: u64, tail_bytes: u64, chunk: u64) -> Vec<FetchStep> {
    if size == 0 {
        return Vec::new();
    }
    // A zero chunk would loop forever; treat it as "the rest in one go".
    let chunk = if chunk == 0 { u64::MAX } else { chunk };

    let mut steps = Vec::new();
    let head = head_bytes.min(size);
    if head > 0 {
        // The head is split so that its *first* piece is small.
        //
        // A step is only announced once all of it has landed, and a decoder
        // opening the file blocks on the very first bytes -- so a head fetched
        // as one megabyte makes the decoder wait for the whole megabyte before
        // it can read the first 64 KB of it. Measured on a deck: 432 ms of a
        // 480 ms load, spent waiting for bytes that had mostly already arrived.
        //
        // The runway is unchanged; only the reporting granularity is. Splitting
        // the tail and middle the same way would buy nothing, because nothing
        // is waiting on those.
        let mut offset = 0;
        while offset < head {
            let len = if offset == 0 {
                LEAD.min(head)
            } else {
                head - offset
            };
            steps.push(FetchStep { offset, len });
            offset += len;
        }
    }

    // The tail starts no earlier than the end of the head, so a small file whose
    // head and tail would overlap yields one range rather than two that repeat
    // the same bytes.
    let tail_start = size.saturating_sub(tail_bytes).max(head);
    if tail_start < size {
        steps.push(FetchStep {
            offset: tail_start,
            len: size - tail_start,
        });
    }

    let mut offset = head;
    while offset < tail_start {
        let len = chunk.min(tail_start - offset);
        steps.push(FetchStep { offset, len });
        offset += len;
    }
    steps
}

impl NfsClient {
    /// Discover `peer`'s RPC ports and be ready to mount.
    ///
    /// Transmits two `GETPORT`s and nothing else. Announcing is not required
    /// (F11) — except against a device that scopes its export per host, where
    /// `MNT` answers [`ErrorStatus::ACCES`] until it is.
    pub async fn connect(peer: Ipv4Addr, interface: Option<&Interface>) -> Result<Self> {
        Self::connect_with(peer, interface, NfsConfig::default()).await
    }

    /// As [`NfsClient::connect`], with an explicit configuration.
    pub async fn connect_with(
        peer: Ipv4Addr,
        interface: Option<&Interface>,
        config: NfsConfig,
    ) -> Result<Self> {
        // Bind our own address on the CDJ-facing interface: a socket bound to
        // 0.0.0.0 on a multi-homed host can send from the wrong NIC, and on a
        // link-local segment there is no route to put that right afterwards.
        let local = interface.map_or(Ipv4Addr::UNSPECIFIED, |interface| interface.ip);
        let socket = socket::bind_at(local, 0, interface)?;
        let mut client = Self {
            socket,
            peer,
            ports: RpcPorts {
                mount: mount::PIONEER_PORT,
                nfs: nfs2::PORT,
            },
            config,
            buffer: vec![0u8; MAX_DATAGRAM],
            directories: HashMap::new(),
            xid: 1,
            stats: NfsStats::default(),
        };
        client.ports = client.discover_ports().await?;
        debug!(%peer, ports = ?client.ports, "RPC ports discovered");
        Ok(client)
    }

    /// The player this client talks to.
    pub fn peer(&self) -> Ipv4Addr {
        self.peer
    }

    /// The ports discovered at connect time.
    pub fn ports(&self) -> RpcPorts {
        self.ports
    }

    /// The configuration in force.
    pub fn config(&self) -> &NfsConfig {
        &self.config
    }

    /// Counters since connect.
    pub fn stats(&self) -> NfsStats {
        self.stats
    }

    /// Change the read size mid-session.
    ///
    /// Every read is independent, so this takes effect on the next one and
    /// nothing has to be restarted.
    pub fn set_read_size(&mut self, read_size: ReadSize) {
        self.config.read_size = read_size;
    }

    /// Drop every cached directory handle.
    ///
    /// The right response to [`ErrorStatus::STALE`], and the thing
    /// [`NfsClient::refresh`] does before re-mounting.
    pub fn forget_directories(&mut self) {
        self.directories.clear();
    }

    /// Re-`MNT` an export and forget every handle derived from the old root.
    ///
    /// A peer answering [`ErrorStatus::STALE`] means the handles it gave us no
    /// longer refer to anything — the medium was swapped, or its table churned.
    /// Nothing below the root survives that, which is why this takes the mount
    /// by value and hands back a new one rather than patching the old.
    pub async fn refresh(&mut self, mounted: Mount) -> Result<Mount> {
        self.forget_directories();
        self.mount(&mounted.export).await
    }

    // -- discovery --------------------------------------------------------

    async fn discover_ports(&mut self) -> Result<RpcPorts> {
        // mountd first, then nfsd: the order 91 of 91 deck-originated GETPORT
        // calls in the corpus use.
        let mount = self.getport(Program::MOUNT, mount::VERSION).await?;
        let nfs = self.getport(Program::NFS, nfs2::VERSION).await?;
        Ok(RpcPorts { mount, nfs })
    }

    async fn getport(&mut self, program: Program, version: u32) -> Result<u16> {
        let arguments =
            portmap::Request::GetPort(portmap::Mapping::query(program, version, IpProtocol::UDP))
                .encode_arguments();
        let port = self
            .call(
                self.config.portmap_port,
                Program::PORTMAP,
                portmap::VERSION,
                portmap::Proc::GETPORT.0,
                "a portmap GETPORT",
                &arguments,
                |results| match portmap::Response::parse(portmap::Proc::GETPORT, results)? {
                    portmap::Response::GetPort(port) => Ok(port),
                    other => Err(unexpected("a portmap GETPORT", &format!("{other:?}"))),
                },
            )
            .await?;
        match port {
            Some(port) => Ok(port),
            // Zero is a successful reply meaning "not registered". A DUMP tells
            // "this host runs no RPC" from "it runs RPC and does not export
            // this", and those lead to different conclusions.
            None => Err(self.explain_missing_program(program).await),
        }
    }

    async fn explain_missing_program(&mut self, program: Program) -> Error {
        let detail = match self.dump().await {
            Ok(mappings) if mappings.is_empty() => {
                "the portmapper answered and has nothing registered".to_owned()
            }
            Ok(mappings) => {
                let names: Vec<String> = mappings
                    .iter()
                    .map(|mapping| format!("{:?} v{}", mapping.program, mapping.version))
                    .collect();
                format!("the portmapper registers only {}", names.join(", "))
            }
            Err(error) => format!("and a DUMP did not answer either ({error})"),
        };
        Error::Refused {
            what: "a portmap GETPORT",
            detail: format!("{program:?} is not registered: {detail}"),
        }
    }

    /// Everything `peer`'s portmapper has registered.
    ///
    /// A real deck never calls this — it asks for the two programs it wants and
    /// nothing else — but it is the only way to tell an empty registration
    /// table from a host with no RPC stack at all.
    pub async fn dump(&mut self) -> Result<Vec<portmap::Mapping>> {
        let arguments = portmap::Request::Dump.encode_arguments();
        self.call(
            self.config.portmap_port,
            Program::PORTMAP,
            portmap::VERSION,
            portmap::Proc::DUMP.0,
            "a portmap DUMP",
            &arguments,
            |results| match portmap::Response::parse(portmap::Proc::DUMP, results)? {
                portmap::Response::Dump(mappings) => Ok(mappings),
                other => Err(unexpected("a portmap DUMP", &format!("{other:?}"))),
            },
        )
        .await
    }

    // -- MOUNT ------------------------------------------------------------

    /// Everything the peer offers, with the access list for each.
    ///
    /// **A real deck never calls this** (F37) — it goes straight to `MNT` with
    /// the documented path. Enumerating is still the more robust client
    /// behaviour, because it is what survives C6's `/C/EXPORT` and because the
    /// group list names the hosts permitted to mount, which is the diagnosis
    /// for a [`ErrorStatus::ACCES`]. A peer that answers `PROC_UNAVAIL` costs one
    /// datagram to find out about.
    pub async fn exports(&mut self) -> Result<Vec<mount::Export>> {
        let arguments = mount::Request::Export.encode_arguments();
        self.call(
            self.ports.mount,
            Program::MOUNT,
            mount::VERSION,
            mount::Proc::EXPORT.0,
            "a MOUNT EXPORT",
            &arguments,
            |results| match mount::Response::parse(mount::Proc::EXPORT, results)? {
                mount::Response::Export(exports) => Ok(exports),
                other => Err(unexpected("a MOUNT EXPORT", &format!("{other:?}"))),
            },
        )
        .await
    }

    /// Mount one export by its exact path.
    pub async fn mount(&mut self, export: &str) -> Result<Mount> {
        let arguments = mount::Request::Mnt(Utf16LeString::new(export)).encode_arguments();
        let handle = self
            .call(
                self.ports.mount,
                Program::MOUNT,
                mount::VERSION,
                mount::Proc::MNT.0,
                "a MOUNT MNT",
                &arguments,
                |results| match mount::Response::parse(mount::Proc::MNT, results)? {
                    mount::Response::Mnt(result) => Ok(result),
                    other => Err(unexpected("a MOUNT MNT", &format!("{other:?}"))),
                },
            )
            .await?
            .map_err(|status| Error::Nfs {
                operation: "MNT",
                path: export.to_owned(),
                status,
            })?;
        debug!(export, ?handle, "mounted");
        Ok(Mount {
            export: export.to_owned(),
            slot: mount::slot_for_export(export),
            root: handle,
        })
    }

    /// Mount whichever export is this slot's, enumerating first.
    ///
    /// `EXPORT` names the path the device itself uses, which is the only way to
    /// get `/C/EXPORT` right (C6); when it is refused or lists nothing for this
    /// slot, the documented `/B/` or `/C/` is tried anyway, because that is
    /// what a real deck sends and it works on every device we have.
    ///
    /// `Err(`[`Error::Nfs`]`)` carrying [`ErrorStatus::ACCES`] means the peer scopes
    /// its export per host: announce with [`crate::VirtualCdj`] and try again
    /// (F12).
    pub async fn mount_slot(&mut self, slot: Slot) -> Result<Mount> {
        let mut candidates: Vec<String> = match self.exports().await {
            Ok(exports) => exports
                .iter()
                .map(|export| export.path.to_string_lossy())
                .filter(|path| mount::slot_for_export(path) == Some(slot))
                .collect(),
            Err(error) => {
                // A deck that refuses EXPORT is normal, not broken.
                trace!(%error, ?slot, "EXPORT unavailable; using the documented path");
                Vec::new()
            }
        };
        if let Some(documented) = mount::export_path_for(slot)
            && !candidates.iter().any(|path| path == documented)
        {
            candidates.push(documented.to_owned());
        }

        let mut last = None;
        for candidate in &candidates {
            match self.mount(candidate).await {
                Ok(mounted) => return Ok(mounted),
                Err(error) => {
                    debug!(export = candidate, %error, "MNT refused");
                    last = Some(error);
                }
            }
        }
        Err(last.unwrap_or_else(|| Error::Refused {
            what: "a MOUNT MNT",
            detail: format!("{slot:?} has no export path to try"),
        }))
    }

    /// Release a mount.
    ///
    /// Real players do send this, once per slot, after a physical eject (C9),
    /// so it is not decoration. `UMNT` returns no results at all, and a peer
    /// that has forgotten the mount answers just the same.
    pub async fn unmount(&mut self, mounted: &Mount) -> Result<()> {
        let arguments =
            mount::Request::Umnt(Utf16LeString::new(&mounted.export)).encode_arguments();
        self.directories
            .retain(|(export, _), _| *export != mounted.export);
        self.call(
            self.ports.mount,
            Program::MOUNT,
            mount::VERSION,
            mount::Proc::UMNT.0,
            "a MOUNT UMNT",
            &arguments,
            |_results| Ok(()),
        )
        .await
    }

    // -- NFS --------------------------------------------------------------

    /// Attributes of whatever a handle names.
    pub async fn attributes(&mut self, handle: FileHandle) -> Result<Fattr> {
        let arguments = nfs2::Request::GetAttr(handle).encode_arguments();
        self.call(
            self.ports.nfs,
            Program::NFS,
            nfs2::VERSION,
            nfs2::Proc::GETATTR.0,
            "an NFS GETATTR",
            &arguments,
            |results| match nfs2::Response::parse(nfs2::Proc::GETATTR, results)? {
                nfs2::Response::Attr(result) => Ok(result),
                other => Err(unexpected("an NFS GETATTR", &format!("{other:?}"))),
            },
        )
        .await?
        .map_err(|status| Error::Nfs {
            operation: "GETATTR",
            path: format!("{handle:?}"),
            status,
        })
    }

    /// Resolve one path component inside one directory.
    ///
    /// Returns the peer's own answer, `NFSERR_NOENT` included, because "that
    /// name is spelled differently here" is a case the caller may want to
    /// handle rather than an error — see [`NfsClient::walk`].
    pub async fn lookup(
        &mut self,
        directory: FileHandle,
        name: &str,
    ) -> Result<nfs2::NfsResult<nfs2::FileRef>> {
        let arguments = nfs2::Request::Lookup {
            dir: directory,
            name: Utf16LeString::new(name),
        }
        .encode_arguments();
        self.stats.lookups += 1;
        self.call(
            self.ports.nfs,
            Program::NFS,
            nfs2::VERSION,
            nfs2::Proc::LOOKUP.0,
            "an NFS LOOKUP",
            &arguments,
            |results| match nfs2::Response::parse(nfs2::Proc::LOOKUP, results)? {
                nfs2::Response::Lookup(result) => Ok(result),
                other => Err(unexpected("an NFS LOOKUP", &format!("{other:?}"))),
            },
        )
        .await
    }

    /// Walk a whole path from a mount root, one `LOOKUP` per component.
    ///
    /// NFS has no multi-component path, so a cold walk is `n` round trips for
    /// `n` components. Every directory it passes through is remembered, so the
    /// second file in the same folder costs one `LOOKUP` rather than four —
    /// which is both faster and the only way to stay inside a player's handle
    /// table. When the cached parent has gone stale the leaf lookup fails, the
    /// entry is dropped, and the path is walked again from the root **once**,
    /// so a file that is genuinely missing fails rather than looping.
    ///
    /// A component that comes back `NFSERR_NOENT` is retried through the
    /// spellings a rekordbox medium and its own database are known to disagree
    /// on — case, and Unicode normalisation (O6). The error, when it comes,
    /// carries the failing component and the status the peer sent.
    pub async fn walk(&mut self, mounted: &Mount, path: &str) -> Result<(FileHandle, Fattr)> {
        match self.walk_from_cache(mounted, path).await {
            Some(Ok(found)) => return Ok(found),
            Some(Err(error)) if !is_missing(&error) => return Err(error),
            // A cached parent that no longer resolves is exactly what a churned
            // handle table looks like. Forget it and walk from the root.
            Some(Err(_)) | None => {}
        }
        self.walk_from_root(mounted, path).await
    }

    /// One `LOOKUP` from the deepest cached ancestor, or `None` if there is
    /// none.
    async fn walk_from_cache(
        &mut self,
        mounted: &Mount,
        path: &str,
    ) -> Option<Result<(FileHandle, Fattr)>> {
        let (parent, leaf) = split_parent(path)?;
        let key = (mounted.export.clone(), parent.clone());
        let directory = *self.directories.get(&key)?;
        match self.lookup_spellings(directory, &leaf, &parent).await {
            Ok(found) => Some(Ok((found.handle, found.attr))),
            Err(error) => {
                debug!(path, %error, "a cached directory handle no longer resolves");
                self.directories.remove(&key);
                Some(Err(error))
            }
        }
    }

    async fn walk_from_root(&mut self, mounted: &Mount, path: &str) -> Result<(FileHandle, Fattr)> {
        let mut handle = mounted.root;
        let mut attr = None;
        let mut walked = String::new();
        for component in path.split('/').filter(|part| !part.is_empty()) {
            let found = self.lookup_spellings(handle, component, &walked).await?;
            walked.push('/');
            walked.push_str(component);
            handle = found.handle;
            attr = Some(found.attr);
            if found.attr.is_directory() {
                self.directories
                    .insert((mounted.export.clone(), walked.clone()), handle);
            }
        }
        match attr {
            Some(attr) => Ok((handle, attr)),
            // The path named the mount root itself, which has no LOOKUP to
            // have produced attributes.
            None => Ok((handle, self.attributes(handle).await?)),
        }
    }

    async fn lookup_spellings(
        &mut self,
        directory: FileHandle,
        component: &str,
        walked: &str,
    ) -> Result<nfs2::FileRef> {
        let mut status = ErrorStatus::NOENT;
        for candidate in spellings(component) {
            match self.lookup(directory, &candidate).await? {
                Ok(found) => {
                    if candidate != component {
                        debug!(
                            asked = component,
                            found = %candidate,
                            "the medium spells this component differently"
                        );
                    }
                    return Ok(found);
                }
                Err(reported) => {
                    status = reported;
                    // Only a missing name is worth another spelling. NOTDIR,
                    // ACCES and STALE each say something else entirely.
                    if reported != ErrorStatus::NOENT {
                        break;
                    }
                }
            }
        }
        Err(Error::Nfs {
            operation: "LOOKUP",
            path: format!("{walked}/{component}"),
            status,
        })
    }

    /// Walk a path and prove it names a regular file.
    ///
    /// The size comes from here and nowhere else: a `READ` reply from a real
    /// CDJ has an all-zero `fattr`, so this is the only place a transfer can
    /// learn where the file ends.
    pub async fn open(&mut self, mounted: &Mount, path: &str) -> Result<RemoteFile> {
        let (handle, attr) = self.walk(mounted, path).await?;
        if !attr.is_regular_file() {
            return Err(Error::Nfs {
                operation: "LOOKUP",
                path: path.to_owned(),
                status: if attr.is_directory() {
                    ErrorStatus::ISDIR
                } else {
                    ErrorStatus::new(Status::NXIO).unwrap_or(ErrorStatus::IO)
                },
            });
        }
        Ok(RemoteFile {
            path: path.to_owned(),
            handle,
            size: u64::from(attr.size),
        })
    }

    /// Read one byte range, in one `READ`.
    ///
    /// `count` is capped at [`NfsConfig::read_size`]. The result may be shorter
    /// than asked for even in the middle of a file — a server is entitled to
    /// answer short — so a caller doing its own loop must treat a short read as
    /// "ask again from further on", never as end of file.
    pub async fn read_at(&mut self, file: &RemoteFile, offset: u64, count: u32) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.read_once(file, offset, count, &mut out).await?;
        Ok(out)
    }

    /// Read `len` bytes from `offset`, looping until they arrive.
    ///
    /// Clamped to the file's length, so a range past the end comes back short
    /// rather than never finishing. This is the streaming entry point: a deck
    /// touches about 38% of a track during a load plus thirty seconds of play
    /// (F18), so pulling whole tracks is the exception rather than the rule.
    pub async fn read_range(
        &mut self,
        file: &RemoteFile,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>> {
        let end = offset.saturating_add(len).min(file.size);
        let mut out = Vec::with_capacity(usize::try_from(end.saturating_sub(offset)).unwrap_or(0));
        let mut position = offset;
        while position < end {
            let want = self.chunk_size(end - position);
            let read = self.read_once(file, position, want, &mut out).await?;
            if read == 0 {
                return Err(no_progress(file, position));
            }
            position = position.saturating_add(u64::try_from(read).unwrap_or(0));
        }
        Ok(out)
    }

    /// Pull a whole file.
    ///
    /// `export.pdb` is about 1 MB and a track up to 75 MB; NFSv2 cannot address
    /// past 4 GiB at all, and [`RemoteFile`] could not have been built for a
    /// file that large.
    pub async fn read_file(&mut self, file: &RemoteFile) -> Result<Vec<u8>> {
        self.read_file_with(file, |_| {}).await
    }

    /// Pull a whole file, reporting progress after every `READ`.
    ///
    /// The callback is invoked once per reply — 128 times for a 1 MB database
    /// at [`ReadSize::CDJ`], 842 at [`ReadSize::UNFRAGMENTED`] — and it runs on
    /// the transfer's own task, so keep it cheap. `read` is contiguous bytes
    /// from the start of the file, never a running total that could exceed
    /// what is safely assembled.
    ///
    /// A partial file is never returned. A truncated `export.pdb` parses far
    /// enough to look plausible and then yields a library missing its last few
    /// hundred tracks.
    pub async fn read_file_with(
        &mut self,
        file: &RemoteFile,
        mut progress: impl FnMut(Progress),
    ) -> Result<Vec<u8>> {
        let capacity = usize::try_from(file.size).unwrap_or(0);
        let mut out = Vec::with_capacity(capacity);
        progress(Progress {
            read: 0,
            total: file.size,
        });
        while out.len() < capacity {
            let position = u64::try_from(out.len()).unwrap_or(u64::MAX);
            let want = self.chunk_size(file.size - position);
            let read = self.read_once(file, position, want, &mut out).await?;
            if read == 0 {
                return Err(no_progress(file, position));
            }
            progress(Progress {
                read: u64::try_from(out.len()).unwrap_or(u64::MAX),
                total: file.size,
            });
        }
        Ok(out)
    }

    fn chunk_size(&self, remaining: u64) -> u32 {
        let want = u32::try_from(remaining).unwrap_or(u32::MAX);
        want.min(self.config.read_size.get())
    }

    /// One `READ`, appending its payload to `out` and returning its length.
    async fn read_once(
        &mut self,
        file: &RemoteFile,
        offset: u64,
        count: u32,
        out: &mut Vec<u8>,
    ) -> Result<usize> {
        let count = count.min(self.config.read_size.get());
        let arguments =
            nfs2::Request::Read(nfs2::ReadArgs::at(file.handle, offset, count)?).encode_arguments();
        self.stats.reads += 1;
        let read = self
            .call(
                self.ports.nfs,
                Program::NFS,
                nfs2::VERSION,
                nfs2::Proc::READ.0,
                "an NFS READ",
                &arguments,
                |results| match nfs2::Response::parse(nfs2::Proc::READ, results)? {
                    // Nothing is read out of `data.attr`: a real CDJ sends it
                    // all-zero apart from `fileid`, so `size == 0` there does
                    // not mean the file is empty.
                    nfs2::Response::Read(Ok(data)) => {
                        out.extend_from_slice(data.data);
                        Ok(Ok(data.data.len()))
                    }
                    nfs2::Response::Read(Err(status)) => Ok(Err(status)),
                    other => Err(unexpected("an NFS READ", &format!("{other:?}"))),
                },
            )
            .await?
            .map_err(|status| Error::Nfs {
                operation: "READ",
                path: file.path.clone(),
                status,
            })?;
        self.stats.bytes += u64::try_from(read).unwrap_or(0);
        Ok(read)
    }

    // -- the RPC exchange -------------------------------------------------

    /// Send one call, retransmitting on silence, and decode its results.
    async fn call<T>(
        &mut self,
        port: u16,
        program: Program,
        version: u32,
        procedure: u32,
        what: &'static str,
        arguments: &[u8],
        decode: impl FnOnce(&[u8]) -> Result<T>,
    ) -> Result<T> {
        let xid = Xid(self.xid);
        // Wrapping past the top lands on 1, not 0: a deck's counter starts at
        // one and `stamp_for_xid` has no entry for a zeroth call.
        self.xid = self.xid.checked_add(1).unwrap_or(1);
        // A player's credential stamp is a fixed sequence indexed by the number
        // of RPC calls it has made since power-on, identical across devices and
        // across a decade of firmware. Walking the same table makes us look
        // like a deck that has just been switched on; past the observed forty,
        // any value will do and nothing validates it.
        let credential =
            AuthUnix::cdj(rpc::stamp_for_xid(xid).unwrap_or(rpc::STAMP_FIRST_CALL)).encode();
        let datagram = Call::new(
            xid,
            program,
            version,
            procedure,
            Auth::unix(&credential),
            arguments,
        )
        .encode();

        let len = self.exchange(port, &datagram, xid, what).await?;
        let raw = self.buffer.get(..len).unwrap_or_default();
        decode(accepted_results(raw, what)?)
    }

    /// Transmit and wait, retransmitting the **same** xid on each attempt.
    ///
    /// Returns how many bytes of [`Self::buffer`] the reply occupies.
    async fn exchange(
        &mut self,
        port: u16,
        datagram: &[u8],
        xid: Xid,
        what: &'static str,
    ) -> Result<usize> {
        let destination = SocketAddr::V4(SocketAddrV4::new(self.peer, port));
        for attempt in 0..self.config.attempts.get() {
            if attempt > 0 {
                self.stats.retries += 1;
                trace!(?xid, what, attempt, "retransmitting");
            }
            self.stats.datagrams += 1;
            self.socket
                .send_to(datagram, destination)
                .await
                .map_err(Error::io("sending an RPC call"))?;

            if let Some(len) = self.wait_for(xid, what).await? {
                return Ok(len);
            }
        }
        self.stats.timeouts += 1;
        warn!(?xid, what, peer = %self.peer, "no reply after every attempt");
        Err(Error::Timeout {
            what,
            after: self
                .config
                .timeout
                .saturating_mul(u32::from(self.config.attempts.get())),
        })
    }

    /// Wait one timeout for the reply to `xid`, discarding anything else.
    ///
    /// `None` means the timeout expired. A datagram from another address, or
    /// carrying another xid, is dropped and the wait continues on what is left
    /// of the budget: it answers a call we have already given up on, and taking
    /// it for this call's reply would decode one procedure's results as
    /// another's.
    async fn wait_for(&mut self, xid: Xid, what: &'static str) -> Result<Option<usize>> {
        let deadline = tokio::time::Instant::now() + self.config.timeout;
        loop {
            let receive = self.socket.recv_from(&mut self.buffer);
            let (len, from) = match tokio::time::timeout_at(deadline, receive).await {
                Err(_elapsed) => return Ok(None),
                Ok(Ok(received)) => received,
                Ok(Err(error)) => return Err(Error::io("receiving an RPC reply")(error)),
            };
            let SocketAddr::V4(from) = from else { continue };
            if *from.ip() != self.peer {
                trace!(%from, what, "a datagram from somebody else");
                continue;
            }
            let raw = self.buffer.get(..len).unwrap_or_default();
            match Reply::parse(raw) {
                Ok(reply) if reply.xid() == xid => return Ok(Some(len)),
                Ok(reply) => trace!(?xid, got = ?reply.xid(), what, "a reply we no longer want"),
                Err(error) => trace!(%error, what, "an undecodable reply"),
            }
        }
    }
}

/// The results of an accepted, successful reply.
///
/// Everything else — denied, `PROC_UNAVAIL`, a version mismatch — is a peer
/// that understood the call and would not run it, which is a different thing
/// from bytes that did not decode.
fn accepted_results<'a>(raw: &'a [u8], what: &'static str) -> Result<&'a [u8]> {
    match Reply::parse(raw)? {
        Reply::Accepted {
            status: Accepted::Success(results),
            ..
        } => Ok(results),
        Reply::Accepted {
            status: Accepted::ProgMismatch { low, high },
            ..
        } => Err(Error::Refused {
            what,
            detail: format!("the peer serves only versions {low}-{high} of that program"),
        }),
        Reply::Accepted {
            status: Accepted::Failed(stat),
            ..
        } => Err(Error::Refused {
            what,
            detail: format!("the peer answered {stat:?}"),
        }),
        Reply::Denied { reason, .. } => Err(Error::Refused {
            what,
            detail: match reason {
                Denied::RpcMismatch { low, high } => {
                    format!("the peer speaks only RPC {low}-{high}")
                }
                Denied::AuthError(stat) => format!("the credential was refused: {stat:?}"),
                Denied::Other(stat) => format!("the call was rejected: {stat:?}"),
            },
        }),
    }
}

fn unexpected(what: &'static str, got: &str) -> Error {
    Error::Refused {
        what,
        detail: format!("the reply decoded as {got}"),
    }
}

fn no_progress(file: &RemoteFile, offset: u64) -> Error {
    Error::Refused {
        what: "an NFS READ",
        detail: format!(
            "{path} returned no bytes at offset {offset} of {size}; stopping here \
             would truncate the file silently",
            path = file.path,
            size = file.size
        ),
    }
}

/// Whether an error is "that name resolved to nothing", the only kind a second
/// walk could fix.
fn is_missing(error: &Error) -> bool {
    match error {
        Error::Nfs { status, .. } => matches!(
            status.status(),
            Status::NOENT | Status::STALE | Status::NOTDIR
        ),
        _ => false,
    }
}

/// A path's parent directory and its last component, or `None` for a path with
/// no components at all.
fn split_parent(path: &str) -> Option<(String, String)> {
    let mut components: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let leaf = components.pop()?;
    let mut parent = String::new();
    for component in components {
        parent.push('/');
        parent.push_str(component);
    }
    Some((parent, leaf.to_owned()))
}

/// The spellings of one path component worth trying, most likely first.
///
/// `export.pdb` records `Gesaffelstein` where the FAT32 directory entry is
/// `GESAFFELSTEIN`, and it stores NFC where the filesystem reports NFD, because
/// rekordbox wrote the two through different APIs. A client walking a path out
/// of the database therefore asks for a name that, byte for byte, is not the
/// one on the medium, and gets `NFSERR_NOENT` with nothing else to go on (O6).
///
/// The exact spelling is always first and is what almost every lookup takes;
/// the rest cost a datagram each and only on a miss. Duplicates are dropped, so
/// an ASCII name that is already uppercase yields two candidates, not seven.
fn spellings(component: &str) -> Vec<String> {
    let nfc: String = component.nfc().collect();
    let nfd: String = component.nfd().collect();
    let mut seen = BTreeSet::new();
    [
        component.to_owned(),
        nfc.clone(),
        nfd.clone(),
        nfc.to_uppercase(),
        nfd.to_uppercase(),
        nfc.to_lowercase(),
        nfd.to_lowercase(),
    ]
    .into_iter()
    .filter(|candidate| seen.insert(candidate.clone()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use prolink_proto::rpc::nfs2::{FType, FileRef};

    // -- a loopback deck --------------------------------------------------
    //
    // There are no CDJs on this machine, so the client is driven against a
    // server built out of the codec crate's own reply builders: three UDP
    // sockets on ephemeral ports, one per RPC program, so that port discovery
    // is genuinely exercised rather than assumed.

    #[derive(Clone, Debug, Default)]
    struct Tree {
        /// path -> children, in listing order.
        directories: HashMap<String, Vec<String>>,
        /// path -> contents.
        files: HashMap<String, Vec<u8>>,
    }

    impl Tree {
        fn with_file(mut self, path: &str, data: &[u8]) -> Self {
            let components: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
            let mut walked = String::new();
            for (index, component) in components.iter().enumerate() {
                let parent = if walked.is_empty() {
                    "/".to_owned()
                } else {
                    walked.clone()
                };
                let child = if parent == "/" {
                    format!("/{component}")
                } else {
                    format!("{parent}/{component}")
                };
                let children = self.directories.entry(parent).or_default();
                if !children.iter().any(|name| name == component) {
                    children.push((*component).to_owned());
                }
                if index + 1 == components.len() {
                    self.files.insert(child.clone(), data.to_vec());
                } else {
                    self.directories.entry(child.clone()).or_default();
                }
                walked = child;
            }
            self.directories.entry("/".to_owned()).or_default();
            self
        }

        /// A handle fills all 32 bytes, so a client that truncated one to the
        /// twelve a *server* may rely on would not be recognised here — which
        /// is the point.
        fn handle_for(path: &str) -> FileHandle {
            let digest = path.bytes().fold(0x811c_9dc5u32, |hash, byte| {
                (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
            });
            let mut bytes = [0u8; FileHandle::LEN];
            for (index, slot) in bytes.iter_mut().enumerate() {
                let word = digest.wrapping_add(u32::try_from(index).unwrap_or(0));
                *slot = u8::try_from(word & 0xff).unwrap_or(0);
            }
            FileHandle(bytes)
        }

        fn path_of(&self, handle: FileHandle) -> Option<String> {
            self.directories
                .keys()
                .chain(self.files.keys())
                .find(|path| Self::handle_for(path) == handle)
                .cloned()
        }

        fn attributes(&self, path: &str) -> Option<Fattr> {
            let handle = Self::handle_for(path);
            if self.directories.contains_key(path) {
                return Some(Fattr::directory(handle.fileid(), Fattr::EPOCH));
            }
            let data = self.files.get(path)?;
            Fattr::regular_file(
                handle.fileid(),
                u64::try_from(data.len()).unwrap_or(0),
                Fattr::EPOCH,
            )
            .ok()
        }
    }

    #[derive(Clone, Debug, Default)]
    struct Behaviour {
        /// Drop every nth datagram, to force a retransmission.
        drop_every: u64,
        /// Refuse MNT with this status.
        refuse_mount: Option<ErrorStatus>,
        /// Answer EXPORT with these paths, or PROC_UNAVAIL when empty.
        exports: Vec<String>,
        /// Answer at most this many bytes per READ, to force a short read.
        short_read: Option<u32>,
        /// Do not answer at all.
        deaf: bool,
    }

    #[derive(Debug)]
    struct Deck {
        portmap_port: u16,
        datagrams: Arc<AtomicU64>,
        echoed_verbatim: Arc<AtomicU64>,
        mangled: Arc<AtomicU64>,
        tasks: Vec<tokio::task::JoinHandle<()>>,
    }

    impl Drop for Deck {
        fn drop(&mut self) {
            for task in &self.tasks {
                task.abort();
            }
        }
    }

    fn bind_local() -> UdpSocket {
        socket::bind_at(Ipv4Addr::LOCALHOST, 0, None).expect("an ephemeral loopback socket")
    }

    fn port_of(socket: &UdpSocket) -> u16 {
        match socket.local_addr().expect("a bound socket has an address") {
            SocketAddr::V4(address) => address.port(),
            SocketAddr::V6(_) => unreachable!("bound as IPv4"),
        }
    }

    fn deck(tree: &Tree, behaviour: &Behaviour) -> Deck {
        let portmap_socket = bind_local();
        let mount_socket = bind_local();
        let nfs_socket = bind_local();
        let portmap_port = port_of(&portmap_socket);
        let mount_port = port_of(&mount_socket);
        let nfs_port = port_of(&nfs_socket);

        let datagrams = Arc::new(AtomicU64::new(0));
        let verbatim = Arc::new(AtomicU64::new(0));
        let mangled = Arc::new(AtomicU64::new(0));

        let mut tasks = Vec::new();
        for (socket, program) in [
            (portmap_socket, Program::PORTMAP),
            (mount_socket, Program::MOUNT),
            (nfs_socket, Program::NFS),
        ] {
            let tree = tree.clone();
            let behaviour = behaviour.clone();
            let datagrams = Arc::clone(&datagrams);
            let verbatim = Arc::clone(&verbatim);
            let mangled = Arc::clone(&mangled);
            tasks.push(tokio::spawn(async move {
                let mut buffer = vec![0u8; MAX_DATAGRAM];
                loop {
                    let Ok((len, from)) = socket.recv_from(&mut buffer).await else {
                        return;
                    };
                    let seen = datagrams.fetch_add(1, Ordering::SeqCst) + 1;
                    if behaviour.deaf
                        || (behaviour.drop_every > 0 && seen.is_multiple_of(behaviour.drop_every))
                    {
                        continue;
                    }
                    let Ok(call) = Call::parse(&buffer[..len]) else {
                        continue;
                    };
                    assert_eq!(call.program, program, "a call reached the wrong port");
                    let reply = match answer(
                        &call,
                        &tree,
                        &behaviour,
                        (mount_port, nfs_port),
                        &verbatim,
                        &mangled,
                    ) {
                        Some(results) => Reply::success(call.xid, &results).encode(),
                        // What a real CDJ answers a procedure it does not run.
                        None => Reply::failed(call.xid, rpc::FailureStat::PROC_UNAVAIL).encode(),
                    };
                    let _ = socket.send_to(&reply, from).await;
                }
            }));
        }

        Deck {
            portmap_port,
            datagrams,
            echoed_verbatim: verbatim,
            mangled,
            tasks,
        }
    }

    fn answer(
        call: &Call<'_>,
        tree: &Tree,
        behaviour: &Behaviour,
        ports: (u16, u16),
        verbatim: &AtomicU64,
        mangled: &AtomicU64,
    ) -> Option<Vec<u8>> {
        match call.program {
            Program::PORTMAP => answer_portmap(call, ports),
            Program::MOUNT => answer_mount(call, behaviour),
            Program::NFS => answer_nfs(call, tree, behaviour, verbatim, mangled),
            _ => None,
        }
    }

    fn answer_portmap(call: &Call<'_>, ports: (u16, u16)) -> Option<Vec<u8>> {
        let request =
            portmap::Request::parse(portmap::Proc(call.procedure), call.arguments).ok()?;
        Some(match request {
            portmap::Request::GetPort(mapping) => {
                let port = match mapping.program {
                    Program::MOUNT => Some(ports.0),
                    Program::NFS => Some(ports.1),
                    _ => None,
                };
                portmap::Response::GetPort(port).encode()
            }
            portmap::Request::Dump => {
                portmap::Response::Dump(portmap::cdj_registrations(0, ports.0, ports.1).to_vec())
                    .encode()
            }
            _ => return None,
        })
    }

    fn answer_mount(call: &Call<'_>, behaviour: &Behaviour) -> Option<Vec<u8>> {
        let request = mount::Request::parse(mount::Proc(call.procedure), call.arguments).ok()?;
        match request {
            mount::Request::Mnt(path) => {
                let response = match behaviour.refuse_mount {
                    Some(status) => mount::Response::Mnt(Err(status)),
                    None if mount::slot_for_export(&path.to_string_lossy()).is_some() => {
                        mount::Response::Mnt(Ok(Tree::handle_for("/")))
                    }
                    None => mount::Response::Mnt(Err(ErrorStatus::NOENT)),
                };
                Some(response.encode())
            }
            mount::Request::Umnt(_) => Some(mount::Response::Umnt.encode()),
            mount::Request::Export if !behaviour.exports.is_empty() => {
                let exports: Vec<mount::Export> = behaviour
                    .exports
                    .iter()
                    .map(|path| mount::Export::new(path, &[mount::Export::LINK_LOCAL_SUBNET]))
                    .collect();
                Some(mount::Response::Export(exports).encode())
            }
            _ => None,
        }
    }

    fn answer_nfs(
        call: &Call<'_>,
        tree: &Tree,
        behaviour: &Behaviour,
        verbatim: &AtomicU64,
        mangled: &AtomicU64,
    ) -> Option<Vec<u8>> {
        let request = nfs2::Request::parse(nfs2::Proc(call.procedure), call.arguments).ok()?;
        let note = |handle: FileHandle| {
            if tree.path_of(handle).is_some() {
                verbatim.fetch_add(1, Ordering::SeqCst);
            } else {
                mangled.fetch_add(1, Ordering::SeqCst);
            }
        };
        match request {
            nfs2::Request::GetAttr(handle) => {
                note(handle);
                let response = match tree.path_of(handle).and_then(|path| tree.attributes(&path)) {
                    Some(attr) => nfs2::Response::Attr(Ok(attr)),
                    None => nfs2::Response::Attr(Err(ErrorStatus::STALE)),
                };
                Some(response.encode())
            }
            nfs2::Request::Lookup { dir, name } => {
                note(dir);
                let found = tree.path_of(dir).and_then(|parent| {
                    let wanted = name.to_string_lossy();
                    let child = tree
                        .directories
                        .get(&parent)?
                        .iter()
                        .find(|child| **child == wanted)?;
                    let path = if parent == "/" {
                        format!("/{child}")
                    } else {
                        format!("{parent}/{child}")
                    };
                    Some((Tree::handle_for(&path), tree.attributes(&path)?))
                });
                let response = match found {
                    Some((handle, attr)) => nfs2::Response::Lookup(Ok(FileRef { handle, attr })),
                    None => nfs2::Response::Lookup(Err(ErrorStatus::NOENT)),
                };
                Some(response.encode())
            }
            nfs2::Request::Read(args) => {
                note(args.handle);
                let data = tree
                    .path_of(args.handle)
                    .and_then(|path| tree.files.get(&path).cloned());
                let Some(data) = data else {
                    return Some(nfs2::Response::Read(Err(ErrorStatus::STALE)).encode());
                };
                let start = usize::try_from(args.offset)
                    .unwrap_or(usize::MAX)
                    .min(data.len());
                let mut count = usize::try_from(args.count).unwrap_or(0);
                if let Some(cap) = behaviour.short_read {
                    count = count.min(usize::try_from(cap).unwrap_or(count));
                }
                let end = start.saturating_add(count).min(data.len());
                let slice = data.get(start..end).unwrap_or_default().to_vec();
                Some(
                    nfs2::Response::Read(Ok(nfs2::FileData {
                        attr: read_reply_attr(args.handle),
                        data: &slice,
                    }))
                    .encode(),
                )
            }
            _ => None,
        }
    }

    /// The `fattr` a real CDJ puts in a `READ` reply: everything zero but the
    /// `fileid`, in 7884 of 7884 observed replies. A client that read `size`
    /// out of this would stop at the first read.
    fn read_reply_attr(handle: FileHandle) -> Fattr {
        Fattr {
            ftype: FType(0),
            mode: 0,
            nlink: 0,
            uid: 0,
            gid: 0,
            size: 0,
            blocksize: 0,
            rdev: 0,
            blocks: 0,
            fsid: 0,
            fileid: handle.fileid(),
            atime_sec: 0,
            atime_usec: 0,
            mtime_sec: 0,
            mtime_usec: 0,
            ctime_sec: 0,
            ctime_usec: 0,
        }
    }

    fn ramp(size: usize) -> Vec<u8> {
        (0..size)
            .map(|i| u8::try_from(i % 251).unwrap_or(0))
            .collect()
    }

    fn medium(size: usize) -> Tree {
        let mut tree = Tree::default()
            .with_file(EXPORT_PDB, &ramp(size))
            .with_file("/Contents/GESAFFELSTEIN/Pursuit.mp3", b"audio bytes")
            .with_file(
                "/Contents/\u{30ab}\u{3099}\u{30ab}\u{3099}\u{30df}.mp3",
                b"nfd",
            );
        // A folder of covers, which is the shape that exhausted a player's
        // handle table in the reference port.
        for index in 0..12 {
            tree = tree.with_file(
                &format!("/PIONEER/Artwork/00001/a{index:04}.jpg"),
                &[u8::try_from(index).unwrap_or(0); 8],
            );
        }
        tree
    }

    async fn client_for(deck: &Deck, read_size: ReadSize) -> NfsClient {
        NfsClient::connect_with(
            Ipv4Addr::LOCALHOST,
            None,
            NfsConfig {
                read_size,
                portmap_port: deck.portmap_port,
                ..NfsConfig::default()
            },
        )
        .await
        .expect("connecting to the loopback deck")
    }

    // -- discovery --------------------------------------------------------

    #[tokio::test]
    async fn the_ports_come_from_the_portmapper_not_from_the_constants() {
        let deck = deck(&medium(64), &Behaviour::default());
        let client = client_for(&deck, ReadSize::CDJ).await;
        assert_ne!(
            client.ports().mount,
            mount::PIONEER_PORT,
            "the loopback deck is on an ephemeral port, so a client that \
             assumed 48276 would be talking to nothing"
        );
        assert_ne!(client.ports().nfs, nfs2::PORT);
        assert_ne!(client.ports().mount, client.ports().nfs);
    }

    #[tokio::test]
    async fn an_unregistered_program_is_explained_with_a_dump() {
        // A GETPORT of zero and a portmapper that does not answer look
        // identical to a client that asks only the first.
        let socket = bind_local();
        let portmap_port = port_of(&socket);
        let task = tokio::spawn(async move {
            let mut buffer = vec![0u8; MAX_DATAGRAM];
            while let Ok((len, from)) = socket.recv_from(&mut buffer).await {
                let Ok(call) = Call::parse(&buffer[..len]) else {
                    continue;
                };
                let results = if call.procedure == portmap::Proc::DUMP.0 {
                    portmap::Response::Dump(Vec::new()).encode()
                } else {
                    portmap::Response::GetPort(None).encode()
                };
                let _ = socket
                    .send_to(&Reply::success(call.xid, &results).encode(), from)
                    .await;
            }
        });

        let error = NfsClient::connect_with(
            Ipv4Addr::LOCALHOST,
            None,
            NfsConfig {
                portmap_port,
                ..NfsConfig::default()
            },
        )
        .await
        .expect_err("a deck with nothing registered cannot be mounted");
        let text = error.to_string();
        assert!(text.contains("mountd"), "{text}");
        assert!(text.contains("nothing registered"), "{text}");
        task.abort();
    }

    // -- mounting ---------------------------------------------------------

    #[tokio::test]
    async fn a_slot_is_mounted_at_the_path_the_device_itself_names() {
        // C6: the same player mounts `/C/` on one peer and `/C/EXPORT` on
        // another, so the path has to come from EXPORT where EXPORT works.
        let deck = deck(
            &medium(64),
            &Behaviour {
                exports: vec!["/C/EXPORT".to_owned()],
                ..Behaviour::default()
            },
        );
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        assert_eq!(mounted.export(), "/C/EXPORT");
        assert_eq!(mounted.slot(), Some(Slot::USB), "matched on the prefix");
    }

    #[tokio::test]
    async fn a_deck_that_refuses_export_still_mounts_the_documented_path() {
        // A real deck answers EXPORT with PROC_UNAVAIL, which is not an error.
        let deck = deck(&medium(64), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        assert!(client.exports().await.is_err(), "PROC_UNAVAIL");
        let mounted = client.mount_slot(Slot::SD).await.expect("MNT");
        assert_eq!(mounted.export(), mount::EXPORT_SD);
    }

    #[tokio::test]
    async fn a_refused_mount_says_which_status_so_acces_can_mean_announce_first() {
        let deck = deck(
            &medium(64),
            &Behaviour {
                refuse_mount: Some(ErrorStatus::ACCES),
                ..Behaviour::default()
            },
        );
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let error = client.mount_slot(Slot::USB).await.expect_err("refused");
        assert!(
            matches!(error, Error::Nfs { status, .. } if status == ErrorStatus::ACCES),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn unmounting_is_answered_with_nothing_at_all() {
        let deck = deck(&medium(64), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        client.unmount(&mounted).await.expect("UMNT");
    }

    // -- walking ----------------------------------------------------------

    #[tokio::test]
    async fn a_path_is_walked_one_lookup_per_component() {
        let deck = deck(&medium(64), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        let file = client.open(&mounted, EXPORT_PDB).await.expect("open");
        assert_eq!(file.size(), 64);
        assert_eq!(file.path(), EXPORT_PDB);
        assert_eq!(
            client.stats().lookups,
            3,
            "PIONEER, rekordbox, export.pdb — and nothing else"
        );
    }

    /// The finding the whole cache exists for: re-walking from the root for
    /// every cover minted ~2300 handles across one medium and **495 of 576**
    /// fetches came back `NFSERR_STALE`. A real CDJ uses four directory
    /// handles across forty-eight lookups.
    #[tokio::test]
    async fn twelve_files_in_one_folder_cost_twelve_lookups_not_forty_eight() {
        let deck = deck(&medium(64), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");

        for index in 0..12 {
            client
                .open(&mounted, &format!("/PIONEER/Artwork/00001/a{index:04}.jpg"))
                .await
                .expect("open a cover");
        }
        assert_eq!(
            client.stats().lookups,
            4 + 11,
            "four for the first path, then one per file — not four each"
        );
    }

    #[tokio::test]
    async fn a_cached_directory_that_stops_resolving_is_re_walked_once() {
        let deck = deck(&medium(64), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        client.open(&mounted, EXPORT_PDB).await.expect("open");

        // Poison the cache the way a churned handle table would.
        client.directories.insert(
            (mounted.export().to_owned(), "/PIONEER/rekordbox".to_owned()),
            FileHandle::ZERO,
        );
        let before = client.stats().lookups;
        let file = client.open(&mounted, EXPORT_PDB).await.expect("re-walk");
        assert_eq!(file.size(), 64);
        assert!(
            client.stats().lookups - before >= 4,
            "the stale leaf lookup, then a full walk from the root"
        );
    }

    #[tokio::test]
    async fn a_component_spelled_differently_in_the_database_still_resolves() {
        // export.pdb says `Gesaffelstein` where the directory is
        // `GESAFFELSTEIN`, and stores NFC where the medium reports NFD (O6).
        let deck = deck(&medium(64), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");

        let by_case = client
            .open(&mounted, "/Contents/Gesaffelstein/Pursuit.mp3")
            .await
            .expect("a case-folded component must resolve");
        assert_eq!(by_case.size(), 11);

        let composed = "/Contents/\u{30ac}\u{30ac}\u{30df}.mp3";
        let by_normalisation = client
            .open(&mounted, composed)
            .await
            .expect("NFC must find NFD");
        assert_eq!(by_normalisation.size(), 3);
    }

    #[tokio::test]
    async fn a_missing_name_reports_the_component_and_the_peers_status() {
        let deck = deck(&medium(64), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        let error = client
            .open(&mounted, "/PIONEER/rekordbox/nothing.pdb")
            .await
            .expect_err("no such file");
        match error {
            Error::Nfs { path, status, .. } => {
                assert_eq!(status, ErrorStatus::NOENT);
                assert!(path.ends_with("nothing.pdb"), "{path}");
            }
            other => panic!("expected an NFS status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn opening_a_directory_is_refused_rather_than_read() {
        let deck = deck(&medium(64), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        let error = client
            .open(&mounted, "/PIONEER")
            .await
            .expect_err("a directory is not a file");
        assert!(
            matches!(error, Error::Nfs { status, .. } if status == ErrorStatus::ISDIR),
            "{error:?}"
        );
    }

    // -- reading ----------------------------------------------------------

    #[tokio::test]
    async fn a_whole_megabyte_pulls_byte_for_byte_with_progress() {
        // The reference implementation's measurement: 842 READs for a 1 MB
        // export.pdb at 1280 bytes each.
        let size = 842 * 1280;
        let deck = deck(&medium(size), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::UNFRAGMENTED).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        let file = client.open(&mounted, EXPORT_PDB).await.expect("open");

        let mut seen = Vec::new();
        let bytes = client
            .read_file_with(&file, |progress| seen.push(progress.read))
            .await
            .expect("the pull must complete");

        assert_eq!(bytes, ramp(size), "every byte, in order");
        assert_eq!(
            client.stats().reads,
            842,
            "1 MB at 1280 bytes a read is 842 round trips — 6.4x what a real \
             CDJ needs at 8192"
        );
        assert_eq!(client.stats().retries, 0, "a quiet link needs none");
        assert_eq!(seen.first(), Some(&0), "progress starts at zero");
        assert_eq!(
            seen.last().copied(),
            u64::try_from(size).ok(),
            "and ends at the whole file"
        );
        assert!(
            seen.windows(2).all(|pair| pair[0] <= pair[1]),
            "progress never goes backwards"
        );
    }

    #[tokio::test]
    async fn the_cdj_read_size_needs_six_times_fewer_round_trips() {
        let size = 842 * 1280;
        let deck = deck(&medium(size), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        let file = client.open(&mounted, EXPORT_PDB).await.expect("open");
        let bytes = client.read_file(&file).await.expect("pull");
        assert_eq!(bytes.len(), size);
        assert_eq!(
            client.stats().reads,
            u64::try_from(size.div_ceil(8192)).unwrap()
        );
        assert!(
            client.stats().reads * 6 < 842,
            "8192-byte reads are 6.4x fewer than 1280-byte ones"
        );
    }

    #[tokio::test]
    async fn a_short_read_is_re_requested_rather_than_taken_for_the_end() {
        // A server may answer less than was asked for at any point. Treating
        // that as EOF truncates the file, which presents as corrupt audio —
        // and a truncated export.pdb parses far enough to look plausible.
        let size = 5000;
        let deck = deck(
            &medium(size),
            &Behaviour {
                short_read: Some(37),
                ..Behaviour::default()
            },
        );
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        let file = client.open(&mounted, EXPORT_PDB).await.expect("open");
        let bytes = client.read_file(&file).await.expect("pull");
        assert_eq!(bytes, ramp(size), "all of it, 37 bytes at a time");
        assert_eq!(
            client.stats().reads,
            u64::try_from(size.div_ceil(37)).unwrap()
        );
    }

    #[tokio::test]
    async fn a_read_reply_with_an_all_zero_fattr_does_not_end_the_transfer() {
        // 7884 of 7884 READ replies from a real CDJ carry `size = 0`. A client
        // that believed it would return an empty file here.
        let deck = deck(&medium(3000), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::UNFRAGMENTED).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        let file = client.open(&mounted, EXPORT_PDB).await.expect("open");
        assert_eq!(client.read_file(&file).await.expect("pull").len(), 3000);
    }

    #[tokio::test]
    async fn an_arbitrary_range_can_be_read_for_streaming() {
        let deck = deck(&medium(4096), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::UNFRAGMENTED).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        let file = client.open(&mounted, EXPORT_PDB).await.expect("open");

        // A track read typically starts at 44, past a container header.
        let middle = client.read_range(&file, 44, 100).await.expect("range");
        assert_eq!(middle, ramp(144).get(44..).unwrap());

        let past_the_end = client.read_range(&file, 4000, 8192).await.expect("range");
        assert_eq!(past_the_end.len(), 96, "clamped to the file, not an error");
    }

    #[tokio::test]
    async fn a_filehandle_is_echoed_back_byte_for_byte() {
        // As a client a handle is an opaque token. A CDJ rewrites all but the
        // leading twelve bytes of one it was served (F28); doing the same to a
        // player would look to it like a handle it never issued.
        let deck = deck(&medium(1024), &Behaviour::default());
        let mut client = client_for(&deck, ReadSize::CDJ).await;
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        let file = client.open(&mounted, EXPORT_PDB).await.expect("open");
        client.read_file(&file).await.expect("pull");
        assert_eq!(
            deck.mangled.load(Ordering::SeqCst),
            0,
            "every handle must go back exactly as it came"
        );
        assert!(deck.echoed_verbatim.load(Ordering::SeqCst) > 0);
    }

    // -- the UDP realities ------------------------------------------------

    #[tokio::test]
    async fn a_dropped_datagram_is_retransmitted_and_the_pull_still_completes() {
        let size = 4096;
        let deck = deck(
            &medium(size),
            &Behaviour {
                drop_every: 3,
                ..Behaviour::default()
            },
        );
        let mut client = NfsClient::connect_with(
            Ipv4Addr::LOCALHOST,
            None,
            NfsConfig {
                read_size: ReadSize::UNFRAGMENTED,
                portmap_port: deck.portmap_port,
                timeout: Duration::from_millis(60),
                ..NfsConfig::default()
            },
        )
        .await
        .expect("connect through a lossy link");
        let mounted = client.mount_slot(Slot::USB).await.expect("MNT");
        let file = client.open(&mounted, EXPORT_PDB).await.expect("open");
        let bytes = client.read_file(&file).await.expect("pull");
        assert_eq!(bytes, ramp(size));
        assert!(
            client.stats().retries > 0,
            "one datagram in three was dropped, so retransmission is what \
             carried this"
        );
        assert_eq!(client.stats().timeouts, 0, "and none of it gave up");
    }

    #[tokio::test]
    async fn a_dead_peer_times_out_rather_than_hanging() {
        let deck = deck(
            &medium(64),
            &Behaviour {
                deaf: true,
                ..Behaviour::default()
            },
        );
        let started = std::time::Instant::now();
        let error = NfsClient::connect_with(
            Ipv4Addr::LOCALHOST,
            None,
            NfsConfig {
                portmap_port: deck.portmap_port,
                timeout: Duration::from_millis(30),
                attempts: NonZeroU8::new(3).unwrap(),
                ..NfsConfig::default()
            },
        )
        .await
        .expect_err("a deaf peer must not hang");
        assert!(matches!(error, Error::Timeout { .. }), "{error:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "gave up promptly"
        );
        assert_eq!(
            deck.datagrams.load(Ordering::SeqCst),
            3,
            "every attempt was actually sent"
        );
    }

    #[tokio::test]
    async fn a_reply_to_an_abandoned_call_is_discarded_not_mistaken_for_this_one() {
        // A late reply carrying an old xid must not be decoded as the current
        // procedure's results: the two have different shapes and nothing on
        // the wire would object.
        let socket = bind_local();
        let portmap_port = port_of(&socket);
        let task = tokio::spawn(async move {
            let mut buffer = vec![0u8; MAX_DATAGRAM];
            let mut answered = 0u32;
            while let Ok((len, from)) = socket.recv_from(&mut buffer).await {
                let Ok(call) = Call::parse(&buffer[..len]) else {
                    continue;
                };
                answered += 1;
                // A stale MNT reply under an xid nobody is waiting for.
                let stale = mount::Response::Mnt(Ok(FileHandle::ZERO)).encode();
                let _ = socket
                    .send_to(&Reply::success(Xid(0), &stale).encode(), from)
                    .await;
                if answered == 1 {
                    // Nothing for the first attempt: the client must keep
                    // waiting past the junk rather than accept it.
                    continue;
                }
                let results = portmap::Response::GetPort(Some(1)).encode();
                let _ = socket
                    .send_to(&Reply::success(call.xid, &results).encode(), from)
                    .await;
            }
        });

        let client = NfsClient::connect_with(
            Ipv4Addr::LOCALHOST,
            None,
            NfsConfig {
                portmap_port,
                timeout: Duration::from_millis(40),
                ..NfsConfig::default()
            },
        )
        .await
        .expect("the retransmission must be answered");
        assert_eq!(client.ports().mount, 1);
        assert!(
            client.stats().retries >= 1,
            "the junk did not count as a reply"
        );
        task.abort();
    }

    // -- units ------------------------------------------------------------

    #[test]
    fn a_read_size_outside_the_nfsv2_maximum_cannot_be_built() {
        assert_eq!(ReadSize::new(8192).map(ReadSize::get), Some(8192));
        assert_eq!(ReadSize::new(8193), None, "past what RFC 1094 permits");
        assert_eq!(ReadSize::new(0), None, "a read of nothing never ends");
        assert_eq!(ReadSize::CDJ.get(), 8192, "F19");
        assert_eq!(ReadSize::UNFRAGMENTED.get(), 1280);
        assert_eq!(
            ReadSize::default(),
            ReadSize::CDJ,
            "behave like the hardware unless told otherwise"
        );
    }

    #[test]
    fn the_exact_spelling_is_tried_first_and_duplicates_are_dropped() {
        assert_eq!(spellings("PIONEER"), vec!["PIONEER", "pioneer"]);
        let folded = spellings("Gesaffelstein");
        assert_eq!(folded.first().map(String::as_str), Some("Gesaffelstein"));
        assert!(folded.iter().any(|name| name == "GESAFFELSTEIN"));
        assert!(folded.iter().any(|name| name == "gesaffelstein"));
    }

    #[test]
    fn a_composed_name_offers_its_decomposed_spelling() {
        // The pdb stores NFC; a FAT32 driver reports NFD.
        let composed = "\u{30ac}\u{30ac}\u{30df}.mp3";
        let decomposed = "\u{30ab}\u{3099}\u{30ab}\u{3099}\u{30df}.mp3";
        assert_ne!(composed.as_bytes(), decomposed.as_bytes());
        assert!(spellings(composed).iter().any(|name| name == decomposed));
    }

    #[test]
    fn a_path_splits_into_a_parent_and_a_leaf() {
        assert_eq!(
            split_parent(EXPORT_PDB),
            Some(("/PIONEER/rekordbox".to_owned(), "export.pdb".to_owned()))
        );
        assert_eq!(
            split_parent("/x"),
            Some((String::new(), "x".to_owned())),
            "a file at the mount root has an empty parent, not no parent"
        );
        assert_eq!(split_parent("/"), None);
    }

    #[test]
    fn progress_is_a_fraction_even_for_an_empty_file() {
        assert!((Progress { read: 0, total: 0 }.fraction() - 1.0).abs() < f64::EPSILON);
        assert!((Progress { read: 1, total: 4 }.fraction() - 0.25).abs() < f64::EPSILON);
        assert!((Progress { read: 9, total: 4 }.fraction() - 1.0).abs() < f64::EPSILON);
    }

    /// The bytes a real deck puts on the wire, as a floor under everything
    /// above: our own encoder and our own decoder agreeing proves only that
    /// they agree with each other.
    #[test]
    fn our_calls_have_the_shape_a_real_decks_calls_have() {
        let credential = AuthUnix::cdj(rpc::STAMP_FIRST_CALL).encode();

        // Deck to deck, `S13-format-ground-truth` frames 91-92: a deck's very
        // first call after power-on asks for mountd, and it is 76 bytes.
        let getport = Call::new(
            Xid(1),
            Program::PORTMAP,
            portmap::VERSION,
            portmap::Proc::GETPORT.0,
            Auth::unix(&credential),
            &portmap::Request::GetPort(portmap::Mapping::query(
                Program::MOUNT,
                mount::VERSION,
                IpProtocol::UDP,
            ))
            .encode_arguments(),
        )
        .encode();
        assert_eq!(getport.len(), 76, "every GETPORT call in the corpus");

        // `MNT '/C/'`: F12's `raw=2f0043002f00`, counted in bytes.
        let arguments =
            mount::Request::Mnt(Utf16LeString::new(mount::EXPORT_USB)).encode_arguments();
        assert_eq!(
            arguments,
            [
                0x00, 0x00, 0x00, 0x06, 0x2f, 0x00, 0x43, 0x00, 0x2f, 0x00, 0x00, 0x00
            ]
        );

        // A READ call is always 104 bytes: the 60-byte header a player sends,
        // the bare 32-byte handle, and three words.
        let read = Call::new(
            Xid(9),
            Program::NFS,
            nfs2::VERSION,
            nfs2::Proc::READ.0,
            Auth::unix(&credential),
            &nfs2::Request::Read(nfs2::ReadArgs::at(FileHandle::ZERO, 44, 8192).unwrap())
                .encode_arguments(),
        )
        .encode();
        assert_eq!(read.len(), 104);
    }
}
