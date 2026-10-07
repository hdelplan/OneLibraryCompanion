// SPDX-License-Identifier: GPL-3.0-only

//! Putting TCP segments back into the byte streams they were cut out of.
//!
//! Each *direction* of each connection is its own stream, keyed by the
//! four-tuple plus direction, because the two directions of a dbserver
//! conversation are different message types and a stream that interleaved them
//! would be one nobody ever sent.
//!
//! # Why a hole may not be closed up
//!
//! The dbserver protocol has no length framing: a message is delimited by
//! nothing but its own contents, so a reader that concatenated across a
//! missing segment would not fail. It would read the bytes after the hole as
//! the field it was expecting, and every message from there on would be one
//! position out, with no error and nothing in the output to show for it. So a
//! [`Stream`] is a list of [`Run`]s and the whole thing is available only from
//! [`Stream::contiguous`], which answers `None` when there is a hole.
//!
//! # Filtering TCP by either endpoint is right, and by destination is wrong
//!
//! The opposite of the rule for UDP, and for a reason: a UDP datagram is a
//! thing with a destination, and its source port says nothing about which
//! protocol wrote it, whereas a TCP connection is a thing with two ends and
//! both of its directions belong to the server's port. Hence
//! [`Reassembler::on_ports`] matches either endpoint.
//!
//! # There is no fixed dbserver port
//!
//! The literature gives 1051, and this project's corpus does contain streams
//! on it — but it also carries dbserver conversations on 1054, 1056, 1058,
//! 1060, 1062, 1064, 1066, 1068, 1070, 1072, 1074, 1076 and 1078, because a
//! CDJ publishes the port it is listening on through the TCP-12523 query and
//! the answer is whatever it happened to bind. Those are observed values, not
//! a rule; what they establish is that reassembling only 1051 finds a fraction
//! of the traffic and looks exactly like having found all of it. Prefer
//! [`Reassembler::new`], which keeps every flow, unless the port came from a
//! port query rather than from a document.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use crate::sparse::{Gap, Run, Sparse};
use crate::{Flow, Packet, Transport};

/// One direction of one connection, as a byte stream with its holes intact.
#[derive(Clone)]
pub struct Stream {
    flow: Flow,
    first_seen: Duration,
    from_connection_start: bool,
    bytes: Sparse,
}

impl Stream {
    /// Which direction of which connection this is.
    pub fn flow(&self) -> Flow {
        self.flow
    }

    /// When the first segment of it was captured.
    pub fn first_seen(&self) -> Duration {
        self.first_seen
    }

    /// Whether the SYN that opened this direction was captured.
    ///
    /// When it was, offset zero is the connection's first byte and
    /// [`Stream::contiguous`] returning `Some` means the whole conversation.
    /// When it was not, the capture began somewhere in the middle and offset
    /// zero is merely the earliest byte seen — a prefix is missing that
    /// nothing in the capture can measure.
    pub fn from_connection_start(&self) -> bool {
        self.from_connection_start
    }

    /// The stretches that arrived, in order.
    pub fn runs(&self) -> &[Run] {
        self.bytes.runs()
    }

    /// The whole stream, or `None` when a hole splits it.
    pub fn contiguous(&self) -> Option<&[u8]> {
        self.bytes.contiguous()
    }

    /// The holes, in order.
    pub fn gaps(&self) -> Vec<Gap> {
        self.bytes.gaps()
    }

    /// How many bytes were captured, not counting the holes.
    pub fn len(&self) -> u64 {
        self.bytes.len()
    }

    /// True when no payload was captured for this direction at all.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream")
            .field("flow", &self.flow)
            .field("bytes", &self.len())
            .field("runs", &self.runs().len())
            .field("from_connection_start", &self.from_connection_start)
            .finish_non_exhaustive()
    }
}

/// Collects segments into [`Stream`]s.
///
/// Feed it every packet of a capture; it ignores everything that is not TCP.
///
/// ```no_run
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use prolink_capture::{Capture, tcp::Reassembler};
///
/// let mut reassembler = Reassembler::new();
/// for packet in Capture::open("run.pcap")? {
///     reassembler.push(&packet?);
/// }
/// for stream in reassembler.finish() {
///     match stream.contiguous() {
///         Some(bytes) => println!("{:?}: {} bytes", stream.flow(), bytes.len()),
///         None => println!("{:?}: {} holes", stream.flow(), stream.gaps().len()),
///     }
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default)]
pub struct Reassembler {
    ports: Option<Vec<u16>>,
    flows: HashMap<Flow, Builder>,
    order: u64,
}

impl Reassembler {
    /// Keep every TCP flow.
    pub fn new() -> Self {
        Self {
            ports: None,
            flows: HashMap::new(),
            order: 0,
        }
    }

    /// Keep only flows with an endpoint on one of `ports`.
    ///
    /// Either endpoint, not the destination — see the module documentation.
    /// Worth using on a large capture: this project's corpus carries 18 MB of
    /// TCP, most of it not dbserver.
    pub fn on_ports(ports: impl IntoIterator<Item = u16>) -> Self {
        Self {
            ports: Some(ports.into_iter().collect()),
            flows: HashMap::new(),
            order: 0,
        }
    }

    /// Offer one packet. Anything that is not TCP, or is on a flow this
    /// reassembler is not keeping, is ignored.
    pub fn push(&mut self, packet: &Packet) {
        let Transport::Tcp { sequence, syn, .. } = packet.transport else {
            return;
        };
        let flow = Flow::of(packet);
        if let Some(ports) = &self.ports
            && !ports.iter().any(|&port| flow.involves(port))
        {
            return;
        }
        if !syn && packet.payload.is_empty() {
            // A bare acknowledgement, FIN or RST carries no bytes and tells us
            // nothing about where the stream starts.
            return;
        }
        self.remember(flow, packet.timestamp);
        let Some(builder) = self.flows.get_mut(&flow) else {
            return;
        };
        if syn {
            // A SYN consumes one sequence number of its own, so the first
            // payload byte of the connection is at `sequence + 1`. Without
            // this the whole stream would sit one byte above the base and
            // every run would report a phantom one-byte hole in front of it.
            builder.open(sequence.wrapping_add(1));
        }
        if !packet.payload.is_empty() {
            let start = if syn {
                sequence.wrapping_add(1)
            } else {
                sequence
            };
            builder.add(start, &packet.payload);
        }
    }

    fn remember(&mut self, flow: Flow, timestamp: Duration) {
        if self.flows.contains_key(&flow) {
            return;
        }
        let order = self.order;
        self.order = self.order.saturating_add(1);
        self.flows.insert(
            flow,
            Builder {
                flow,
                first_seen: timestamp,
                base: None,
                from_connection_start: false,
                bytes: Sparse::new(),
                order,
            },
        );
    }

    /// The streams, in the order their first segment was captured.
    ///
    /// A flow that carried no payload — a handshake, a port scan, a connection
    /// refused — is not a stream and does not appear.
    pub fn finish(self) -> Vec<Stream> {
        let mut builders: Vec<Builder> = self
            .flows
            .into_values()
            .filter(|builder| !builder.bytes.is_empty())
            .collect();
        builders.sort_by_key(|builder| builder.order);
        builders
            .into_iter()
            .map(|builder| Stream {
                flow: builder.flow,
                first_seen: builder.first_seen,
                from_connection_start: builder.from_connection_start,
                bytes: builder.bytes,
            })
            .collect()
    }
}

#[derive(Debug)]
struct Builder {
    flow: Flow,
    first_seen: Duration,
    /// Sequence number that offset zero corresponds to.
    base: Option<u32>,
    from_connection_start: bool,
    bytes: Sparse,
    order: u64,
}

impl Builder {
    /// Record the sequence number of the connection's first payload byte.
    ///
    /// A retransmitted SYN carries the same number, so this is idempotent:
    /// [`Builder::rebase`] refuses a base that is not below the current one.
    fn open(&mut self, base: u32) {
        self.from_connection_start = true;
        self.rebase(base);
    }

    fn add(&mut self, sequence: u32, data: &[u8]) {
        let base = *self.base.get_or_insert(sequence);
        let delta = sequence.wrapping_sub(base);
        if delta & 0x8000_0000 == 0 {
            self.bytes.insert(u64::from(delta), data);
        } else {
            // The segment precedes what we had taken to be the start: the
            // capture opened on a later segment and this is an earlier one,
            // arriving late or retransmitted. Move the base down rather than
            // reading the wrap-around as a four-gigabyte hole.
            self.rebase(sequence);
            self.bytes.insert(0, data);
        }
    }

    /// Move offset zero down to `base`, carrying everything already held.
    fn rebase(&mut self, base: u32) {
        match self.base {
            None => self.base = Some(base),
            Some(current) => {
                let shift = current.wrapping_sub(base);
                if shift == 0 || shift & 0x8000_0000 != 0 {
                    return; // `base` is not below the current one.
                }
                self.bytes.shift(u64::from(shift));
                self.base = Some(base);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddrV4};

    use super::*;

    fn endpoint(last_octet: u8, port: u16) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::new(169, 254, 0, last_octet), port)
    }

    struct Wire {
        index: u64,
    }

    impl Wire {
        fn new() -> Self {
            Self { index: 0 }
        }

        fn segment(&mut self, from: u16, to: u16, sequence: u32, payload: &[u8]) -> Packet {
            self.build(from, to, sequence, payload, false)
        }

        fn syn(&mut self, from: u16, to: u16, sequence: u32) -> Packet {
            self.build(from, to, sequence, b"", true)
        }

        fn build(
            &mut self,
            from: u16,
            to: u16,
            sequence: u32,
            payload: &[u8],
            syn: bool,
        ) -> Packet {
            self.index += 1;
            Packet {
                index: self.index,
                timestamp: Duration::from_millis(self.index),
                interface: 0,
                source: endpoint(1, from),
                destination: endpoint(2, to),
                transport: Transport::Tcp {
                    sequence,
                    syn,
                    fin: false,
                    reset: false,
                },
                payload: payload.to_vec(),
            }
        }
    }

    fn reassemble(packets: &[Packet]) -> Vec<Stream> {
        let mut reassembler = Reassembler::new();
        for packet in packets {
            reassembler.push(packet);
        }
        reassembler.finish()
    }

    #[test]
    fn segments_in_order_become_one_stream() {
        let mut wire = Wire::new();
        let packets = [
            wire.segment(1053, 1051, 100, b"hello "),
            wire.segment(1053, 1051, 106, b"world"),
        ];
        let streams = reassemble(&packets);
        assert_eq!(streams.len(), 1);
        assert_eq!(
            streams.first().and_then(Stream::contiguous),
            Some(b"hello world".as_slice())
        );
    }

    #[test]
    fn the_two_directions_are_separate_streams() {
        let mut wire = Wire::new();
        let mut request = wire.segment(1053, 1051, 100, b"request");
        let mut reply = wire.segment(1053, 1051, 0, b"reply");
        // The reply travels the other way.
        std::mem::swap(&mut reply.source, &mut reply.destination);
        request.timestamp = Duration::from_millis(1);
        reply.timestamp = Duration::from_millis(2);
        let streams = reassemble(&[request, reply]);
        assert_eq!(
            streams.len(),
            2,
            "interleaving the directions would invent a byte stream"
        );
        assert_eq!(
            streams.first().and_then(Stream::contiguous),
            Some(b"request".as_slice())
        );
        assert_eq!(
            streams.get(1).and_then(Stream::contiguous),
            Some(b"reply".as_slice())
        );
        assert_eq!(
            streams.first().map(|stream| stream.flow().reversed()),
            streams.get(1).map(Stream::flow)
        );
    }

    #[test]
    fn a_missing_segment_is_reported_not_concatenated_over() {
        let mut wire = Wire::new();
        // 100..106 arrives, 106..112 is lost, 112..118 arrives.
        let packets = [
            wire.segment(1053, 1051, 100, b"first "),
            wire.segment(1053, 1051, 112, b"third "),
        ];
        let streams = reassemble(&packets);
        let stream = streams.first().expect("one stream");
        assert_eq!(
            stream.contiguous(),
            None,
            "a dbserver reader given `first third ` would decode confident nonsense"
        );
        assert_eq!(stream.gaps(), vec![Gap { offset: 6, len: 6 }]);
        assert_eq!(stream.runs().len(), 2);
        assert_eq!(stream.len(), 12);
    }

    #[test]
    fn a_retransmission_leaves_the_stream_unchanged() {
        let mut wire = Wire::new();
        let packets = [
            wire.segment(1053, 1051, 100, b"hello "),
            wire.segment(1053, 1051, 106, b"world"),
            wire.segment(1053, 1051, 100, b"hello "),
            wire.segment(1053, 1051, 106, b"world"),
        ];
        let streams = reassemble(&packets);
        assert_eq!(
            streams.first().and_then(Stream::contiguous),
            Some(b"hello world".as_slice())
        );
        assert_eq!(streams.first().map(Stream::len), Some(11));
    }

    #[test]
    fn out_of_order_segments_are_put_back_in_order() {
        let mut wire = Wire::new();
        let packets = [
            wire.segment(1053, 1051, 106, b"world"),
            wire.segment(1053, 1051, 100, b"hello "),
        ];
        let streams = reassemble(&packets);
        assert_eq!(
            streams.first().and_then(Stream::contiguous),
            Some(b"hello world".as_slice())
        );
        assert_eq!(
            streams.first().map(Stream::from_connection_start),
            Some(false),
            "no SYN was captured, so the start of the connection is unknown"
        );
    }

    #[test]
    fn a_sequence_number_wrapping_past_zero_is_not_a_hole() {
        let mut wire = Wire::new();
        // 0xfffffffc..0x00000000, then 0x00000000 onwards. Subtracting the two
        // without wrapping would put the second segment four gigabytes below
        // the first, and the stream would be reported as almost entirely hole.
        let packets = [
            wire.segment(1053, 1051, 0xffff_fffc, b"abcd"),
            wire.segment(1053, 1051, 0, b"efgh"),
        ];
        let streams = reassemble(&packets);
        let stream = streams.first().expect("one stream");
        assert_eq!(stream.contiguous(), Some(b"abcdefgh".as_slice()));
        assert!(stream.gaps().is_empty());
    }

    #[test]
    fn the_syn_does_not_consume_a_payload_byte() {
        let mut wire = Wire::new();
        let packets = [
            wire.syn(1053, 1051, 4242),
            wire.segment(1053, 1051, 4243, b"first byte is here"),
        ];
        let streams = reassemble(&packets);
        let stream = streams.first().expect("one stream");
        assert!(stream.from_connection_start(), "the SYN was captured");
        assert_eq!(stream.contiguous(), Some(b"first byte is here".as_slice()));
        assert!(
            stream.gaps().is_empty(),
            "a SYN numbers itself, not a byte of data"
        );
    }

    #[test]
    fn a_capture_that_starts_mid_connection_says_so() {
        let mut wire = Wire::new();
        let packets = [wire.segment(1053, 1051, 900_000, b"midstream")];
        let streams = reassemble(&packets);
        let stream = streams.first().expect("one stream");
        assert!(!stream.from_connection_start());
        assert_eq!(
            stream.contiguous(),
            Some(b"midstream".as_slice()),
            "offset zero is the earliest byte seen, not the connection's first"
        );
    }

    #[test]
    fn a_handshake_that_carried_nothing_is_not_a_stream() {
        let mut wire = Wire::new();
        let packets = [wire.syn(1053, 1051, 7)];
        assert!(reassemble(&packets).is_empty());
    }

    #[test]
    fn a_udp_packet_is_not_offered_to_a_stream() {
        let mut wire = Wire::new();
        let mut packet = wire.segment(1053, 1051, 0, b"not tcp");
        packet.transport = Transport::Udp;
        assert!(reassemble(&[packet]).is_empty());
    }

    #[test]
    fn a_port_filter_matches_either_endpoint() {
        let mut wire = Wire::new();
        let to_server = wire.segment(1053, 1051, 0, b"query");
        let mut from_server = wire.segment(1051, 1053, 0, b"answer");
        std::mem::swap(&mut from_server.source, &mut from_server.destination);
        let elsewhere = wire.segment(2049, 2049, 0, b"nfs");

        let mut reassembler = Reassembler::on_ports([1051]);
        for packet in [&to_server, &from_server, &elsewhere] {
            reassembler.push(packet);
        }
        let streams = reassembler.finish();
        assert_eq!(
            streams.len(),
            2,
            "a connection's two directions both belong to its port"
        );
    }

    #[test]
    fn the_same_segment_seen_on_two_interfaces_is_one_copy_of_the_bytes() {
        let mut wire = Wire::new();
        let first = wire.segment(1053, 1051, 100, b"once");
        let mut duplicate = first.clone();
        duplicate.interface = 1;
        let streams = reassemble(&[first, duplicate]);
        assert_eq!(streams.len(), 1);
        assert_eq!(
            streams.first().and_then(Stream::contiguous),
            Some(b"once".as_slice())
        );
    }
}
