// SPDX-License-Identifier: GPL-3.0-only

//! One transport payload lifted out of a capture.

use std::fmt;
use std::net::SocketAddrV4;
use std::time::Duration;

/// One UDP datagram or TCP segment, with the addresses it travelled between.
///
/// Only IPv4 over Ethernet is modelled, and only UDP and TCP above it, because
/// that is the whole of what a Pro DJ Link capture contains. Anything else in
/// the file — ARP, IPv6, ICMP, a frame from a link layer we do not dissect —
/// is not a packet in this sense and never appears.
///
/// The payload has already been IP-defragmented, so an 8 KiB NFS reply arrives
/// here as one datagram and not as the six on the wire.
#[derive(Clone, PartialEq, Eq)]
pub struct Packet {
    /// Position of the frame in the file, counting from one.
    ///
    /// A defragmented datagram carries the index of its *last* fragment, which
    /// is the frame at which it became readable.
    pub index: u64,

    /// When the frame was captured, since the Unix epoch.
    ///
    /// A pcapng simple-packet block records no time at all; those arrive as
    /// [`Duration::ZERO`] rather than as a guess.
    pub timestamp: Duration,

    /// Which of the capture's interfaces recorded it; `0` for classic pcap.
    ///
    /// Worth carrying because a capture of a bridge records the *same*
    /// datagram once per interface. Two identical packets differing only here
    /// are one datagram seen twice, not two datagrams — [`crate::tcp`] folds
    /// them back together, but a UDP count does not, and cannot: nothing on
    /// the wire distinguishes a duplicate from a genuine repeat.
    pub interface: u32,

    /// Who sent it.
    ///
    /// **Not** what to filter a corpus on. See [`Packet::destination`].
    pub source: SocketAddrV4,

    /// Who it was addressed to — the field a corpus filter belongs on.
    ///
    /// The Pro DJ Link type byte at offset `0x0a` is shared across UDP ports
    /// and the layouts behind it are not: `0x06` is a keep-alive on 50000 and
    /// a media response on 50002. A tool that binds one socket and sends its
    /// keep-alives *from* 50002 therefore contributes packets that a
    /// "either endpoint is 50002" filter accepts and the 50002 decoder reads
    /// as confident nonsense. Filter on where a packet was going, which is the
    /// only thing that decides which protocol wrote it.
    pub destination: SocketAddrV4,

    /// Which transport, and the fields reassembly needs from it.
    pub transport: Transport,

    /// The transport payload, with IP fragmentation already undone.
    pub payload: Vec<u8>,
}

impl Packet {
    /// The payload, if this datagram was addressed to `port` over UDP.
    ///
    /// The one-packet form of [`crate::Capture::udp_to`], and the same rule:
    /// the *destination* decides, never the source.
    pub fn udp_payload_to(&self, port: u16) -> Option<&[u8]> {
        (self.transport == Transport::Udp && self.destination.port() == port)
            .then_some(self.payload.as_slice())
    }
}

impl fmt::Debug for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let protocol = match self.transport {
            Transport::Udp => "udp",
            Transport::Tcp { .. } => "tcp",
        };
        write!(
            f,
            "#{} {:.6}s {protocol} {} -> {} {}B",
            self.index,
            self.timestamp.as_secs_f64(),
            self.source,
            self.destination,
            self.payload.len()
        )
    }
}

/// Which transport carried a [`Packet`], and what reassembly needs from it.
///
/// A real enum rather than a protocol-number newtype because this crate owns
/// every case: a payload we cannot attribute to UDP or TCP is not a
/// [`Packet`] at all. The sequence number lives inside the TCP variant so that
/// a UDP packet carrying one cannot be built.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    /// A UDP datagram.
    Udp,
    /// One TCP segment.
    Tcp {
        /// Sequence number of this segment's first byte on the wire.
        ///
        /// On a SYN this numbers the SYN itself, so the first payload byte is
        /// at `sequence + 1`; [`crate::tcp`] is where that is accounted for.
        sequence: u32,
        /// The SYN flag: this segment opens the connection in this direction.
        syn: bool,
        /// The FIN flag: the sender has no more data.
        fin: bool,
        /// The RST flag: the connection was aborted.
        reset: bool,
    },
}

impl Transport {
    /// True for UDP.
    pub fn is_udp(self) -> bool {
        matches!(self, Self::Udp)
    }

    /// True for TCP.
    pub fn is_tcp(self) -> bool {
        matches!(self, Self::Tcp { .. })
    }
}

/// One direction of one connection: the key a TCP byte stream is kept under.
///
/// The four-tuple *and* the direction, because the two directions of a
/// dbserver conversation are different message types and interleaving them
/// would produce a byte stream nobody ever sent.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Flow {
    /// Who sends on this half of the connection.
    pub source: SocketAddrV4,
    /// Who receives.
    pub destination: SocketAddrV4,
}

impl Flow {
    /// The flow a packet belongs to.
    pub fn of(packet: &Packet) -> Self {
        Self {
            source: packet.source,
            destination: packet.destination,
        }
    }

    /// The same connection, the other way round.
    #[must_use]
    pub fn reversed(self) -> Self {
        Self {
            source: self.destination,
            destination: self.source,
        }
    }

    /// Whether either endpoint sits on `port`.
    ///
    /// **This is the right test for TCP and the wrong one for UDP.** A TCP
    /// connection is a thing with two ends, and both directions of a dbserver
    /// conversation belong to the server's port; a UDP datagram is a thing
    /// with a destination, and its source port says nothing about which
    /// protocol wrote it. See [`Packet::destination`].
    pub fn involves(self, port: u16) -> bool {
        self.source.port() == port || self.destination.port() == port
    }
}

impl fmt::Debug for Flow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} -> {}", self.source, self.destination)
    }
}
