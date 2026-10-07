// SPDX-License-Identifier: GPL-3.0-only

//! Turning one call into one reply, for all three programs.
//!
//! Everything here is synchronous and takes no sockets, so the whole serving
//! surface — every status a player can be told, every handle it can send back —
//! is testable from a byte literal. [`super`] owns the sockets and does nothing
//! but hand datagrams to [`Dispatcher::answer`] and send back what it returns.
//!
//! # Which program a port answers for is decided by the port
//!
//! A [`Service`] is fixed when the socket is bound, so a call that arrives on
//! the wrong port is answered `PROG_UNAVAIL` and a call for the right program
//! at the wrong version gets `PROG_MISMATCH` carrying the range. Both are real
//! answers, and answering rather than dropping is what a player expects when it
//! probes.
//!
//! # The export table is the tree
//!
//! There is no separate list of exports to keep in step with the medium: an
//! export is served exactly when its subtree is in the [`Vfs`], so inserting a
//! stick is a `Vfs::mount` and nothing else, and `MNT` of an empty slot answers
//! `NFSERR_NOENT` by construction. [`ServedSlot`] supplies both halves — the
//! export path a player names and the subtree it maps to — so the two cannot
//! disagree.
//!
//! `MNT` matches on the drive-letter **prefix**, because one capture shows the
//! same player mounting `/C/` on one peer and `/C/EXPORT` on another (C6).
//!
//! # Statuses are a user-visible surface
//!
//! On a CDJ's screen an error and an empty folder look identical, so the
//! difference between `NFSERR_STALE` and `NFSERR_NOENT` is not an internal
//! detail: the first tells a deck to mount again, the second that a file is
//! gone. `Vfs` answers `None` to both, so this module resolves the parent
//! handle first and only then looks the name up.
//!
//! # Attributes are the ones a proven server sent
//!
//! `S10j-serve-to-cdj` is a whole load and playback with zero errors, and the
//! server in it answered `LOOKUP`, `GETATTR` and `READ` with a complete
//! `fattr`: mode `0o40755`/`0o100644`, `fsid` 1, `rdev` 0, and 2020-09-13 in
//! all three timestamps. Those are what [`Vfs::attributes`] synthesises and
//! what we send, so a capture of this server diffs against that session to
//! nothing.
//!
//! It is *not* what a real deck sends. A deck fills in a `READ` reply's `fattr`
//! with zeroes but for the `fileid` (7884 of 7884), and uses mode `0o100000`,
//! `rdev` 1, `fsid` 2 elsewhere. Reproducing that would mean removing correct
//! information from a reply that is known to work in exactly this role, so it
//! is deliberately not reproduced; the one field where hardware and the
//! reference disagreed and hardware won is `fileid`, which `Vfs::attributes`
//! takes from the handle's leading word as a deck does in 8285 of 8285 replies.
//!
//! # Credentials are decoded and not enforced
//!
//! A real player exports its media to the whole link-local subnet
//! (`169.254.0.0/255.255.0.0`), which is why a host that has never announced
//! itself can read a deck's files at all (F11, F12). So the `AUTH_UNIX`
//! credential is decoded for the trace log and acted on by nothing, and no
//! procedure here consults the mount registry before answering. Being stricter
//! than the hardware we are impersonating could only make us the reason a real
//! deck fails.
//!
//! # Retransmits, and why there is no duplicate-request cache
//!
//! A deck that gets no answer resends the identical datagram — same `xid`, same
//! credential, once a second: 29 of the 31 calls in `S24c-e9-noportmap` are
//! retransmissions of two calls nothing answered. Across the sessions where a
//! server *did* answer, 45 763 calls carry 45 763 distinct `xid`s and there is
//! not one retransmission in the corpus.
//!
//! So a duplicate-request cache would buy nothing here. It exists in general
//! NFS servers to keep a retried `WRITE` or `REMOVE` from being applied twice;
//! every procedure this server implements is idempotent, because a rekordbox
//! export is read-only, and re-running one costs a hash lookup and at worst an
//! 8 KiB read. A cache would add memory, a lifetime policy, and a way to answer
//! from a medium that has since been ejected.

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, RwLock};

use prolink_proto::rpc::nfs2::{
    Cookie, DirEntry, ErrorStatus, FileData, FileHandle, FileRef, FsStat, Listing, NfsResult,
    ReadArgs, ReadDirArgs, Status,
};
use prolink_proto::rpc::xdr::{self, Utf16LeString};
use prolink_proto::rpc::{
    Accepted, Auth, AuthUnix, Call, FailureStat, IpProtocol, Program, Reply, mount, nfs2, portmap,
};
use tracing::{debug, trace};

use crate::serve::medium::ServedSlot;
use crate::serve::vfs::{Node, Vfs};

use super::Ports;

/// The most payload one reply may carry, for a `READ` or a `READDIR`.
///
/// A reply of this size is 8292 bytes on the wire — the payload plus the RPC
/// header, the status word, the `fattr` and the payload's own length prefix.
///
/// RFC 1094's ceiling, and **hardware exceeds it in both directions**: deck to
/// deck the modal request is 9408 bytes and a file's first read can be 28584,
/// answered in full as one datagram in about twenty IP fragments. Answering
/// like that is nonetheless not portable — **macOS refuses to send a UDP
/// datagram larger than 9216 bytes** (`net.inet.udp.maxdgram`, its default),
/// so a reply of 9508 fails with `EMSGSIZE` and is never sent at all. A stall
/// is much worse than a short read: a short read is ordinary and the client
/// asks for the rest, where a reply that cannot be sent leaves the deck
/// retransmitting for ever.
///
/// So the cap is the specification's, which every platform can send, which
/// both reference implementations used while a CDJ-2000NXS loaded and played
/// from them (F39), and which is what every read a deck has ever sent *us* asks
/// for — 160, 2048 and 8192 across the serve sessions. Larger requests are
/// answered short.
///
/// **A short answer is not a guess about what hardware tolerates.** Replaying
/// the corpus through this server measured it: a real Pioneer server answered
/// short of the request **1372 times mid-file**, and the reading device's next
/// read of that file resumed at exactly the shortfall **1296** of those times.
/// A deck asking us for 9408 and getting 8192 therefore asks for the remaining
/// 1216, which is what it does to its own kind routinely.
/// This is [`nfs2::MAX_DATA`] as a `u32`, which the two are tested to agree on.
const MAX_READ: u32 = 8192;

/// Bytes a `READDIR` reply spends before its first entry and after its last:
/// the RPC header, the status word, the end-of-list marker and the eof flag.
const READDIR_OVERHEAD: usize = 24 + 4 + 4 + 4;

/// How many `MNT`s to remember for `DUMP`.
///
/// A registry, not a permission check — nothing consults it — so the only
/// question is how much a peer may make us keep. Four players and two slots is
/// eight; sixteen leaves room and still bounds a stranger looping on `MNT`.
const MAX_MOUNTS: usize = 16;

/// Filesystem statistics, which no player has ever asked for.
///
/// RFC 1094 as written and the numbers the reference server answered with; no
/// capture in the corpus contains a `STATFS` in either direction, so there is
/// nothing to reproduce. A medium we serve is read-only to the player reading
/// it, and a deck's Link Info panel takes its free-space figure from the
/// dbserver media response rather than from here.
const STATFS: FsStat = FsStat {
    tsize: 8192,
    bsize: 512,
    blocks: 1_000_000,
    bfree: 500_000,
    bavail: 500_000,
};

/// Which of the three programs a socket answers for.
///
/// A real enum because we own every case: one port, one program, decided when
/// the socket is bound.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Service {
    /// Program 100000 v2 — the gate on everything (F46).
    Portmap,
    /// Program 100005 v1.
    Mount,
    /// Program 100003 v2.
    Nfs,
}

impl Service {
    /// The program number this service answers for.
    pub(crate) const fn program(self) -> Program {
        match self {
            Self::Portmap => Program::PORTMAP,
            Self::Mount => Program::MOUNT,
            Self::Nfs => Program::NFS,
        }
    }

    /// The only version of it we speak.
    pub(crate) const fn version(self) -> u32 {
        match self {
            Self::Portmap => portmap::VERSION,
            Self::Mount => mount::VERSION,
            Self::Nfs => nfs2::VERSION,
        }
    }
}

/// One export a peer holds open.
///
/// Kept for `DUMP` and for logging which deck is reading what. Nothing consults
/// it to decide access: a player exports to the whole link-local subnet and
/// being stricter than the hardware we impersonate could only make us the
/// reason a real deck fails (F11, F12).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mount {
    /// The address that called `MNT`.
    pub peer: Ipv4Addr,
    /// The export path it named, spelled as it spelled it.
    pub export: String,
}

/// The state all three servers share: the tree, the ports and who has mounted.
#[derive(Debug)]
pub(crate) struct Dispatcher {
    vfs: Arc<RwLock<Vfs>>,
    ports: Ports,
    mounts: Mutex<Vec<Mount>>,
}

impl Dispatcher {
    /// Serve `vfs`, publishing `ports` through the portmapper.
    pub(crate) fn new(vfs: Arc<RwLock<Vfs>>, ports: Ports) -> Self {
        Self {
            vfs,
            ports,
            mounts: Mutex::new(Vec::new()),
        }
    }

    /// The exports peers currently hold open.
    pub(crate) fn mounts(&self) -> Vec<Mount> {
        self.with_mounts(<[Mount]>::to_vec)
    }

    /// Answer one datagram that arrived on `service`'s socket.
    ///
    /// `None` means "say nothing", which is the right answer for traffic that
    /// is not an RPC call at all: our ports see strays, and a reply that
    /// wandered in is not something to reply to.
    pub(crate) fn answer(
        &self,
        service: Service,
        datagram: &[u8],
        peer: Ipv4Addr,
    ) -> Option<Vec<u8>> {
        let call = match Call::parse(datagram) {
            Ok(call) => call,
            Err(error) => {
                trace!(%error, %peer, bytes = datagram.len(), "not an RPC call");
                return None;
            }
        };

        // Decoded for the record and acted on by nothing — see the module
        // documentation on credentials — and only when somebody is listening,
        // since decoding one allocates and a load is thousands of calls.
        if tracing::enabled!(tracing::Level::TRACE)
            && let Ok(credential) = AuthUnix::parse(call.credential.body)
        {
            trace!(
                xid = ?call.xid,
                flavor = ?call.credential.flavor,
                stamp = format_args!("{:#010x}", credential.stamp),
                uid = credential.uid,
                "credential",
            );
        }

        if call.program != service.program() {
            trace!(?service, program = ?call.program, "call for a program this port does not run");
            return Some(Reply::failed(call.xid, FailureStat::PROG_UNAVAIL).encode());
        }
        if call.version != service.version() {
            return Some(
                Reply::Accepted {
                    xid: call.xid,
                    verifier: Auth::NULL,
                    status: Accepted::ProgMismatch {
                        low: service.version(),
                        high: service.version(),
                    },
                }
                .encode(),
            );
        }

        let answered = match service {
            Service::Portmap => self.portmap(&call),
            Service::Mount => self.mountd(&call, peer),
            Service::Nfs => self.nfsd(&call),
        };
        Some(match answered {
            Ok(results) => Reply::success(call.xid, &results).encode(),
            Err(stat) => Reply::failed(call.xid, stat).encode(),
        })
    }

    /// Program 100000: what port is what.
    fn portmap(&self, call: &Call<'_>) -> Result<Vec<u8>, FailureStat> {
        let procedure = portmap::Proc(call.procedure);
        let request = portmap::Request::parse(procedure, call.arguments)
            .map_err(|_| FailureStat::GARBAGE_ARGS)?;
        Ok(match request {
            portmap::Request::Null => Vec::new(),
            portmap::Request::GetPort(mapping) => {
                let port = self.ports.serving(mapping);
                trace!(program = ?mapping.program, version = mapping.version, ?port, "GETPORT");
                portmap::Response::GetPort(port).encode()
            }
            portmap::Request::Dump => portmap::Response::Dump(
                portmap::cdj_registrations(self.ports.portmap, self.ports.mount, self.ports.nfs)
                    .to_vec(),
            )
            .encode(),
            // `SET` and `UNSET` land here. A remote caller has no business
            // registering anything with us, and `PROC_UNAVAIL` says so without
            // pretending the call was malformed.
            portmap::Request::Unknown { .. } => return Err(FailureStat::PROC_UNAVAIL),
        })
    }

    /// Program 100005: turning an export path into a root filehandle.
    fn mountd(&self, call: &Call<'_>, peer: Ipv4Addr) -> Result<Vec<u8>, FailureStat> {
        let procedure = mount::Proc(call.procedure);
        let request = mount::Request::parse(procedure, call.arguments)
            .map_err(|_| FailureStat::GARBAGE_ARGS)?;
        Ok(match request {
            mount::Request::Null => Vec::new(),
            mount::Request::Mnt(path) => {
                let path = path.to_string_lossy();
                let result = self.root_of(&path).ok_or(ErrorStatus::NOENT);
                if let Ok(handle) = &result {
                    debug!(%peer, %path, ?handle, "MNT");
                    self.remember_mount(peer, &path);
                } else {
                    // What a real player answers for a slot with no medium in
                    // it, and what our own client reads as "nothing there".
                    debug!(%peer, %path, "MNT of an export we do not serve");
                }
                mount::Response::Mnt(result).encode()
            }
            // Real players send these, one per slot, after a physical eject
            // (C9). There is no per-client state to release — a handle is a
            // hash of a path, not an allocation — so this only updates the
            // registry `DUMP` reads.
            mount::Request::Umnt(path) => {
                let path = path.to_string_lossy();
                debug!(%peer, %path, "UMNT");
                self.forget_mount(peer, Some(&path));
                mount::Response::Umnt.encode()
            }
            mount::Request::UmntAll => {
                self.forget_mount(peer, None);
                mount::Response::UmntAll.encode()
            }
            mount::Request::Dump => mount::Response::Dump(self.mount_entries()).encode(),
            // Never called by a real deck, in any session (F37).
            mount::Request::Export => mount::Response::Export(self.exports()).encode(),
            mount::Request::Unknown { .. } => return Err(FailureStat::PROC_UNAVAIL),
        })
    }

    /// Program 100003: the files themselves.
    fn nfsd(&self, call: &Call<'_>) -> Result<Vec<u8>, FailureStat> {
        let procedure = nfs2::Proc(call.procedure);
        let request = nfs2::Request::parse(procedure, call.arguments)
            .map_err(|_| FailureStat::GARBAGE_ARGS)?;
        let vfs = self.vfs();
        Ok(match request {
            nfs2::Request::Null => Vec::new(),
            nfs2::Request::GetAttr(handle) => {
                nfs2::Response::Attr(attributes(&vfs, handle)).encode()
            }
            nfs2::Request::Lookup { dir, name } => {
                let name = name.to_string_lossy();
                let found = lookup(&vfs, dir, &name);
                trace!(?dir, %name, status = %nfs2::Response::Lookup(found).status(), "LOOKUP");
                nfs2::Response::Lookup(found).encode()
            }
            nfs2::Request::Read(args) => {
                let read = read(&vfs, args);
                match &read {
                    Ok((attr, data)) => nfs2::Response::Read(Ok(FileData {
                        attr: *attr,
                        data: data.as_slice(),
                    })),
                    Err(status) => nfs2::Response::Read(Err(*status)),
                }
                .encode()
            }
            nfs2::Request::ReadDir(args) => nfs2::Response::ReadDir(listing(&vfs, args)).encode(),
            nfs2::Request::StatFs(handle) => nfs2::Response::StatFs(
                vfs.resolve(handle)
                    .map(|_| STATFS)
                    .ok_or(ErrorStatus::STALE),
            )
            .encode(),
            // Every write procedure lands here, and `SETATTR`, `READLINK` and
            // the two RFC 1094 declares obsolete. This is what a real CDJ
            // answered a reference client that tried `READDIR`, so it is a
            // normal thing for a client to handle rather than a broken server.
            nfs2::Request::Unknown { .. } => return Err(FailureStat::PROC_UNAVAIL),
        })
    }

    /// The root handle for an export path, or `None` if we do not serve it.
    ///
    /// Returns the handle for the medium's own subtree, never the tree's root:
    /// a handle is a hash of its path and a deck keeps only its leading twelve
    /// bytes (F28), so two media sharing a root would be indistinguishable
    /// afterwards.
    fn root_of(&self, export: &str) -> Option<FileHandle> {
        let slot = ServedSlot::new(mount::slot_for_export(export)?)?;
        let handle = Vfs::handle_for(&subtree(slot));
        self.vfs()
            .resolve(handle)
            .filter(|node| node.is_dir())
            .map(|_| handle)
    }

    /// The exports on offer: a slot appears exactly when its subtree is there.
    fn exports(&self) -> Vec<mount::Export> {
        let vfs = self.vfs();
        [ServedSlot::SD, ServedSlot::USB]
            .into_iter()
            .filter(|slot| {
                vfs.resolve(Vfs::handle_for(&subtree(*slot)))
                    .is_some_and(Node::is_dir)
            })
            // The whole link-local range, which is what a CDJ-2000NXS
            // publishes and the mechanism behind passive access (F11, F12).
            .map(|slot| mount::Export::new(slot.export_path(), &[mount::Export::LINK_LOCAL_SUBNET]))
            .collect()
    }

    fn mount_entries(&self) -> Vec<mount::MountEntry> {
        self.with_mounts(|mounts| {
            mounts
                .iter()
                .map(|mount| mount::MountEntry {
                    hostname: mount.peer.to_string(),
                    directory: Utf16LeString::new(&mount.export),
                })
                .collect()
        })
    }

    fn remember_mount(&self, peer: Ipv4Addr, export: &str) {
        self.with_mounts_mut(|mounts| {
            let held = mounts
                .iter()
                .any(|mount| mount.peer == peer && mount.export == export);
            if !held && mounts.len() < MAX_MOUNTS {
                mounts.push(Mount {
                    peer,
                    export: export.to_owned(),
                });
            }
        });
    }

    /// Forget one export a peer held, or all of them for `None`.
    fn forget_mount(&self, peer: Ipv4Addr, export: Option<&str>) {
        self.with_mounts_mut(|mounts| {
            mounts.retain(|mount| {
                mount.peer != peer || export.is_some_and(|export| mount.export != export)
            });
        });
    }

    /// The tree, for the duration of one call.
    ///
    /// Held across the disk read a `READ` performs, so grafting a medium in
    /// waits for the calls already in flight — milliseconds, and inserting a
    /// stick is not a moment that needs to be instant.
    fn vfs(&self) -> std::sync::RwLockReadGuard<'_, Vfs> {
        match self.vfs.read() {
            Ok(vfs) => vfs,
            // The tree holds no invariant a panic mid-`mount` could break that
            // a reader would notice — a half-grafted medium is a medium with
            // fewer files — so recovering beats propagating a panic into a
            // DJ's set.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn with_mounts<T>(&self, read: impl FnOnce(&[Mount]) -> T) -> T {
        match self.mounts.lock() {
            Ok(mounts) => read(&mounts),
            Err(poisoned) => read(&poisoned.into_inner()),
        }
    }

    fn with_mounts_mut<T>(&self, write: impl FnOnce(&mut Vec<Mount>) -> T) -> T {
        match self.mounts.lock() {
            Ok(mut mounts) => write(&mut mounts),
            Err(poisoned) => write(&mut poisoned.into_inner()),
        }
    }
}

/// The subtree a slot's medium occupies, `/B` or `/C`.
fn subtree(slot: ServedSlot) -> String {
    format!("/{}", slot.vfs_prefix())
}

/// Attributes for a handle, or the status a client should be told instead.
///
/// The 4 GiB ceiling is a refusal rather than a clamp: `fattr.size` is 32 bits,
/// and a file reported as 4 GiB minus one byte would be read to that point and
/// no further, which presents as a truncated track rather than as an error.
fn attributes(vfs: &Vfs, handle: FileHandle) -> NfsResult<nfs2::Fattr> {
    let node = vfs.resolve(handle).ok_or(ErrorStatus::STALE)?;
    if node.size() > nfs2::MAX_FILE_SIZE {
        return Err(file_too_big());
    }
    vfs.attributes(handle).ok_or(ErrorStatus::STALE)
}

/// `NFSERR_FBIG`: a file larger than NFSv2 can describe.
///
/// [`ErrorStatus`] has no constant for it, because no capture contains one —
/// its constructor refuses only `NFS_OK`, which 27 is not, so the fallback is
/// unreachable and costs a line rather than a panic.
fn file_too_big() -> ErrorStatus {
    ErrorStatus::new(Status::FBIG).unwrap_or(ErrorStatus::IO)
}

/// Walk one component.
///
/// Resolves the parent first so that "I never issued that handle" and "that
/// directory has no such child" are different statuses: the first tells a deck
/// to mount again, the second that a file is gone.
///
/// `.` and `..` are RFC 1094's, not Pioneer's — no deck has ever been seen to
/// ask for either, since it walks paths it read out of `export.pdb`. They cost
/// two lines and a generic client cannot navigate without them.
fn lookup(vfs: &Vfs, dir: FileHandle, name: &str) -> NfsResult<FileRef> {
    let parent = vfs.resolve(dir).ok_or(ErrorStatus::STALE)?;
    if !parent.is_dir() {
        return Err(ErrorStatus::NOTDIR);
    }
    let handle = match name {
        "." => Vfs::handle_for(vfs.path_of(dir).ok_or(ErrorStatus::STALE)?),
        ".." => Vfs::handle_for(parent_of(vfs.path_of(dir).ok_or(ErrorStatus::STALE)?)),
        // Never the handle for the name as asked for: matching is case- and
        // normalisation-insensitive, and hashing the spelling a player used
        // would mint a handle that is in no table, so every later use of it
        // would come back `NFSERR_STALE` (O6).
        _ => vfs.lookup(dir, name).ok_or(ErrorStatus::NOENT)?.0,
    };
    Ok(FileRef {
        handle,
        attr: attributes(vfs, handle)?,
    })
}

/// One byte range, with the attributes that accompany it.
///
/// Returns the bytes rather than a borrow because the tree's lock is released
/// before the reply is encoded. A short answer is not an error and not the end
/// of the file: a client re-requests the shortfall.
fn read(vfs: &Vfs, args: ReadArgs) -> NfsResult<(nfs2::Fattr, Vec<u8>)> {
    let node = vfs.resolve(args.handle).ok_or(ErrorStatus::STALE)?;
    if node.is_dir() {
        return Err(ErrorStatus::ISDIR);
    }
    let attr = attributes(vfs, args.handle)?;
    let count = usize::try_from(args.count.min(MAX_READ)).unwrap_or(0);
    let data = vfs
        .read(args.handle, u64::from(args.offset), count)
        .ok_or(ErrorStatus::IO)?;
    Ok((attr, data))
}

/// One page of a directory listing.
///
/// The cookie is opaque to the client and minted by us, so it is the index of
/// the entry to resume at. `count` is a reply size in bytes, not a number of
/// entries, and a reply that fills it stops short with `eof` false rather than
/// overflowing the datagram.
fn listing(vfs: &Vfs, args: ReadDirArgs) -> NfsResult<Listing> {
    let node = vfs.resolve(args.handle).ok_or(ErrorStatus::STALE)?;
    if !node.is_dir() {
        return Err(ErrorStatus::NOTDIR);
    }
    let path = vfs.path_of(args.handle).ok_or(ErrorStatus::STALE)?;
    let children = vfs.read_dir(args.handle).ok_or(ErrorStatus::NOTDIR)?;
    let first = usize::try_from(u32::from_be_bytes(args.cookie.0)).unwrap_or(usize::MAX);
    let budget = usize::try_from(args.count.min(MAX_READ)).unwrap_or(0);

    let mut entries = Vec::new();
    let mut used = READDIR_OVERHEAD;
    let mut eof = true;
    for (index, child) in children.iter().enumerate().skip(first) {
        let name = Utf16LeString::new(child);
        // A value-follows word, the fileid, the counted name and the cookie.
        let cost = 16usize.saturating_add(xdr::align4(name.len_bytes()));
        if used.saturating_add(cost) > budget && !entries.is_empty() {
            eof = false;
            break;
        }
        used = used.saturating_add(cost);
        let Ok(next) = u32::try_from(index.saturating_add(1)) else {
            eof = false;
            break;
        };
        entries.push(DirEntry {
            fileid: Vfs::handle_for(&join(path, child)).fileid(),
            name,
            cookie: Cookie(next.to_be_bytes()),
        });
    }
    Ok(Listing { entries, eof })
}

/// The directory holding `path`; the root is its own parent, as POSIX has it.
fn parent_of(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => parent,
        _ => "/",
    }
}

/// The path of a child, matching how the tree spells one.
fn join(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

impl Ports {
    /// The port serving a mapping a `GETPORT` asked about.
    ///
    /// `None` encodes to the wire's zero — "that program is not registered" —
    /// which is a successful reply rather than an error, and needs no special
    /// case for a program we do not run.
    fn serving(self, mapping: portmap::Mapping) -> Option<u16> {
        if mapping.protocol != IpProtocol::UDP {
            // Nothing in Pro DJ Link's RPC is on TCP, and answering a port we
            // do not listen on there would send a player somewhere silent.
            return None;
        }
        match (mapping.program, mapping.version) {
            (Program::MOUNT, mount::VERSION) => Some(self.mount),
            (Program::NFS, nfs2::VERSION) => Some(self.nfs),
            (Program::PORTMAP, portmap::VERSION) => Some(self.portmap),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use prolink_proto::rpc::Xid;

    const PORTS: Ports = Ports {
        portmap: portmap::PORT,
        mount: mount::PIONEER_PORT,
        nfs: nfs2::PORT,
    };

    const PEER: Ipv4Addr = Ipv4Addr::new(169, 254, 202, 84);

    /// A USB stick with two tracks, one of them spelled the way a rekordbox
    /// database spells it and not the way the filesystem does.
    fn served() -> Dispatcher {
        let mut vfs = Vfs::new();
        vfs.add_file("/C/PIONEER/rekordbox/export.pdb", b"pdb bytes".to_vec());
        vfs.add_file(
            "/C/Contents/GESAFFELSTEIN/track.mp3",
            (0..=255u8).cycle().take(20_000).collect(),
        );
        vfs.add_file(
            "/C/Contents/\u{30ab}\u{3099}\u{30ab}\u{3099}\u{30df}.mp3",
            b"nfd".to_vec(),
        );
        Dispatcher::new(Arc::new(RwLock::new(vfs)), PORTS)
    }

    fn both_slots() -> Dispatcher {
        let mut vfs = Vfs::new();
        vfs.add_file("/C/PIONEER/rekordbox/export.pdb", b"usb".to_vec());
        vfs.add_file("/B/PIONEER/rekordbox/export.pdb", b"sd".to_vec());
        Dispatcher::new(Arc::new(RwLock::new(vfs)), PORTS)
    }

    /// Decode a hex literal, ignoring whitespace so a captured datagram can be
    /// wrapped for reading.
    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            digits.len().is_multiple_of(2),
            "an even number of hex digits"
        );
        digits
            .chunks_exact(2)
            .map(|pair| {
                let byte: String = pair.iter().collect();
                u8::from_str_radix(&byte, 16).expect("a hex literal must be hex")
            })
            .collect()
    }

    /// Build a call the way a player does, with an `AUTH_UNIX` credential.
    fn call(service: Service, procedure: u32, arguments: &[u8]) -> Vec<u8> {
        let credential = AuthUnix::cdj(prolink_proto::rpc::STAMP_FIRST_CALL).encode();
        Call::new(
            Xid(0x2a),
            service.program(),
            service.version(),
            procedure,
            Auth::unix(&credential),
            arguments,
        )
        .encode()
    }

    /// The results of a reply we expect to have succeeded.
    fn results(datagram: &[u8]) -> Vec<u8> {
        let reply = Reply::parse(datagram).expect("a reply must decode");
        reply.results().expect("a successful reply").to_vec()
    }

    fn nfs(dispatcher: &Dispatcher, request: &nfs2::Request) -> nfs2::Response<'static> {
        let datagram = dispatcher
            .answer(
                Service::Nfs,
                &call(
                    Service::Nfs,
                    request.procedure().0,
                    &request.encode_arguments(),
                ),
                PEER,
            )
            .expect("an RPC call is answered");
        // Owned so the borrow of the datagram ends here; nothing below looks
        // at a `READ` payload.
        let results = results(&datagram);
        let parsed = nfs2::Response::parse(request.procedure(), &results)
            .expect("our own reply must decode");
        match parsed {
            nfs2::Response::Null => nfs2::Response::Null,
            nfs2::Response::Attr(attr) => nfs2::Response::Attr(attr),
            nfs2::Response::Lookup(found) => nfs2::Response::Lookup(found),
            nfs2::Response::Read(read) => nfs2::Response::Read(read.map(|read| FileData {
                attr: read.attr,
                data: &[],
            })),
            nfs2::Response::ReadDir(listing) => nfs2::Response::ReadDir(listing),
            nfs2::Response::StatFs(stat) => nfs2::Response::StatFs(stat),
        }
    }

    fn status(response: &nfs2::Response<'_>) -> Status {
        response.status()
    }

    fn handle(path: &str) -> FileHandle {
        Vfs::handle_for(path)
    }

    // -- portmap ----------------------------------------------------------

    /// `S10j-serve-to-cdj` frame 1, verbatim: the first call a deck makes when
    /// it decides to look at our media, and the gate on everything after it
    /// (F46).
    const CAPTURED_GETPORT_MOUNTD: &str = "\
        00000fe90000000000000002000186a000000002000000030000000100000014\
        14b7e60a000000000000000000000000000000000000000000000000000186a5\
        000000010000001100000000";

    /// The same session's second call, one millisecond later.
    const CAPTURED_GETPORT_NFSD: &str = "\
        00000fea0000000000000002000186a000000002000000030000000100000014\
        5034ae03000000000000000000000000000000000000000000000000000186a3\
        000000020000001100000000";

    #[test]
    fn a_real_deck_asking_for_mountd_is_told_where_ours_is() {
        let dispatcher = served();
        let reply = dispatcher
            .answer(Service::Portmap, &hex(CAPTURED_GETPORT_MOUNTD), PEER)
            .expect("a GETPORT is answered");
        assert_eq!(
            portmap::Response::parse(portmap::Proc::GETPORT, &results(&reply)).unwrap(),
            portmap::Response::GetPort(Some(mount::PIONEER_PORT))
        );
        assert_eq!(
            Reply::parse(&reply).unwrap().xid(),
            Xid(0xfe9),
            "the xid is echoed, because that is all it is for"
        );
    }

    #[test]
    fn a_real_deck_asking_for_nfsd_is_told_where_ours_is() {
        let dispatcher = served();
        let reply = dispatcher
            .answer(Service::Portmap, &hex(CAPTURED_GETPORT_NFSD), PEER)
            .expect("a GETPORT is answered");
        assert_eq!(
            portmap::Response::parse(portmap::Proc::GETPORT, &results(&reply)).unwrap(),
            portmap::Response::GetPort(Some(nfs2::PORT))
        );
    }

    /// `S24c-e9-noportmap`: with nothing on 111 a deck resent this identical
    /// datagram 30 times, once a second, and never tried the well-known ports.
    /// Answering it twice must give the same bytes twice — see the module
    /// documentation on why there is no duplicate-request cache.
    #[test]
    fn a_retransmitted_call_is_answered_identically() {
        let retransmitted = hex(
            "000001170000000000000002000186a000000002000000030000000100000014\
             c0e93b1d000000000000000000000000000000000000000000000000000186a5\
             000000010000001100000000",
        );
        let dispatcher = served();
        let first = dispatcher.answer(Service::Portmap, &retransmitted, PEER);
        let second = dispatcher.answer(Service::Portmap, &retransmitted, PEER);
        assert_eq!(first, second, "every procedure we answer is idempotent");
        assert!(first.is_some());
    }

    #[test]
    fn a_program_we_do_not_run_is_not_registered_rather_than_an_error() {
        let dispatcher = served();
        let asked =
            portmap::Request::GetPort(portmap::Mapping::query(Program::STATUS, 1, IpProtocol::UDP));
        let reply = dispatcher
            .answer(
                Service::Portmap,
                &call(
                    Service::Portmap,
                    portmap::Proc::GETPORT.0,
                    &asked.encode_arguments(),
                ),
                PEER,
            )
            .unwrap();
        assert_eq!(
            portmap::Response::parse(portmap::Proc::GETPORT, &results(&reply)).unwrap(),
            portmap::Response::GetPort(None),
            "zero is the wire's own 'not registered'"
        );
    }

    #[test]
    fn nothing_here_is_registered_on_tcp() {
        let dispatcher = served();
        let asked =
            portmap::Request::GetPort(portmap::Mapping::query(Program::NFS, 2, IpProtocol::TCP));
        let reply = dispatcher
            .answer(
                Service::Portmap,
                &call(
                    Service::Portmap,
                    portmap::Proc::GETPORT.0,
                    &asked.encode_arguments(),
                ),
                PEER,
            )
            .unwrap();
        assert_eq!(
            portmap::Response::parse(portmap::Proc::GETPORT, &results(&reply)).unwrap(),
            portmap::Response::GetPort(None),
            "sending a player to 2049/tcp would send it somewhere silent"
        );
    }

    #[test]
    fn a_dump_publishes_the_table_a_cdj_publishes() {
        let dispatcher = served();
        let reply = dispatcher
            .answer(
                Service::Portmap,
                &call(Service::Portmap, portmap::Proc::DUMP.0, &[]),
                PEER,
            )
            .unwrap();
        assert_eq!(
            portmap::Response::parse(portmap::Proc::DUMP, &results(&reply)).unwrap(),
            portmap::Response::Dump(
                portmap::cdj_registrations(portmap::PORT, mount::PIONEER_PORT, nfs2::PORT).to_vec()
            ),
        );
    }

    #[test]
    fn registering_a_mapping_is_not_something_a_stranger_may_do() {
        let dispatcher = served();
        let reply = dispatcher
            .answer(
                Service::Portmap,
                &call(Service::Portmap, portmap::Proc::SET.0, &[0; 16]),
                PEER,
            )
            .unwrap();
        assert!(matches!(
            Reply::parse(&reply).unwrap(),
            Reply::Accepted {
                status: Accepted::Failed(FailureStat::PROC_UNAVAIL),
                ..
            }
        ));
    }

    #[test]
    fn a_null_call_succeeds_with_no_results_at_all() {
        let dispatcher = served();
        for (service, procedure) in [
            (Service::Portmap, portmap::Proc::NULL.0),
            (Service::Mount, mount::Proc::NULL.0),
            (Service::Nfs, nfs2::Proc::NULL.0),
        ] {
            let reply = dispatcher
                .answer(service, &call(service, procedure, &[]), PEER)
                .unwrap();
            assert_eq!(reply.len(), 24, "{service:?}: header and nothing else");
            assert_eq!(Reply::parse(&reply).unwrap().results(), Some([].as_slice()));
        }
    }

    // -- mountd -----------------------------------------------------------

    /// `S10j-serve-to-cdj` frame 3: the only mountd call a real deck makes.
    /// The path is six bytes of UTF-16LE for three characters.
    const CAPTURED_MNT_USB: &str = "\
        00000feb0000000000000002000186a500000001000000010000000100000014\
        05de9b1c00000000000000000000000000000000000000000000000000000006\
        2f0043002f000011";

    /// `S18-two-slots` frame 5, where the same deck mounted both slots: the SD
    /// export differs from the USB one by a single character.
    const CAPTURED_MNT_SD: &str = "\
        000021760000000000000002000186a500000001000000010000000100000014\
        94cab30600000000000000000000000000000000000000000000000000000006\
        2f0042002f000011";

    #[test]
    fn a_real_mnt_of_the_usb_export_returns_that_mediums_subtree() {
        let dispatcher = served();
        let reply = dispatcher
            .answer(Service::Mount, &hex(CAPTURED_MNT_USB), PEER)
            .expect("a MNT is answered");
        assert_eq!(
            mount::Response::parse(mount::Proc::MNT, &results(&reply)).unwrap(),
            mount::Response::Mnt(Ok(handle("/C"))),
            "the handle for the medium's own subtree, not for the tree's root",
        );
    }

    #[test]
    fn two_slots_mounted_from_one_deck_get_different_handles() {
        // A handle is a hash of its path and a deck keeps only twelve bytes of
        // it (F28), so two media sharing a root would be indistinguishable.
        let dispatcher = both_slots();
        let usb = dispatcher
            .answer(Service::Mount, &hex(CAPTURED_MNT_USB), PEER)
            .unwrap();
        let sd = dispatcher
            .answer(Service::Mount, &hex(CAPTURED_MNT_SD), PEER)
            .unwrap();
        let root =
            |datagram: &[u8]| match mount::Response::parse(mount::Proc::MNT, &results(datagram))
                .unwrap()
            {
                mount::Response::Mnt(Ok(handle)) => handle,
                other => panic!("expected a root handle, got {other:?}"),
            };
        assert_eq!(root(&usb), handle("/C"));
        assert_eq!(root(&sd), handle("/B"));
        assert_ne!(root(&usb).key(), root(&sd).key());
    }

    #[test]
    fn an_export_path_is_matched_on_its_prefix() {
        // C6: the same player mounted `/C/` on one peer and `/C/EXPORT` on
        // another in one session.
        let dispatcher = served();
        for path in ["/C/", "/C/EXPORT"] {
            let request = mount::Request::Mnt(Utf16LeString::new(path));
            let reply = dispatcher
                .answer(
                    Service::Mount,
                    &call(
                        Service::Mount,
                        mount::Proc::MNT.0,
                        &request.encode_arguments(),
                    ),
                    PEER,
                )
                .unwrap();
            assert_eq!(
                mount::Response::parse(mount::Proc::MNT, &results(&reply)).unwrap(),
                mount::Response::Mnt(Ok(handle("/C"))),
                "{path} names the USB slot",
            );
        }
    }

    #[test]
    fn a_slot_with_no_medium_in_it_is_noent() {
        // `served()` has a USB stick and no SD card. An empty slot is an
        // ordinary state, not a failure.
        let dispatcher = served();
        let reply = dispatcher
            .answer(Service::Mount, &hex(CAPTURED_MNT_SD), PEER)
            .unwrap();
        assert_eq!(
            mount::Response::parse(mount::Proc::MNT, &results(&reply)).unwrap(),
            mount::Response::Mnt(Err(ErrorStatus::NOENT))
        );
    }

    #[test]
    fn an_export_we_have_never_heard_of_is_noent() {
        let dispatcher = served();
        for path in ["/A/", "", "/C", "sausage"] {
            let request = mount::Request::Mnt(Utf16LeString::new(path));
            let reply = dispatcher
                .answer(
                    Service::Mount,
                    &call(
                        Service::Mount,
                        mount::Proc::MNT.0,
                        &request.encode_arguments(),
                    ),
                    PEER,
                )
                .unwrap();
            assert_eq!(
                mount::Response::parse(mount::Proc::MNT, &results(&reply)).unwrap(),
                mount::Response::Mnt(Err(ErrorStatus::NOENT)),
                "{path:?} is not an export",
            );
        }
    }

    #[test]
    fn an_eject_takes_the_mount_out_of_the_dump() {
        let dispatcher = both_slots();
        for captured in [CAPTURED_MNT_USB, CAPTURED_MNT_SD] {
            dispatcher
                .answer(Service::Mount, &hex(captured), PEER)
                .unwrap();
        }
        assert_eq!(dispatcher.mounts().len(), 2, "one per slot");

        // C9: ejecting SD then USB produced `UMNT('/B/')` then `UMNT('/C/')`
        // twelve seconds apart.
        let umnt = mount::Request::Umnt(Utf16LeString::new(mount::EXPORT_SD));
        let reply = dispatcher
            .answer(
                Service::Mount,
                &call(
                    Service::Mount,
                    mount::Proc::UMNT.0,
                    &umnt.encode_arguments(),
                ),
                PEER,
            )
            .unwrap();
        assert_eq!(results(&reply), Vec::new(), "UMNT returns nothing at all");
        assert_eq!(
            dispatcher.mounts(),
            vec![Mount {
                peer: PEER,
                export: "/C/".to_owned()
            }]
        );

        let dump = dispatcher
            .answer(
                Service::Mount,
                &call(Service::Mount, mount::Proc::DUMP.0, &[]),
                PEER,
            )
            .unwrap();
        assert_eq!(
            mount::Response::parse(mount::Proc::DUMP, &results(&dump)).unwrap(),
            mount::Response::Dump(vec![mount::MountEntry {
                hostname: "169.254.202.84".to_owned(),
                directory: Utf16LeString::new("/C/"),
            }])
        );
    }

    #[test]
    fn a_peer_looping_on_mnt_cannot_grow_the_registry_without_bound() {
        let dispatcher = both_slots();
        for octet in 0..40u8 {
            let peer = Ipv4Addr::new(169, 254, 0, octet);
            dispatcher
                .answer(Service::Mount, &hex(CAPTURED_MNT_USB), peer)
                .unwrap();
        }
        assert_eq!(dispatcher.mounts().len(), MAX_MOUNTS);
    }

    #[test]
    fn an_export_reply_carries_a_utf16le_path_and_ascii_groups() {
        // C7, the correction a capture forced: one structure, two encodings.
        // No real deck ever calls this (F37); a client enumerating us does.
        let dispatcher = both_slots();
        let reply = dispatcher
            .answer(
                Service::Mount,
                &call(Service::Mount, mount::Proc::EXPORT.0, &[]),
                PEER,
            )
            .unwrap();
        let results = results(&reply);
        assert_eq!(
            mount::Response::parse(mount::Proc::EXPORT, &results).unwrap(),
            mount::Response::Export(vec![
                mount::Export::new(mount::EXPORT_SD, &[mount::Export::LINK_LOCAL_SUBNET]),
                mount::Export::new(mount::EXPORT_USB, &[mount::Export::LINK_LOCAL_SUBNET]),
            ])
        );
        assert!(
            results
                .windows(23)
                .any(|window| window == mount::Export::LINK_LOCAL_SUBNET.as_bytes()),
            "the group is one byte per character, not UTF-16LE",
        );
    }

    #[test]
    fn an_empty_slot_is_not_offered_in_the_export_list() {
        let dispatcher = served();
        let reply = dispatcher
            .answer(
                Service::Mount,
                &call(Service::Mount, mount::Proc::EXPORT.0, &[]),
                PEER,
            )
            .unwrap();
        assert_eq!(
            mount::Response::parse(mount::Proc::EXPORT, &results(&reply)).unwrap(),
            mount::Response::Export(vec![mount::Export::new(
                mount::EXPORT_USB,
                &[mount::Export::LINK_LOCAL_SUBNET]
            )])
        );
    }

    // -- nfsd -------------------------------------------------------------

    /// The twenty bytes a CDJ-2000NXS wrote over the tail of our root handle,
    /// from `S10j-serve-to-cdj` frame 4. Three 32-bit words that read as
    /// `[self, parent, mount-root]` in the deck's own namespace, then its file
    /// reference (F28).
    const DECK_TAIL: [u8; 20] = [
        0x03, 0x01, 0x2d, 0x00, 0x00, 0x00, 0x1b, 0x58, 0x00, 0x00, 0x00, 0x00, 0x03, 0x03, 0x01,
        0x00, 0x00, 0x00, 0x00, 0xb6,
    ];

    /// A handle as a deck would send it back: our twelve bytes, its twenty.
    fn rewritten(path: &str) -> FileHandle {
        let mut bytes = handle(path).0;
        bytes[FileHandle::KEY_LEN..].copy_from_slice(&DECK_TAIL);
        FileHandle(bytes)
    }

    #[test]
    fn a_lookup_from_a_handle_the_deck_rewrote_still_resolves() {
        // The bug that browses perfectly and fails at exactly the moment a DJ
        // loads a track (F28).
        let dispatcher = served();
        let served_handle = handle("/C");
        assert_ne!(
            served_handle.as_bytes(),
            rewritten("/C").as_bytes(),
            "the bytes really are different",
        );
        let found = nfs(
            &dispatcher,
            &nfs2::Request::Lookup {
                dir: rewritten("/C"),
                name: Utf16LeString::new("Contents"),
            },
        );
        let nfs2::Response::Lookup(Ok(found)) = found else {
            panic!("a rewritten handle must still resolve, got {found:?}");
        };
        assert_eq!(found.handle, handle("/C/Contents"));
        assert!(found.attr.is_directory());
    }

    /// `S10j-serve-to-cdj` frame 4, re-aimed at our tree: the deck's own
    /// credential, its own UTF-16LE name, and its own rewritten handle tail,
    /// with only the twelve bytes it preserved replaced by ours.
    const CAPTURED_LOOKUP_CONTENTS: &str = "\
        00000fec0000000000000002000186a300000002000000040000000100000014\
        cc2681250000000000000000000000000000000000000000000000008a5edab2\
        82632443219e051e03012d0000001b580000000003030100000000b600000010\
        43006f006e00740065006e0074007300";

    /// Re-aim a captured call at our own tree, keeping the twenty bytes the
    /// deck rewrote and replacing the twelve it preserved.
    fn re_aimed(captured: &str, ours: FileHandle) -> Vec<u8> {
        let datagram = hex(captured);
        let parsed = Call::parse(&datagram).expect("a captured call must decode");
        let request = nfs2::Request::parse(nfs2::Proc(parsed.procedure), parsed.arguments)
            .expect("captured arguments must decode");
        let graft = |theirs: FileHandle| {
            let mut bytes = ours.0;
            bytes[FileHandle::KEY_LEN..].copy_from_slice(&theirs.0[FileHandle::KEY_LEN..]);
            FileHandle(bytes)
        };
        let request = match request {
            nfs2::Request::Lookup { dir, name } => nfs2::Request::Lookup {
                dir: graft(dir),
                name,
            },
            nfs2::Request::GetAttr(handle) => nfs2::Request::GetAttr(graft(handle)),
            nfs2::Request::Read(args) => nfs2::Request::Read(ReadArgs {
                handle: graft(args.handle),
                ..args
            }),
            other => other,
        };
        let arguments = request.encode_arguments();
        Call::new(
            parsed.xid,
            parsed.program,
            parsed.version,
            parsed.procedure,
            parsed.credential,
            &arguments,
        )
        .encode()
    }

    #[test]
    fn a_real_lookup_walks_one_component() {
        let dispatcher = served();
        let reply = dispatcher
            .answer(
                Service::Nfs,
                &re_aimed(CAPTURED_LOOKUP_CONTENTS, handle("/C")),
                PEER,
            )
            .expect("a LOOKUP is answered");
        let results = results(&reply);
        assert_eq!(
            results.len(),
            4 + FileHandle::LEN + nfs2::Fattr::WIRE_LEN,
            "a status, a bare filehandle and seventeen words",
        );
        let nfs2::Response::Lookup(Ok(found)) =
            nfs2::Response::parse(nfs2::Proc::LOOKUP, &results).unwrap()
        else {
            panic!("expected a resolved name");
        };
        assert_eq!(found.handle, handle("/C/Contents"));
        assert_eq!(
            found.attr.fileid,
            found.handle.fileid(),
            "the fileid is the handle's leading word, as a deck's is in 8285 of 8285",
        );
    }

    #[test]
    fn a_name_differing_only_in_case_resolves_to_the_name_as_stored() {
        // `export.pdb` says `Gesaffelstein` where the directory is
        // `GESAFFELSTEIN`; a FAT32 driver does not notice and neither may we.
        // The handle must be the stored name's, or every later use is STALE.
        let dispatcher = served();
        let found = nfs(
            &dispatcher,
            &nfs2::Request::Lookup {
                dir: rewritten("/C/Contents"),
                name: Utf16LeString::new("Gesaffelstein"),
            },
        );
        let nfs2::Response::Lookup(Ok(found)) = found else {
            panic!("a folded name must resolve, got {found:?}");
        };
        assert_eq!(found.handle, handle("/C/Contents/GESAFFELSTEIN"));
    }

    #[test]
    fn a_name_differing_only_in_normalisation_resolves() {
        // The pdb stores NFC where the filesystem reports NFD, because
        // rekordbox wrote the two through different APIs.
        let dispatcher = served();
        let composed = "\u{30ac}\u{30ac}\u{30df}.mp3";
        let found = nfs(
            &dispatcher,
            &nfs2::Request::Lookup {
                dir: handle("/C/Contents"),
                name: Utf16LeString::new(composed),
            },
        );
        assert_eq!(status(&found), Status::OK, "NFC must find NFD");
    }

    #[test]
    fn a_missing_name_is_noent_and_an_unknown_directory_is_stale() {
        // Two different instructions to a deck: mount again, or the file is
        // gone. `Vfs` answers `None` to both.
        let dispatcher = served();
        let missing = nfs(
            &dispatcher,
            &nfs2::Request::Lookup {
                dir: handle("/C"),
                name: Utf16LeString::new("nothing here"),
            },
        );
        assert_eq!(status(&missing), Status::NOENT);

        let stale = nfs(
            &dispatcher,
            &nfs2::Request::Lookup {
                dir: handle("/nowhere"),
                name: Utf16LeString::new("Contents"),
            },
        );
        assert_eq!(status(&stale), Status::STALE);
    }

    #[test]
    fn a_lookup_inside_a_file_is_notdir() {
        let dispatcher = served();
        let response = nfs(
            &dispatcher,
            &nfs2::Request::Lookup {
                dir: handle("/C/PIONEER/rekordbox/export.pdb"),
                name: Utf16LeString::new("anything"),
            },
        );
        assert_eq!(status(&response), Status::NOTDIR);
    }

    #[test]
    fn dot_and_dot_dot_navigate_even_though_no_deck_asks() {
        let dispatcher = served();
        let resolve = |dir: &str, name: &str| {
            let response = nfs(
                &dispatcher,
                &nfs2::Request::Lookup {
                    dir: handle(dir),
                    name: Utf16LeString::new(name),
                },
            );
            match response {
                nfs2::Response::Lookup(Ok(found)) => found.handle,
                other => panic!("{dir} / {name}: {other:?}"),
            }
        };
        assert_eq!(resolve("/C/Contents", "."), handle("/C/Contents"));
        assert_eq!(resolve("/C/Contents", ".."), handle("/C"));
        assert_eq!(
            resolve("/", ".."),
            handle("/"),
            "the root is its own parent"
        );
    }

    /// `S10j-serve-to-cdj` frame 8: the deck asks for the last 160 bytes of a
    /// 6 942 380-byte MP3 before it asks for the first — F18's read of the very
    /// last byte, which is where a container's trailing tag lives.
    const CAPTURED_READ_TAIL: &str = "\
        00000ff10000000000000002000186a300000002000000060000000100000014\
        1db3622d000000000000000000000000000000000000000000000000a04e7747\
        2d06cafead52b8afa9277dfdcca4192725c8b0032084a017d6cb1ed70069ee0c\
        000000a000000000";

    #[test]
    fn a_read_that_runs_off_the_end_is_short_rather_than_an_error() {
        let dispatcher = served();
        let track = handle("/C/Contents/GESAFFELSTEIN/track.mp3");
        // The captured call asks for 160 bytes at 6 942 220; our file is
        // 20 000 bytes, so the same shape lands past its end.
        let datagram = re_aimed(CAPTURED_READ_TAIL, track);
        let reply = dispatcher
            .answer(Service::Nfs, &datagram, PEER)
            .expect("a READ is answered");
        let past_the_end = results(&reply);
        let nfs2::Response::Read(Ok(read)) =
            nfs2::Response::parse(nfs2::Proc::READ, &past_the_end).unwrap()
        else {
            panic!("a read past the end is not an error");
        };
        assert!(read.data.is_empty(), "nothing there to return");
        assert_eq!(
            read.attr.size, 20_000,
            "and the size still says where it ends"
        );

        // The same call inside the file returns the bytes, and the last read
        // of all is a short one.
        let args = ReadArgs::at(track, 19_900, 160).unwrap();
        let reply = dispatcher
            .answer(
                Service::Nfs,
                &call(
                    Service::Nfs,
                    nfs2::Proc::READ.0,
                    &nfs2::Request::Read(args).encode_arguments(),
                ),
                PEER,
            )
            .unwrap();
        let tail = results(&reply);
        let nfs2::Response::Read(Ok(read)) =
            nfs2::Response::parse(nfs2::Proc::READ, &tail).unwrap()
        else {
            panic!("expected bytes");
        };
        assert_eq!(read.data.len(), 100, "short, and not the end of the story");
    }

    #[test]
    fn a_read_spans_a_file_byte_for_byte() {
        let dispatcher = served();
        let track = handle("/C/Contents/GESAFFELSTEIN/track.mp3");
        let expected: Vec<u8> = (0..=255u8).cycle().take(20_000).collect();
        let mut assembled = Vec::new();
        let mut offset = 0u64;
        // 8192 is what a real CDJ asks for, relying on IP fragmentation.
        while offset < 20_000 {
            let args = ReadArgs::at(track, offset, 8192).unwrap();
            let reply = dispatcher
                .answer(
                    Service::Nfs,
                    &call(
                        Service::Nfs,
                        nfs2::Proc::READ.0,
                        &nfs2::Request::Read(args).encode_arguments(),
                    ),
                    PEER,
                )
                .unwrap();
            let results = results(&reply);
            let nfs2::Response::Read(Ok(read)) =
                nfs2::Response::parse(nfs2::Proc::READ, &results).unwrap()
            else {
                panic!("expected bytes at {offset}");
            };
            assert!(!read.data.is_empty(), "no progress at {offset}");
            assembled.extend_from_slice(read.data);
            offset += u64::try_from(read.data.len()).unwrap();
        }
        assert_eq!(assembled, expected);
    }

    #[test]
    fn a_read_is_capped_at_a_reply_every_platform_can_send() {
        // A client may ask for anything, including the 9408 and 28584 a deck
        // asks another deck for. macOS will not send a datagram past 9216
        // bytes, and a reply that cannot be sent is a stall where a short read
        // is an ordinary "ask for the rest".
        assert_eq!(usize::try_from(MAX_READ), Ok(nfs2::MAX_DATA));
        let dispatcher = served();
        for count in [9408u32, 28_584, u32::MAX] {
            let args =
                ReadArgs::at(handle("/C/Contents/GESAFFELSTEIN/track.mp3"), 0, count).unwrap();
            let reply = dispatcher
                .answer(
                    Service::Nfs,
                    &call(
                        Service::Nfs,
                        nfs2::Proc::READ.0,
                        &nfs2::Request::Read(args).encode_arguments(),
                    ),
                    PEER,
                )
                .unwrap();
            assert!(
                reply.len() <= 9216,
                "a {count}-byte read produced {} bytes, which macOS will not send",
                reply.len(),
            );
        }
    }

    #[test]
    fn a_directory_cannot_be_read_and_a_stranger_is_stale() {
        let dispatcher = served();
        let directory = nfs(
            &dispatcher,
            &nfs2::Request::Read(ReadArgs::at(handle("/C"), 0, 512).unwrap()),
        );
        assert_eq!(status(&directory), Status::ISDIR);

        let stranger = nfs(
            &dispatcher,
            &nfs2::Request::Read(ReadArgs::at(handle("/nowhere"), 0, 512).unwrap()),
        );
        assert_eq!(status(&stranger), Status::STALE);
    }

    /// `S10j-serve-to-cdj` frame 7, re-aimed: the one `GETATTR` in that
    /// session, which the deck sends before its first read.
    const CAPTURED_GETATTR: &str = "\
        00000ff00000000000000002000186a300000002000000010000000100000014\
        8ef85226000000000000000000000000000000000000000000000000a04e7747\
        2d06cafead52b8afa9277dfdcca4192725c8b0032084a017d6cb1ed7";

    #[test]
    fn a_real_getattr_is_answered_with_seventeen_words() {
        let dispatcher = served();
        let track = handle("/C/Contents/GESAFFELSTEIN/track.mp3");
        let reply = dispatcher
            .answer(Service::Nfs, &re_aimed(CAPTURED_GETATTR, track), PEER)
            .expect("a GETATTR is answered");
        let results = results(&reply);
        assert_eq!(results.len(), 4 + nfs2::Fattr::WIRE_LEN);
        let nfs2::Response::Attr(Ok(attr)) =
            nfs2::Response::parse(nfs2::Proc::GETATTR, &results).unwrap()
        else {
            panic!("expected attributes");
        };
        assert!(attr.is_regular_file());
        assert_eq!(attr.size, 20_000, "the one field that is load-bearing");
        assert_eq!(attr.blocks, 20_000_u32.div_ceil(512));
        assert_eq!(attr.fileid, track.fileid());
    }

    /// The seventeen words the reference server put in a `LOOKUP` reply for the
    /// `Contents` directory in `S10j-serve-to-cdj`.
    const CAPTURED_DIRECTORY_FATTR: &str = "\
        00000002000041ed000000020000000000000000000000000000020000000000\
        00000000000000010000009a5f5e1000000000005f5e1000000000005f5e1000\
        00000000";

    /// And in the `GETATTR` reply for the 6 942 380-byte MP3 the deck then
    /// played from it.
    const CAPTURED_FILE_FATTR: &str = "\
        00000001000081a40000000100000000000000000069eeac0000020000000000\
        000034f8000000010000009e5f5e1000000000005f5e1000000000005f5e1000\
        00000000";

    /// A whole load and thirty seconds of playback went through these, so they
    /// are not a plausible reading of RFC 1094 — they are what a CDJ-2000NXS
    /// has been observed to accept, and byte-identical replies keep a capture
    /// of this server diffable against that session.
    #[test]
    fn our_attributes_are_the_ones_a_deck_played_from_word_for_word() {
        let root = std::env::temp_dir().join(format!("prolink-nfs-attr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Contents")).unwrap();
        let file = std::fs::File::create(root.join("Contents/track.mp3")).unwrap();
        file.set_len(0x0069_eeac).unwrap();
        drop(file);
        let mut vfs = Vfs::new();
        vfs.mount("C", &root).unwrap();
        let dispatcher = Dispatcher::new(Arc::new(RwLock::new(vfs)), PORTS);

        let compare = |path: &str, captured: &str, what: &str| {
            let handle = handle(path);
            let response = nfs(&dispatcher, &nfs2::Request::GetAttr(handle));
            let ours = response.encode();
            let captured = hex(captured);
            assert_eq!(captured.len(), nfs2::Fattr::WIRE_LEN);
            assert_eq!(ours.len(), 4 + captured.len(), "{what}");
            // Word ten is the fileid, and it is the one place we follow the
            // hardware rather than the reference: a deck derives it from the
            // handle's leading word in 8285 of 8285 replies, where the
            // reference server used a counter.
            assert_eq!(
                ours.get(4..44),
                captured.get(..40),
                "{what}, before the fileid"
            );
            assert_eq!(ours.get(48..), captured.get(44..), "{what}, after it");
            let nfs2::Response::Attr(Ok(attr)) = response else {
                panic!("{what}: expected attributes");
            };
            assert_eq!(attr.fileid, handle.fileid());
        };
        compare("/C/Contents", CAPTURED_DIRECTORY_FATTR, "a directory");
        compare("/C/Contents/track.mp3", CAPTURED_FILE_FATTR, "a file");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_getattr_of_a_handle_we_never_issued_is_stale() {
        let dispatcher = served();
        let response = nfs(&dispatcher, &nfs2::Request::GetAttr(handle("/nowhere")));
        assert_eq!(status(&response), Status::STALE);
    }

    #[test]
    fn a_file_too_large_for_nfsv2_is_refused_rather_than_wrapped() {
        // `fattr.size` is 32 bits, so a 5 GiB file cannot be described. A
        // clamped size would be read to 4 GiB and no further, which presents
        // as a truncated track rather than as an error.
        let root = std::env::temp_dir().join(format!("prolink-nfs-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let huge = root.join("huge.wav");
        let Ok(file) = std::fs::File::create(&huge) else {
            return;
        };
        // Sparse: no filesystem this runs on allocates blocks for it.
        if file.set_len(5 << 30).is_err() {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        drop(file);

        let mut vfs = Vfs::new();
        vfs.mount("C", &root).unwrap();
        let dispatcher = Dispatcher::new(Arc::new(RwLock::new(vfs)), PORTS);
        let response = nfs(&dispatcher, &nfs2::Request::GetAttr(handle("/C/huge.wav")));
        assert_eq!(status(&response), Status::FBIG);
        let read = nfs(
            &dispatcher,
            &nfs2::Request::Read(ReadArgs::at(handle("/C/huge.wav"), 0, 8192).unwrap()),
        );
        assert_eq!(status(&read), Status::FBIG);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_readdir_lists_a_directory_and_a_cookie_resumes_it() {
        // No deck has ever called this — it walks paths it read out of
        // `export.pdb` — but a client with no database has no other way in.
        let dispatcher = served();
        let page = |cookie: Cookie, count: u32| {
            let response = nfs(
                &dispatcher,
                &nfs2::Request::ReadDir(ReadDirArgs {
                    handle: handle("/C"),
                    cookie,
                    count,
                }),
            );
            match response {
                nfs2::Response::ReadDir(Ok(listing)) => listing,
                other => panic!("expected a listing, got {other:?}"),
            }
        };

        let whole = page(Cookie::START, 8192);
        let names: Vec<String> = whole
            .entries
            .iter()
            .map(|entry| entry.name.to_string_lossy())
            .collect();
        assert_eq!(names, ["PIONEER", "Contents"], "in the tree's own order");
        assert!(whole.eof);
        assert_eq!(
            whole.entries.first().map(|entry| entry.fileid),
            Some(handle("/C/PIONEER").fileid()),
            "an entry's fileid is the one its own attributes will report",
        );

        // A budget that fits one entry stops short, and its cookie resumes.
        let first = page(Cookie::START, 40);
        assert_eq!(first.entries.len(), 1);
        assert!(!first.eof, "there is more to come");
        let cookie = first.entries.first().unwrap().cookie;
        let rest = page(cookie, 8192);
        assert_eq!(
            rest.entries
                .iter()
                .map(|entry| entry.name.to_string_lossy())
                .collect::<Vec<_>>(),
            ["Contents"],
        );
        assert!(rest.eof);

        // A cookie past the end is an empty listing, not an error.
        let past = page(Cookie(99u32.to_be_bytes()), 8192);
        assert!(past.entries.is_empty() && past.eof);
    }

    #[test]
    fn a_readdir_of_a_file_is_notdir() {
        let dispatcher = served();
        let response = nfs(
            &dispatcher,
            &nfs2::Request::ReadDir(ReadDirArgs {
                handle: handle("/C/PIONEER/rekordbox/export.pdb"),
                cookie: Cookie::START,
                count: 8192,
            }),
        );
        assert_eq!(status(&response), Status::NOTDIR);
    }

    #[test]
    fn statfs_answers_for_a_handle_we_issued_and_not_for_one_we_did_not() {
        let dispatcher = served();
        let response = nfs(&dispatcher, &nfs2::Request::StatFs(handle("/C")));
        assert_eq!(
            response,
            nfs2::Response::StatFs(Ok(STATFS)),
            "RFC 1094 as written; nothing has ever asked",
        );
        let stale = nfs(&dispatcher, &nfs2::Request::StatFs(handle("/nowhere")));
        assert_eq!(status(&stale), Status::STALE);
    }

    #[test]
    fn every_procedure_that_would_write_is_proc_unavail() {
        let dispatcher = served();
        for procedure in [
            nfs2::Proc::SETATTR,
            nfs2::Proc::ROOT,
            nfs2::Proc::READLINK,
            nfs2::Proc::WRITECACHE,
            nfs2::Proc::WRITE,
            nfs2::Proc::CREATE,
            nfs2::Proc::REMOVE,
            nfs2::Proc::RENAME,
            nfs2::Proc::LINK,
            nfs2::Proc::SYMLINK,
            nfs2::Proc::MKDIR,
            nfs2::Proc::RMDIR,
            nfs2::Proc(99),
        ] {
            let reply = dispatcher
                .answer(
                    Service::Nfs,
                    &call(Service::Nfs, procedure.0, &[0; 44]),
                    PEER,
                )
                .unwrap();
            assert!(
                matches!(
                    Reply::parse(&reply).unwrap(),
                    Reply::Accepted {
                        status: Accepted::Failed(FailureStat::PROC_UNAVAIL),
                        ..
                    }
                ),
                "{procedure:?} is not something we do",
            );
        }
    }

    // -- the RPC layer itself ---------------------------------------------

    #[test]
    fn a_call_for_a_program_this_port_does_not_run_is_prog_unavail() {
        let dispatcher = served();
        // An NFS call that arrived on the mountd socket.
        let reply = dispatcher
            .answer(
                Service::Mount,
                &call(Service::Nfs, nfs2::Proc::NULL.0, &[]),
                PEER,
            )
            .unwrap();
        assert!(matches!(
            Reply::parse(&reply).unwrap(),
            Reply::Accepted {
                status: Accepted::Failed(FailureStat::PROG_UNAVAIL),
                ..
            }
        ));
    }

    #[test]
    fn a_call_for_a_version_we_do_not_speak_carries_the_range_we_do() {
        let dispatcher = served();
        let credential = AuthUnix::cdj(0).encode();
        let raw = Call::new(
            Xid(1),
            Program::NFS,
            3,
            nfs2::Proc::NULL.0,
            Auth::unix(&credential),
            &[],
        )
        .encode();
        let reply = dispatcher.answer(Service::Nfs, &raw, PEER).unwrap();
        assert!(
            matches!(
                Reply::parse(&reply).unwrap(),
                Reply::Accepted {
                    status: Accepted::ProgMismatch { low: 2, high: 2 },
                    ..
                }
            ),
            "PROG_MISMATCH is 'not that version', which is not PROG_UNAVAIL",
        );
    }

    #[test]
    fn arguments_that_do_not_decode_are_garbage_args() {
        let dispatcher = served();
        for (service, procedure) in [
            (Service::Portmap, portmap::Proc::GETPORT.0),
            (Service::Mount, mount::Proc::MNT.0),
            (Service::Nfs, nfs2::Proc::LOOKUP.0),
        ] {
            let reply = dispatcher
                .answer(service, &call(service, procedure, &[0, 0, 0, 1]), PEER)
                .unwrap();
            assert!(
                matches!(
                    Reply::parse(&reply).unwrap(),
                    Reply::Accepted {
                        status: Accepted::Failed(FailureStat::GARBAGE_ARGS),
                        ..
                    }
                ),
                "{service:?} procedure {procedure}",
            );
        }
    }

    #[test]
    fn a_datagram_that_is_not_a_call_is_not_answered() {
        // Our ports see strays, and a reply that wandered in is not something
        // to reply to. Answering would make two servers argue forever.
        let dispatcher = served();
        assert_eq!(
            dispatcher.answer(Service::Nfs, &Reply::success(Xid(1), &[]).encode(), PEER),
            None,
        );
        assert_eq!(dispatcher.answer(Service::Nfs, &[], PEER), None);
        assert_eq!(
            dispatcher.answer(Service::Nfs, b"GET / HTTP/1.1", PEER),
            None
        );
    }

    /// Every captured datagram, mutated, truncated and re-run.
    ///
    /// This is a network input path reachable by anyone on the link, and the
    /// workspace forbids panicking outside tests precisely so a hostile
    /// datagram costs a reply and nothing else. The assertion is only that it
    /// returned.
    #[test]
    fn no_datagram_can_take_the_server_down() {
        let dispatcher = served();
        let corpus = [
            CAPTURED_GETPORT_MOUNTD,
            CAPTURED_MNT_USB,
            CAPTURED_LOOKUP_CONTENTS,
            CAPTURED_READ_TAIL,
            CAPTURED_GETATTR,
        ];
        // A full-period LCG, so a failure reproduces.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize
        };
        for text in corpus {
            let original = hex(text);
            for service in [Service::Portmap, Service::Mount, Service::Nfs] {
                for _ in 0..400 {
                    let mut mutated = original.clone();
                    match next() % 3 {
                        0 => mutated.truncate(next() % original.len().max(1)),
                        1 => {
                            let at = next() % original.len();
                            mutated[at] = u8::try_from(next() % 256).unwrap();
                        }
                        _ => {
                            let at = next() % original.len();
                            mutated[at] ^= 0xff;
                            mutated.truncate(original.len() - (next() % 8));
                        }
                    }
                    let _ = dispatcher.answer(service, &mutated, PEER);
                }
            }
        }
    }
}
