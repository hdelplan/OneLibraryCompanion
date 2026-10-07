// SPDX-License-Identifier: GPL-3.0-only

//! Building the UDP sockets Pro DJ Link needs.
//!
//! Three details make these different from a plain `UdpSocket::bind`.
//!
//! **Address reuse.** Another tool — rekordbox, a reference implementation, a
//! second copy of this one — may already hold port 50000 or 50002. Without
//! `SO_REUSEADDR` and `SO_REUSEPORT` the bind simply fails, and a library that
//! cannot coexist with the tools a DJ already has open is a library nobody can
//! debug with.
//!
//! **Binding the interface, not the address.** The socket has to stay bound to
//! `0.0.0.0` to *receive* subnet broadcasts, but a socket bound that way has no
//! route for `169.254.255.255` and every send fails with `No route to host` —
//! link-local has no default route, so the kernel cannot choose for us. Pinning
//! the *interface* resolves both halves at once: broadcast reception keeps
//! working and transmission goes out the interface we mean. It also makes
//! multi-homed behaviour explicit rather than leaving it to the routing table.
//!
//! **Sending from the port we listen on.** Replies to a device-number claim —
//! conflicts, and mixer assignments — are unicast *to port 50000*, so a virtual
//! CDJ must transmit from the socket it is listening on. Two sockets would mean
//! never hearing the answer.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

use crate::interface::Interface;
use crate::{Error, Result};

/// Comfortably above the largest datagram this protocol carries.
///
/// An NFSv2 `READ` maxes out at 8192 bytes of payload — real CDJs use exactly
/// that, relying on IP fragmentation — and a portmap `DUMP` of a busy host can
/// be a few kilobytes.
pub(crate) const MAX_DATAGRAM: usize = 65535;

/// Whether a port may be shared with another process.
///
/// The two cases are opposites and getting them the wrong way round fails
/// differently in each direction.
///
/// On the **Pro DJ Link ports** sharing is the point: a DJ will have rekordbox
/// or another tool open, both of us only ever *receive* broadcasts there, and a
/// library that cannot bind beside the tools already running is a library nobody
/// can debug with.
///
/// On the **ONC RPC ports** sharing is silent corruption. With `SO_REUSEPORT` a
/// bind of 111 or 2049 succeeds beside a real `rpcbind` or `nfsd`, and the
/// kernel then splits arriving datagrams between the two of us — so a deck's
/// `GETPORT` is answered by whichever process happened to receive it, and the
/// mount that follows goes to the wrong server. There is no way to detect that
/// afterwards, so those binds must be exclusive and a clash must be an error the
/// caller can report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sharing {
    /// Take the port even if another process holds it.
    Shared,
    /// Fail if another process holds it.
    Exclusive,
}

/// Bind a UDP socket for a Pro DJ Link port.
///
/// Binds `0.0.0.0:port` so subnet broadcasts arrive, enables address reuse so it
/// can coexist with other tools, enables broadcast transmission, and pins the
/// outgoing interface.
pub(crate) fn bind(port: u16, interface: Option<&Interface>) -> Result<UdpSocket> {
    bind_at(Ipv4Addr::UNSPECIFIED, port, interface)
}

/// Send one datagram, right now, from a socket that lives no longer than the
/// call.
///
/// For answers that have to leave from a synchronous context: a tokio
/// `UdpSocket` starts out with its readiness state unknown, so `try_send_to` on
/// a freshly bound one returns `WouldBlock` and the datagram is simply never
/// sent — which is what silently swallowed every answer to a CDJ's MASTER
/// button. Awaiting `send_to` would fix it and would also mean holding the
/// session lock across an await, which the guard's type forbids. A blocking
/// socket sends immediately: there is no queue to fill with a single datagram.
pub(crate) fn send_once(interface: &Interface, to: SocketAddr, datagram: &[u8]) -> Result<()> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(Error::io("creating a UDP socket"))?;
    socket
        .set_broadcast(true)
        .map_err(Error::io("SO_BROADCAST"))?;
    socket
        .bind(&SocketAddr::V4(SocketAddrV4::new(interface.ip, 0)).into())
        .map_err(Error::io("binding a UDP socket"))?;
    pin_to_interface(&socket, interface)?;
    socket
        .send_to(datagram, &to.into())
        .map_err(Error::io("sending a datagram"))?;
    Ok(())
}

/// Bind a UDP socket at a specific local address.
///
/// Used for the ephemeral sockets that carry ONC RPC, where the source address
/// must be ours on the CDJ-facing interface so the reply comes back.
pub(crate) fn bind_at(
    address: Ipv4Addr,
    port: u16,
    interface: Option<&Interface>,
) -> Result<UdpSocket> {
    bind_sharing(address, port, interface, Sharing::Shared)
}

/// Bind a UDP socket, refusing to share the port with another process.
///
/// For the ONC RPC servers. See [`Sharing`] for why they are different.
pub(crate) fn bind_exclusive(
    address: Ipv4Addr,
    port: u16,
    interface: Option<&Interface>,
) -> Result<UdpSocket> {
    bind_sharing(address, port, interface, Sharing::Exclusive)
}

fn bind_sharing(
    address: Ipv4Addr,
    port: u16,
    interface: Option<&Interface>,
    sharing: Sharing,
) -> Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(Error::io("creating a UDP socket"))?;

    if sharing == Sharing::Shared {
        socket
            .set_reuse_address(true)
            .map_err(Error::io("SO_REUSEADDR"))?;
        #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
        socket
            .set_reuse_port(true)
            .map_err(Error::io("SO_REUSEPORT"))?;
    }
    socket
        .set_broadcast(true)
        .map_err(Error::io("SO_BROADCAST"))?;
    socket
        .set_nonblocking(true)
        .map_err(Error::io("O_NONBLOCK"))?;

    socket
        .bind(&SocketAddr::V4(SocketAddrV4::new(address, port)).into())
        .map_err(Error::io("binding a UDP socket"))?;

    if let Some(interface) = interface {
        pin_to_interface(&socket, interface)?;
    }

    UdpSocket::from_std(socket.into()).map_err(Error::io("registering a UDP socket with tokio"))
}

/// Pin a socket's outgoing interface.
///
/// Linux takes a name through `SO_BINDTODEVICE`; the BSDs and macOS take an
/// index through `IP_BOUND_IF`. `socket2` exposes both, and which one is
/// available is a compile-time fact, so this is a `cfg` rather than a runtime
/// probe.
fn pin_to_interface(socket: &Socket, interface: &Interface) -> Result<()> {
    #[cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux"))]
    {
        socket
            .bind_device(Some(interface.name.as_bytes()))
            .map_err(Error::io("SO_BINDTODEVICE"))?;
    }
    #[cfg(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
    ))]
    {
        socket
            .bind_device_by_index_v4(std::num::NonZeroU32::new(interface.index))
            .map_err(Error::io("IP_BOUND_IF"))?;
    }
    #[cfg(not(any(
        target_os = "android",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
    )))]
    {
        // Nothing to do; the routing table decides. Named so the silence is
        // deliberate rather than an oversight.
        let _ = interface;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn two_sockets_can_hold_the_same_port() {
        // A DJ will have rekordbox or another tool open. Failing to bind
        // because of that would make this library undebuggable in the field.
        let first = bind(0, None).expect("first bind");
        let port = match first.local_addr().expect("local address") {
            SocketAddr::V4(address) => address.port(),
            SocketAddr::V6(_) => unreachable!("bound as IPv4"),
        };
        let second = bind(port, None);
        assert!(
            second.is_ok(),
            "address reuse must be enabled: {:?}",
            second.err()
        );
    }

    #[tokio::test]
    async fn an_rpc_port_is_not_shared() {
        // The opposite case, and it fails silently rather than loudly: with
        // SO_REUSEPORT a bind of 111 succeeds beside a real rpcbind and the
        // kernel then splits arriving datagrams between the two of us.
        let first = bind_exclusive(Ipv4Addr::LOCALHOST, 0, None).expect("first bind");
        let port = match first.local_addr().expect("local address") {
            SocketAddr::V4(address) => address.port(),
            SocketAddr::V6(_) => unreachable!("bound as IPv4"),
        };
        assert!(
            bind_exclusive(Ipv4Addr::LOCALHOST, port, None).is_err(),
            "a second exclusive bind must fail so the caller can say whose port it is",
        );
    }

    #[tokio::test]
    async fn a_bound_socket_can_broadcast() {
        let socket = bind(0, None).expect("bind");
        assert!(socket.broadcast().expect("SO_BROADCAST readable"));
    }
}
