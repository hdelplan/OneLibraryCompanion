// SPDX-License-Identifier: GPL-3.0-only

//! Reading Pro DJ Link traffic out of pcap and pcapng captures.
//!
//! Every codec in this workspace can be exercised against **real Pioneer
//! traffic** with no hardware attached, and this crate is what makes that
//! possible. The distinction matters: a round-trip test between our encoder
//! and our decoder proves they agree with each other, which is not the same as
//! agreeing with a CDJ. A capture is the only thing that can tell the two
//! apart, and the reference implementation this project replaces had an
//! encoder and a decoder that agreed perfectly on a bug.
//!
//! Nothing here knows what a Pro DJ Link message is. This is link and
//! transport only — Ethernet, IPv4, UDP, TCP — and the magic check belongs to
//! whoever asked for the bytes.
//!
//! # The trap: filter on the destination port
//!
//! The Pro DJ Link type byte at offset `0x0a` is **shared across UDP ports and
//! the layouts behind it are not**. `0x06` is a keep-alive on 50000 and a
//! media response on 50002; `0x0a` is a hello on 50000 and a player status on
//! 50002. So a corpus filter of "either endpoint is 50002" is wrong: a tool
//! that binds one socket and sends its keep-alives *from* 50002 contributes
//! packets that the filter accepts and the 50002 decoder reads as confident
//! nonsense — the right number of bytes, plausible values, no error anywhere.
//!
//! Filter on where a packet was **going**, which is the only thing that says
//! which protocol wrote it. [`Capture::udp_to`] is the whole of the intended
//! API for this, and there is deliberately no source-port equivalent.
//!
//! Real hardware hides the mistake well. Of the 35 103 datagrams in this
//! project's corpus addressed to 50002, only 24 were sent *from* 50002,
//! because a CDJ sends each status packet from a different, incrementing
//! source port. A source-port filter therefore finds nothing at all, and an
//! either-endpoint filter looks correct right up until a second tool joins the
//! network.
//!
//! **TCP is the other way round.** A UDP datagram is a thing with a
//! destination; a TCP connection is a thing with two ends, and both directions
//! of a dbserver conversation belong to the server's port. So
//! [`tcp::Reassembler::on_ports`] matches either endpoint, and says so.
//!
//! # Reading a capture
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use prolink_capture::Capture;
//!
//! for packet in Capture::open("run.pcap")?.udp_to(50000) {
//!     let packet = packet?;
//!     println!("{} sent {} bytes", packet.source, packet.payload.len());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Reassembling the dbserver conversations in one:
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use prolink_capture::{Capture, tcp::Reassembler};
//!
//! let mut reassembler = Reassembler::new();
//! for packet in Capture::open("run.pcap")? {
//!     reassembler.push(&packet?);
//! }
//! for stream in reassembler.finish() {
//!     // `None` means a segment is missing: the dbserver protocol has no
//!     // framing, so concatenating across the hole would silently desynchronise
//!     // every message after it.
//!     if let Some(bytes) = stream.contiguous() {
//!         println!("{:?}: {} bytes", stream.flow(), bytes.len());
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Scope
//!
//! Ethernet over IPv4, UDP and TCP, and IP fragment reassembly, which is not
//! optional: a CDJ's NFS reads come back as five or six fragments each and a
//! reader that ignored them would under-report every transfer without saying
//! so. Frames from a link layer this crate does not dissect are an error and
//! not an empty result, because an empty result is indistinguishable from a
//! capture of a quiet network.

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

mod capture;
mod corpus;
mod error;
mod packet;
mod sparse;
pub mod tcp;

pub use capture::{Capture, Format, UdpTo};
pub use corpus::{CORPUS_ENV, Corpus};
pub use error::{Error, Result};
pub use packet::{Flow, Packet, Transport};
pub use sparse::{Gap, Run};

/// The port a Pioneer device announces itself on, broadcast.
///
/// Named here so a caller filtering a corpus does not have to write the number
/// out, and so that the name sits next to the warning about which end of the
/// packet to match it against. See [`Capture::udp_to`].
pub const DISCOVERY_PORT: u16 = 50000;

/// The port beat packets are broadcast to.
pub const BEAT_PORT: u16 = 50001;

/// The port player status, media queries and settings queries are unicast to.
pub const STATUS_PORT: u16 = 50002;
