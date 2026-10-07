// SPDX-License-Identifier: GPL-3.0-only

//! The three UDP servers a real player reads our files through.
//!
//! A CDJ loading a track off a peer does not download it: it streams, seeking
//! on demand, and the samples arrive over NFSv2 `READ` while the track plays
//! (F18). So this is not a file-transfer service with a browsing feature — it
//! is the audio path, and a stall in it is an audio dropout on someone's deck.
//!
//! Three programs on three UDP ports, over one shared [`Vfs`]:
//!
//! ```text
//! portmapper  100000 v2  UDP   111   NULL, GETPORT, DUMP
//! mountd      100005 v1  UDP 48276   NULL, MNT, UMNT, UMNTALL, DUMP, EXPORT
//! nfsd        100003 v2  UDP  2049   NULL, GETATTR, LOOKUP, READ, READDIR, STATFS
//! ```
//!
//! # Port 111 is the gate on everything, and it is privileged
//!
//! With nothing on UDP/111 a deck sends `GETPORT` once a second for as long as
//! you care to watch — 31 attempts in `S24c-e9-noportmap`, no sign of giving
//! up — and **never tries 48276 or 2049**, though both were bound and idle. It
//! never opens the dbserver port query on 12523, never opens dbserver, and so
//! never lists us at all (F46).
//!
//! That failure does not look like a file-access failure. It looks like "we do
//! not appear on LINK", and the entire browse path is downstream of it. So the
//! portmapper is bound **last** — after the two ports that cannot fail for want
//! of privilege — and failing to bind it is [`Error::PrivilegedPort`], an error
//! carrying what a user can do about it on this platform, rather than a warning
//! in a log nobody reads. Linux can be told
//! `net.ipv4.ip_unprivileged_port_start=111`; macOS cannot serve 111 without
//! elevation at all.
//!
//! Asking for a portmapper anywhere else — which is what a `portmap_port` other
//! than 111 means — is allowed, because it is how the fallback question was
//! settled experimentally, and it warns at startup. [`Ports::is_discoverable`]
//! is what a caller should check before believing it is browsable.
//!
//! All three binds are **exclusive**. Everywhere else in this crate a port is
//! shared, because a DJ will have rekordbox open and both of us only listen;
//! here sharing would be silent corruption, since the kernel would hand each of
//! two servers some of a player's calls and the mount would go to one while the
//! reads went to the other. So a clash with a real `rpcbind` or `nfsd` is an
//! error, and its remedy says which of the two problems it is.
//!
//! # mountd's port is a constant that is still discovered
//!
//! 48276 is not registered to anything, but three independent observations
//! across three devices gave the same number (F6), so it looks like a Pioneer
//! constant. We take it when it is free and publish it through the portmapper
//! anyway, because that is how a deck actually finds it. Losing it to some
//! other program on the host costs only the client that skips discovery;
//! failing to bind at all would cost everything, so mountd and nfsd both fall
//! back to an ephemeral port and the portmapper publishes whatever they got.
//!
//! # What a datagram costs
//!
//! Each call is answered on its own task, and the answer is computed on a
//! blocking thread, because a `READ` of a file on a USB stick is a disk seek.
//! Doing that inline would park a runtime worker mid-`recv`, and the same
//! runtime is emitting the 200 ms status packets that keep us on the network.
//! Concurrency is capped at [`MAX_IN_FLIGHT`]: past that a datagram is dropped
//! rather than queued, which on UDP is a retransmit and not a failure.
//!
//! A load is around 20 `LOOKUP`s and 201 `READ`s, and playback seeks all over
//! the file, so the shape that matters is many small random reads at low
//! latency rather than throughput.
//!
//! A reply is capped at 8192 payload bytes, which is both RFC 1094's ceiling
//! and the largest read a deck has ever sent us — and, less obviously, the
//! largest one that is portable: macOS will not send a UDP datagram past 9216
//! bytes, so answering a deck-to-deck-sized 28584-byte read the way a deck does
//! would fail with `EMSGSIZE` and leave the caller retransmitting for ever.
//!
//! # Everything else worth knowing
//!
//! is in `nfs/answer.rs`, which turns one call into one reply and is where the
//! module documentation covers the filehandle a CDJ truncates (F28), why there
//! is no duplicate-request cache, why the export list is the tree itself, and
//! which attributes we send.

mod answer;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};

use prolink_proto::rpc::{mount, nfs2, portmap};
use tokio::net::UdpSocket;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::interface::Interface;
use crate::serve::vfs::Vfs;
use crate::socket::{self, MAX_DATAGRAM};
use crate::{Error, Result};

use answer::{Dispatcher, Service};

pub use answer::Mount;

/// How many calls may be in flight at once.
///
/// A player's own pipelining is modest — one call, one reply, a few thousand
/// times — so this is not a throughput knob but a bound on what a stranger on
/// the link can make us hold. Past it a datagram is dropped, which costs a
/// retransmit a second later rather than a failure.
pub const MAX_IN_FLIGHT: usize = 64;

/// What a user can do about a port they are not allowed to bind.
///
/// Platform-specific because the remedy is: Linux has a sysctl for exactly
/// this, and macOS has nothing of the kind.
const NOT_PERMITTED: &str = if cfg!(target_os = "linux") {
    "run as root, grant this binary CAP_NET_BIND_SERVICE, or lower the privileged range with \
     `sysctl -w net.ipv4.ip_unprivileged_port_start=111`"
} else if cfg!(target_os = "macos") {
    "macOS has no unprivileged-port setting, so serving files to a player requires running as root"
} else {
    "ports below 1024 usually require elevated privileges"
};

/// What a user can do about a port somebody else already holds.
///
/// A different problem from privilege with a different fix, and on 111 a user
/// will meet both — a desktop Linux often runs `rpcbind` by default. These
/// binds are deliberately exclusive: sharing 111 with a real portmapper would
/// let the kernel split arriving datagrams between the two servers, so a deck's
/// `GETPORT` would be answered by whichever happened to receive it and the mount
/// that followed would go to the wrong one. Failing loudly is the only outcome
/// that can be acted on.
const ALREADY_HELD: &str = "another RPC server already holds it — on Linux that is usually \
                            `rpcbind` or `nfs-server`, and stopping them frees it; sharing the \
                            port is not an option, because the two servers would each receive \
                            some of a player's calls";

/// Where the three servers are answering.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ports {
    /// The portmapper. 111, or a player will never look (F46).
    pub portmap: u16,
    /// mountd. 48276 when it was free.
    pub mount: u16,
    /// nfsd. 2049 when it was free.
    pub nfs: u16,
}

impl Ports {
    /// Whether real hardware can find these.
    ///
    /// False means the portmapper is not on 111, and a deck will retry
    /// `GETPORT` for ever rather than fall back to the well-known ports (F46) —
    /// so everything else here is bound, correct and unreachable.
    pub fn is_discoverable(&self) -> bool {
        self.portmap == portmap::PORT
    }
}

/// Which ports to take and which interface to answer on.
#[derive(Clone, Debug)]
pub struct NfsConfig {
    /// The interface to pin the sockets to.
    ///
    /// `None` leaves the choice to the routing table, which is right for tests
    /// and for a single-homed host. Link-local has no default route, so on a
    /// multi-homed host this is what keeps replies going out the way they came.
    pub interface: Option<Interface>,
    /// Where to put the portmapper. Anything but 111 makes us invisible to real
    /// hardware, and is useful only for tests and experiments.
    pub portmap_port: u16,
    /// Preferred mountd port; an ephemeral one is used if it is taken.
    pub mount_port: u16,
    /// Preferred nfsd port; an ephemeral one is used if it is taken.
    pub nfs_port: u16,
}

impl Default for NfsConfig {
    fn default() -> Self {
        Self {
            interface: None,
            portmap_port: portmap::PORT,
            mount_port: mount::PIONEER_PORT,
            nfs_port: nfs2::PORT,
        }
    }
}

/// A portmapper, a mountd and an nfsd over one tree.
///
/// Dropping it stops all three.
#[derive(Debug)]
pub struct NfsServer {
    ports: Ports,
    dispatcher: Arc<Dispatcher>,
    tasks: Vec<JoinHandle<()>>,
}

impl NfsServer {
    /// Bind the three ports and start answering.
    ///
    /// `vfs` is shared and may be mutated while this runs: grafting a medium in
    /// with [`Vfs::mount`] makes its export appear, because the export list is
    /// derived from the tree rather than kept beside it.
    ///
    /// Fails if the portmapper cannot be bound, which is the one failure that
    /// makes everything else pointless — see the module documentation.
    #[expect(
        clippy::unused_async,
        reason = "spawns tasks, so it needs a tokio runtime; async is how that is documented \
                  at the call site, and keeps the signature stable if setup later awaits"
    )]
    pub async fn start(vfs: Arc<RwLock<Vfs>>, config: NfsConfig) -> Result<Self> {
        let interface = config.interface.as_ref();
        // The unprivileged two first: if they cannot be had at all there is
        // nothing for a portmapper to publish, and finding that out is cheap.
        let (nfs, nfs_port) = bind_preferred(config.nfs_port, interface, "nfsd")?;
        let (mount, mount_port) = bind_preferred(config.mount_port, interface, "mountd")?;
        // And 111 last, because it is the only one that can fail for want of
        // privilege and the only one whose failure is worth an error.
        let portmap = bind_portmapper(config.portmap_port, interface)?;
        let portmap_port = port_of(&portmap)?;

        let ports = Ports {
            portmap: portmap_port,
            mount: mount_port,
            nfs: nfs_port,
        };
        if ports.is_discoverable() {
            info!(
                portmap = ports.portmap,
                mount = ports.mount,
                nfs = ports.nfs,
                "serving files",
            );
        } else {
            warn!(
                portmap = ports.portmap,
                "the portmapper is not on {}: a player will retry GETPORT indefinitely, never \
                 fall back to the well-known ports, and never list us (F46)",
                portmap::PORT,
            );
        }

        let dispatcher = Arc::new(Dispatcher::new(vfs, ports));
        let limit = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
        let tasks = [
            (portmap, Service::Portmap),
            (mount, Service::Mount),
            (nfs, Service::Nfs),
        ]
        .into_iter()
        .map(|(socket, service)| {
            listen(
                Arc::new(socket),
                service,
                Arc::clone(&dispatcher),
                Arc::clone(&limit),
            )
        })
        .collect();

        Ok(Self {
            ports,
            dispatcher,
            tasks,
        })
    }

    /// Where the three servers ended up.
    pub fn ports(&self) -> Ports {
        self.ports
    }

    /// The exports peers currently hold open, most recently mounted last.
    ///
    /// A registry rather than a permission check: a real player exports to the
    /// whole link-local subnet, so nothing here decides who may read what.
    pub fn mounts(&self) -> Vec<Mount> {
        self.dispatcher.mounts()
    }
}

impl Drop for NfsServer {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Answer datagrams on one socket until it dies.
fn listen(
    socket: Arc<UdpSocket>,
    service: Service,
    dispatcher: Arc<Dispatcher>,
    limit: Arc<Semaphore>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = vec![0u8; MAX_DATAGRAM];
        loop {
            let (len, from) = match socket.recv_from(&mut buffer).await {
                Ok(received) => received,
                Err(error) => {
                    warn!(%error, ?service, "socket closed");
                    return;
                }
            };
            let SocketAddr::V4(from) = from else { continue };
            let Some(datagram) = buffer.get(..len) else {
                continue;
            };
            let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
                // Dropping is a legitimate answer on UDP: the caller resends.
                // Queueing would trade a retransmit for unbounded memory.
                debug!(?service, %from, "at {MAX_IN_FLIGHT} calls in flight; dropping one");
                continue;
            };

            let datagram = datagram.to_vec();
            let dispatcher = Arc::clone(&dispatcher);
            let socket = Arc::clone(&socket);
            tokio::spawn(async move {
                let _permit = permit;
                // On a blocking thread: a READ is a disk seek, and this
                // runtime is also emitting the status packets that keep us on
                // the network.
                let answered = tokio::task::spawn_blocking(move || {
                    dispatcher.answer(service, &datagram, *from.ip())
                })
                .await;
                match answered {
                    Ok(Some(reply)) => {
                        if let Err(error) = socket.send_to(&reply, SocketAddr::V4(from)).await {
                            warn!(%error, %from, "reply not sent");
                        }
                    }
                    Ok(None) => {}
                    Err(error) => warn!(%error, ?service, "answering panicked"),
                }
            });
        }
    })
}

/// Bind `preferred` exclusively, falling back to an ephemeral port.
///
/// The fallback matters because a real `rpcbind` or `nfsd` on the same host may
/// hold the number. Losing it costs only the client that skips the portmapper;
/// failing to bind would cost everything.
///
/// Exclusive, because the alternative is worse than losing the port: with
/// `SO_REUSEPORT` the bind succeeds beside the other server and the kernel then
/// hands each of us some of a player's calls. See [`crate::socket::Sharing`].
fn bind_preferred(
    preferred: u16,
    interface: Option<&Interface>,
    what: &'static str,
) -> Result<(UdpSocket, u16)> {
    let socket = match socket::bind_exclusive(Ipv4Addr::UNSPECIFIED, preferred, interface) {
        Ok(socket) => socket,
        Err(error) => {
            warn!(%error, what, wanted = preferred, "port taken; using an ephemeral one");
            socket::bind_exclusive(Ipv4Addr::UNSPECIFIED, 0, interface)?
        }
    };
    let port = port_of(&socket)?;
    Ok((socket, port))
}

/// Bind the portmapper, or explain what to do about it.
///
/// No fallback: a portmapper anywhere but 111 is one no player will ever ask,
/// so quietly moving it would turn a loud failure into a silent one. The two
/// ways it fails have different fixes and a user will meet both, so the remedy
/// is chosen from the operating system's own reason — and the reason itself
/// stays attached, so a caller can tell them apart by
/// [`std::io::ErrorKind`] rather than by reading English.
fn bind_portmapper(port: u16, interface: Option<&Interface>) -> Result<UdpSocket> {
    match socket::bind_exclusive(Ipv4Addr::UNSPECIFIED, port, interface) {
        Ok(socket) => Ok(socket),
        Err(Error::Io { source, .. }) => {
            let remedy = if source.kind() == std::io::ErrorKind::AddrInUse {
                ALREADY_HELD
            } else {
                NOT_PERMITTED
            };
            Err(Error::PrivilegedPort {
                port,
                source,
                remedy,
            })
        }
        Err(other) => Err(other),
    }
}

fn port_of(socket: &UdpSocket) -> Result<u16> {
    Ok(
        match socket
            .local_addr()
            .map_err(Error::io("reading a bound port"))?
        {
            SocketAddr::V4(address) => address.port(),
            SocketAddr::V6(address) => address.port(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::Ipv4Addr;
    use std::time::Duration;

    use prolink_proto::rpc::nfs2::{FileHandle, ReadArgs};
    use prolink_proto::rpc::xdr::Utf16LeString;
    use prolink_proto::rpc::{Auth, AuthUnix, Call, IpProtocol, Program, Reply, Xid};

    /// A stick with one track on it, 100 KB of recognisable bytes.
    fn medium() -> (Arc<RwLock<Vfs>>, Vec<u8>) {
        let audio: Vec<u8> = (0..=255u8).cycle().take(100_000).collect();
        let mut vfs = Vfs::new();
        vfs.add_file("/C/PIONEER/rekordbox/export.pdb", b"pdb".to_vec());
        vfs.add_file("/C/Contents/GESAFFELSTEIN/track.mp3", audio.clone());
        (Arc::new(RwLock::new(vfs)), audio)
    }

    /// Ephemeral everything, so a test needs no privileges and cannot collide
    /// with a real `rpcbind` or with another test.
    fn ephemeral() -> NfsConfig {
        NfsConfig {
            interface: None,
            portmap_port: 0,
            mount_port: 0,
            nfs_port: 0,
        }
    }

    fn at(port: u16) -> SocketAddr {
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)
    }

    /// One call out, one reply back, the way a player does it.
    async fn round_trip(client: &UdpSocket, port: u16, call: &[u8]) -> Vec<u8> {
        client
            .send_to(call, at(port))
            .await
            .expect("a call goes out");
        let mut buffer = vec![0u8; MAX_DATAGRAM];
        let (len, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buffer))
            .await
            .expect("a reply arrives within five seconds")
            .expect("a reply is received");
        buffer.truncate(len);
        buffer
    }

    fn encode(
        program: Program,
        version: u32,
        procedure: u32,
        arguments: &[u8],
        xid: u32,
    ) -> Vec<u8> {
        let credential =
            AuthUnix::cdj(prolink_proto::rpc::stamp_for_xid(Xid(xid)).unwrap_or(0)).encode();
        Call::new(
            Xid(xid),
            program,
            version,
            procedure,
            Auth::unix(&credential),
            arguments,
        )
        .encode()
    }

    fn results(datagram: &[u8]) -> Vec<u8> {
        Reply::parse(datagram)
            .expect("a reply must decode")
            .results()
            .expect("a successful reply")
            .to_vec()
    }

    /// The whole path a deck walks, over real sockets: find mountd, mount the
    /// stick, walk to a track, and stream it.
    #[tokio::test]
    async fn a_player_can_find_mount_walk_and_stream() {
        let (vfs, audio) = medium();
        let server = NfsServer::start(vfs, ephemeral())
            .await
            .expect("ephemeral ports need no privileges");
        let ports = server.ports();
        assert!(
            !ports.is_discoverable(),
            "a portmapper off 111 is not one hardware would find",
        );
        let client = socket::bind(0, None).expect("a client socket");

        // 1. Which port is mountd? A deck asks this first, always.
        let getport = encode(
            Program::PORTMAP,
            portmap::VERSION,
            portmap::Proc::GETPORT.0,
            &portmap::Request::GetPort(portmap::Mapping::query(
                Program::MOUNT,
                mount::VERSION,
                IpProtocol::UDP,
            ))
            .encode_arguments(),
            1,
        );
        let reply = round_trip(&client, ports.portmap, &getport).await;
        assert_eq!(
            portmap::Response::parse(portmap::Proc::GETPORT, &results(&reply)).unwrap(),
            portmap::Response::GetPort(Some(ports.mount)),
        );

        // 2. Mount the USB export.
        let mnt = encode(
            Program::MOUNT,
            mount::VERSION,
            mount::Proc::MNT.0,
            &mount::Request::Mnt(Utf16LeString::new(mount::EXPORT_USB)).encode_arguments(),
            2,
        );
        let reply = round_trip(&client, ports.mount, &mnt).await;
        let mount::Response::Mnt(Ok(root)) =
            mount::Response::parse(mount::Proc::MNT, &results(&reply)).unwrap()
        else {
            panic!("the USB export must mount");
        };
        assert_eq!(server.mounts().len(), 1, "and the server remembers it");

        // 3. Walk to the track, one LOOKUP per component, rewriting the tail
        //    of every handle on the way exactly as a CDJ does (F28).
        let mut handle = root;
        for (index, component) in ["Contents", "GESAFFELSTEIN", "track.mp3"]
            .into_iter()
            .enumerate()
        {
            let mut sent = handle;
            sent.0[FileHandle::KEY_LEN..].copy_from_slice(&[0xab; 20]);
            let lookup = encode(
                Program::NFS,
                nfs2::VERSION,
                nfs2::Proc::LOOKUP.0,
                &nfs2::Request::Lookup {
                    dir: sent,
                    name: Utf16LeString::new(component),
                }
                .encode_arguments(),
                u32::try_from(index).unwrap_or(0) + 3,
            );
            let reply = round_trip(&client, ports.nfs, &lookup).await;
            let nfs2::Response::Lookup(Ok(found)) =
                nfs2::Response::parse(nfs2::Proc::LOOKUP, &results(&reply)).unwrap()
            else {
                panic!("{component} must resolve");
            };
            handle = found.handle;
        }

        // 4. Stream it, 8192 bytes at a time, as a real CDJ does.
        let mut streamed = Vec::new();
        while streamed.len() < audio.len() {
            let offset = u64::try_from(streamed.len()).unwrap();
            let read = encode(
                Program::NFS,
                nfs2::VERSION,
                nfs2::Proc::READ.0,
                &nfs2::Request::Read(ReadArgs::at(handle, offset, 8192).unwrap())
                    .encode_arguments(),
                9,
            );
            let reply = round_trip(&client, ports.nfs, &read).await;
            let payload = results(&reply);
            let nfs2::Response::Read(Ok(data)) =
                nfs2::Response::parse(nfs2::Proc::READ, &payload).unwrap()
            else {
                panic!("a read at {offset} must succeed");
            };
            assert!(!data.data.is_empty(), "no progress at {offset}");
            streamed.extend_from_slice(data.data);
        }
        assert_eq!(streamed, audio, "the bytes that come back are the file");
    }

    #[tokio::test]
    async fn a_medium_grafted_in_after_the_server_started_is_served() {
        // Inserting a stick is a `Vfs::mount` and nothing else: there is no
        // export table beside the tree to forget to update.
        let vfs = Arc::new(RwLock::new(Vfs::new()));
        let server = NfsServer::start(Arc::clone(&vfs), ephemeral())
            .await
            .expect("start");
        let client = socket::bind(0, None).expect("a client socket");
        let mnt = encode(
            Program::MOUNT,
            mount::VERSION,
            mount::Proc::MNT.0,
            &mount::Request::Mnt(Utf16LeString::new(mount::EXPORT_USB)).encode_arguments(),
            1,
        );

        let reply = round_trip(&client, server.ports().mount, &mnt).await;
        assert!(
            matches!(
                mount::Response::parse(mount::Proc::MNT, &results(&reply)).unwrap(),
                mount::Response::Mnt(Err(_)),
            ),
            "an empty slot has nothing to mount",
        );

        vfs.write()
            .expect("the tree")
            .add_file("/C/PIONEER/rekordbox/export.pdb", b"pdb".to_vec());

        let reply = round_trip(&client, server.ports().mount, &mnt).await;
        assert!(
            matches!(
                mount::Response::parse(mount::Proc::MNT, &results(&reply)).unwrap(),
                mount::Response::Mnt(Ok(_)),
            ),
            "and now there is",
        );
    }

    #[tokio::test]
    async fn a_second_server_cannot_take_the_ports_of_the_first() {
        // Shared, this bind would succeed and the kernel would split a player's
        // calls between the two of us: the mount answered by one server and the
        // reads by the other, with nothing anywhere reporting a problem.
        let (vfs, _) = medium();
        let first = NfsServer::start(Arc::clone(&vfs), ephemeral())
            .await
            .expect("the first server starts");
        let held = NfsConfig {
            interface: None,
            portmap_port: first.ports().portmap,
            mount_port: first.ports().mount,
            nfs_port: first.ports().nfs,
        };
        let error = NfsServer::start(vfs, held)
            .await
            .expect_err("the portmapper's port is already held");
        // mountd and nfsd fall back to an ephemeral port, because losing their
        // number costs only a client that skips discovery. The portmapper has
        // nowhere to fall back to.
        assert!(
            matches!(&error, Error::PrivilegedPort { source, .. }
                if source.kind() == std::io::ErrorKind::AddrInUse),
            "{error:?}",
        );
        assert!(
            error.to_string().contains("already holds it"),
            "the remedy must be the one for a clash, not the one for privilege: {error}",
        );
    }

    #[tokio::test]
    async fn a_reply_comes_back_from_the_port_the_call_went_to() {
        // A client matches a reply by its source port as well as by its xid, so
        // a server answering from anywhere else would look silent.
        let (vfs, _) = medium();
        let server = NfsServer::start(vfs, ephemeral()).await.expect("start");
        let client = socket::bind(0, None).expect("a client socket");
        let null = encode(Program::NFS, nfs2::VERSION, nfs2::Proc::NULL.0, &[], 1);
        client
            .send_to(&null, at(server.ports().nfs))
            .await
            .expect("a call goes out");
        let mut buffer = vec![0u8; MAX_DATAGRAM];
        let (_, from) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buffer))
            .await
            .expect("a reply arrives")
            .expect("a reply is received");
        assert_eq!(from.port(), server.ports().nfs);
    }

    #[test]
    fn the_defaults_are_the_numbers_a_cdj_publishes() {
        // F10's `rpcinfo` output, which is what we impersonate.
        let config = NfsConfig::default();
        assert_eq!(config.portmap_port, 111);
        assert_eq!(config.mount_port, 48276);
        assert_eq!(config.nfs_port, 2049);
        assert!(
            Ports {
                portmap: 111,
                mount: 48276,
                nfs: 2049,
            }
            .is_discoverable()
        );
    }

    #[tokio::test]
    async fn a_dropped_server_stops_answering() {
        let (vfs, _) = medium();
        let server = NfsServer::start(vfs, ephemeral()).await.expect("start");
        let port = server.ports().nfs;
        let client = socket::bind(0, None).expect("a client socket");
        let null = encode(Program::NFS, nfs2::VERSION, nfs2::Proc::NULL.0, &[], 1);
        let reply = round_trip(&client, port, &null).await;
        assert_eq!(reply.len(), 24);

        drop(server);
        // Let the aborted tasks actually stop before asking again.
        tokio::task::yield_now().await;
        client
            .send_to(&null, at(port))
            .await
            .expect("a call goes out");
        let mut buffer = vec![0u8; MAX_DATAGRAM];
        assert!(
            tokio::time::timeout(Duration::from_millis(250), client.recv_from(&mut buffer))
                .await
                .is_err(),
            "a dropped server has closed its sockets",
        );
    }
}
