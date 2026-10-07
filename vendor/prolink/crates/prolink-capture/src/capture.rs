// SPDX-License-Identifier: GPL-3.0-only

//! Reading a capture file, frame by frame.
//!
//! Handles classic pcap and pcapng, dispatching on the first four bytes; this
//! project's corpus contains both, because `tcpdump` writes pcapng as soon as
//! it is given more than one interface and pcap otherwise.
//!
//! # What is deliberately not handled
//!
//! Ethernet only, IPv4 only, UDP and TCP only. That is the whole of what a Pro
//! DJ Link capture contains, and every additional link layer is code that
//! nothing in this project can test. A capture recorded on a link type this
//! crate does not dissect is an [`Error::UnsupportedLinkType`] rather than an
//! empty result, because an empty result is indistinguishable from a capture
//! of a quiet network. Frames that *are* Ethernet but carry ARP, IPv6 or
//! anything above IP that is not UDP or TCP are simply not packets in this
//! crate's sense and are skipped without comment. Of pcapng's block types only
//! the section header, the interface description, the enhanced packet block
//! and the simple packet block are read; the obsolete packet block, which
//! nothing since libpcap 0.x writes, is not.
//!
//! # Two things the format makes easy to get wrong
//!
//! **Ethernet padding.** A frame shorter than 60 bytes is padded, and the
//! padding is not payload. The IP header's `total_len` is authoritative, so
//! every length here comes from that and not from the captured frame length.
//!
//! **pcapng timestamp units.** A pcapng timestamp is a 64-bit counter in units
//! the *interface* declares through its `if_tsresol` option, defaulting to
//! microseconds. `pcap-file` 2.0 hands the raw counter back as a
//! [`Duration`] of nanoseconds regardless, so every timestamp it produces for
//! a `tcpdump` capture is a thousand times too small — 1970-01-21 instead of
//! 2026. The option is re-read here and the scaling redone.
//!
//! # IP fragmentation is not optional
//!
//! A CDJ issues NFS `READ`s of 8192 bytes, the NFSv2 maximum, and each reply
//! comes back as five or six IP fragments of which only the first carries a
//! UDP header. A reader that ignores fragments therefore sees a fraction of
//! every transfer and *under-reports it silently*. Fragments are reassembled
//! here, per capture interface, and an incomplete datagram is never emitted.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Chain, Cursor, Read};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::time::Duration;

use etherparse::{IpNumber, LaxNetSlice, LaxSlicedPacket, TcpSlice, UdpSlice};
use pcap_file::PcapError;
use pcap_file::pcap::PcapReader;
use pcap_file::pcapng::blocks::interface_description::{
    InterfaceDescriptionBlock, InterfaceDescriptionOption,
};
use pcap_file::pcapng::{Block, PcapNgReader};

use crate::sparse::Sparse;
use crate::{Error, Packet, Result, Transport};

/// pcapng section header block type, which is byte-order independent.
const PCAPNG_SECTION_HEADER: [u8; 4] = [0x0a, 0x0d, 0x0d, 0x0a];

/// The classic pcap magic in both byte orders and both time resolutions.
const PCAP_MAGICS: [[u8; 4]; 4] = [
    [0xa1, 0xb2, 0xc3, 0xd4],
    [0xd4, 0xc3, 0xb2, 0xa1],
    [0xa1, 0xb2, 0x3c, 0x4d],
    [0x4d, 0x3c, 0xb2, 0xa1],
];

/// Ethernet, the only link type dissected here.
const LINKTYPE_ETHERNET: u32 = 1;

/// pcapng's default timestamp resolution when an interface declares none.
const DEFAULT_TS_DIVISOR: u64 = 1_000_000;

/// How many part-assembled datagrams to hold before dropping the stalest.
///
/// A capture with heavy loss can leave fragments that will never complete;
/// without a bound they accumulate for the length of the file. The bound is
/// far above what a clean capture needs — the corpus never exceeds a handful.
const MAX_PENDING_DATAGRAMS: usize = 1024;

/// Which of the two file formats a capture is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// Classic libpcap: one global header, one link type, flat records.
    Pcap,
    /// pcapng: sections, per-interface link types and timestamp resolutions.
    PcapNg,
}

/// The reader, once the four magic bytes have been put back in front of it.
type Rewound<R> = BufReader<Chain<Cursor<[u8; 4]>, R>>;

enum Source<R: Read> {
    Pcap(Box<PcapReader<Rewound<R>>>),
    PcapNg(Box<PcapNgReader<Rewound<R>>>),
}

/// What a pcapng interface description block tells us about its frames.
#[derive(Clone, Copy, Debug)]
struct Interface {
    link_type: u32,
    /// Timestamp counter ticks per second.
    ts_divisor: u64,
    /// Whether an unsupported link type has already been reported for it, so
    /// a capture on the wrong interface costs one error and not one per frame.
    reported: bool,
}

/// A capture file, read one packet at a time.
///
/// Iterating yields every UDP datagram and TCP segment in the file, in capture
/// order. Frames are read and dissected lazily: a capture costs the underlying
/// reader's 8 MiB window plus one frame plus whatever IP fragments are still
/// being reassembled, and not the size of the file. That matters here — the
/// corpus this crate exists for is 272 MB, one file of it 78 MB.
///
/// # Errors are the caller's to act on
///
/// The item type is [`Result`], and a hard error ends the iteration. That is
/// deliberate: when walking a directory of captures, a file that ends
/// part-way through a record should cost that file and not the run, but
/// *whether* it does is a policy this crate has no business choosing. Match on
/// [`Error::is_truncated`] and decide.
///
/// ```no_run
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use prolink_capture::Capture;
///
/// for entry in std::fs::read_dir("captures")? {
///     let path = entry?.path();
///     let capture = match Capture::open(&path) {
///         Ok(capture) => capture,
///         Err(error) => {
///             eprintln!("{}: {error}", path.display());
///             continue;
///         }
///     };
///     // Every datagram a real player addressed to the status port.
///     for packet in capture.udp_to(50002) {
///         match packet {
///             Ok(packet) => println!("{} bytes", packet.payload.len()),
///             Err(error) => {
///                 eprintln!("{}: {error}", path.display());
///                 break;
///             }
///         }
///     }
/// }
/// # Ok(())
/// # }
/// ```
pub struct Capture<R: Read> {
    source: Source<R>,
    /// Indexed by pcapng interface id. Classic pcap has exactly one entry.
    interfaces: Vec<Interface>,
    defragmenter: Defragmenter,
    index: u64,
    finished: bool,
}

impl Capture<File> {
    /// Open a capture file.
    ///
    /// The format is decided by the first four bytes, not by the extension:
    /// every file in this project's corpus is named `run.pcap` and eleven of
    /// the thirty-seven are pcapng.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(File::open(path)?)
    }
}

impl<R: Read> Capture<R> {
    /// Read a capture from anything byte-shaped.
    ///
    /// Fails when the leading bytes are neither magic, or — for classic pcap,
    /// whose link type is declared once in the global header — when the frames
    /// are not Ethernet.
    pub fn new(mut reader: R) -> Result<Self> {
        // Peek the four bytes the format is decided by, then put them back in
        // front of the reader: a capture may come from a pipe, so seeking back
        // is not available.
        let mut magic = [0u8; 4];
        let mut filled = 0usize;
        while filled < magic.len() {
            let Some(rest) = magic.get_mut(filled..) else {
                break;
            };
            match reader.read(rest) {
                Ok(0) => return Err(Error::NotACapture { magic }),
                Ok(read) => filled = filled.saturating_add(read),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }

        let rewound = BufReader::new(Cursor::new(magic).chain(reader));
        if magic == PCAPNG_SECTION_HEADER {
            let reader = PcapNgReader::new(rewound).map_err(convert)?;
            return Ok(Self::from_source(
                Source::PcapNg(Box::new(reader)),
                Vec::new(),
            ));
        }
        if PCAP_MAGICS.contains(&magic) {
            let reader = PcapReader::new(rewound).map_err(convert)?;
            let link_type = u32::from(reader.header().datalink);
            if link_type != LINKTYPE_ETHERNET {
                return Err(Error::UnsupportedLinkType { link_type });
            }
            let interface = Interface {
                link_type,
                ts_divisor: DEFAULT_TS_DIVISOR,
                reported: false,
            };
            return Ok(Self::from_source(
                Source::Pcap(Box::new(reader)),
                vec![interface],
            ));
        }
        Err(Error::NotACapture { magic })
    }

    fn from_source(source: Source<R>, interfaces: Vec<Interface>) -> Self {
        Self {
            source,
            interfaces,
            defragmenter: Defragmenter::new(),
            index: 0,
            finished: false,
        }
    }

    /// Which of the two formats this file turned out to be.
    pub fn format(&self) -> Format {
        match self.source {
            Source::Pcap(_) => Format::Pcap,
            Source::PcapNg(_) => Format::PcapNg,
        }
    }

    /// Every UDP datagram **addressed to** `port`.
    ///
    /// The destination and nothing else, which is the whole reason this exists
    /// rather than being left to the caller. The Pro DJ Link type byte at
    /// offset `0x0a` is shared across ports and the layouts behind it are not:
    /// `0x06` is a keep-alive on 50000 and a media response on 50002. A tool
    /// that binds one socket and sends its keep-alives *from* 50002
    /// contributes packets that an "either endpoint" filter accepts and the
    /// 50002 decoder reads as confident nonsense — a failure with no error and
    /// no exception, only wrong fields.
    ///
    /// Real hardware makes this easy to miss: of the 35 103 datagrams in this
    /// project's corpus addressed to 50002, only 24 were *sent from* 50002,
    /// because a CDJ sends each status packet from a **different, incrementing
    /// source port** — 4763, 4764, 4765 and so on through a session. Filtering
    /// on the source therefore finds almost nothing, and filtering on either
    /// endpoint looks right until the day a second tool is on the network.
    ///
    /// Zero-length datagrams are not filtered out, because a zero-length
    /// datagram is a thing that was sent and an empty result is not an error.
    pub fn udp_to(self, port: u16) -> UdpTo<R> {
        UdpTo {
            capture: self,
            port,
        }
    }
}

impl<R: Read> std::fmt::Debug for Capture<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("format", &self.format())
            .field("interfaces", &self.interfaces.len())
            .field("frames_read", &self.index)
            .finish_non_exhaustive()
    }
}

impl<R: Read> Iterator for Capture<R> {
    type Item = Result<Packet>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.finished {
            let outcome = match &mut self.source {
                Source::Pcap(reader) => {
                    let Some(frame) = reader.next_packet() else {
                        self.finished = true;
                        return None;
                    };
                    match frame {
                        Ok(frame) => {
                            self.index = self.index.saturating_add(1);
                            dissect(
                                &mut self.defragmenter,
                                &frame.data,
                                0,
                                self.index,
                                frame.timestamp,
                            )
                            .map(Ok)
                        }
                        Err(error) => Some(Err(convert(error))),
                    }
                }
                Source::PcapNg(reader) => {
                    let Some(block) = reader.next_block() else {
                        self.finished = true;
                        return None;
                    };
                    match block {
                        Ok(block) => step_pcapng(
                            block,
                            &mut self.interfaces,
                            &mut self.defragmenter,
                            &mut self.index,
                        ),
                        Err(error) => Some(Err(convert(error))),
                    }
                }
            };
            match outcome {
                Some(Ok(packet)) => return Some(Ok(packet)),
                Some(Err(error)) => {
                    // An unsupported link type concerns one interface of a
                    // multi-interface capture; everything else is fatal.
                    if !matches!(error, Error::UnsupportedLinkType { .. }) {
                        self.finished = true;
                    }
                    return Some(Err(error));
                }
                None => {}
            }
        }
        None
    }
}

/// Consume one pcapng block, which may describe an interface, carry a frame,
/// or be neither.
fn step_pcapng(
    block: Block<'_>,
    interfaces: &mut Vec<Interface>,
    defragmenter: &mut Defragmenter,
    index: &mut u64,
) -> Option<Result<Packet>> {
    match block {
        Block::SectionHeader(_) => {
            // Interface ids restart with every section.
            interfaces.clear();
            None
        }
        Block::InterfaceDescription(idb) => {
            interfaces.push(Interface {
                link_type: u32::from(idb.linktype),
                ts_divisor: timestamp_divisor(&idb),
                reported: false,
            });
            None
        }
        Block::EnhancedPacket(epb) => {
            *index = index.saturating_add(1);
            let interface_id = epb.interface_id;
            let Some(interface) = usize::try_from(interface_id)
                .ok()
                .and_then(|id| interfaces.get_mut(id))
            else {
                return Some(Err(Error::Malformed {
                    reason: format!("packet block names interface {interface_id}, undeclared"),
                }));
            };
            if interface.link_type != LINKTYPE_ETHERNET {
                if interface.reported {
                    return None;
                }
                interface.reported = true;
                return Some(Err(Error::UnsupportedLinkType {
                    link_type: interface.link_type,
                }));
            }
            let timestamp = rescale(epb.timestamp, interface.ts_divisor);
            dissect(defragmenter, &epb.data, interface_id, *index, timestamp).map(Ok)
        }
        Block::SimplePacket(spb) => {
            // A simple packet block records no interface and no time; it
            // belongs to interface 0 by definition and carries no timestamp,
            // which is reported as zero rather than guessed at.
            *index = index.saturating_add(1);
            let link_type = interfaces
                .first()
                .map_or(LINKTYPE_ETHERNET, |i| i.link_type);
            if link_type != LINKTYPE_ETHERNET {
                return None;
            }
            dissect(defragmenter, &spb.data, 0, *index, Duration::ZERO).map(Ok)
        }
        _ => None,
    }
}

/// Ticks per second for a pcapng interface, from its `if_tsresol` option.
///
/// The high bit of the option selects a power of two rather than of ten. An
/// absent option means microseconds, which is what `tcpdump` writes and
/// therefore what every capture in this project's corpus uses.
fn timestamp_divisor(idb: &InterfaceDescriptionBlock<'_>) -> u64 {
    for option in &idb.options {
        if let InterfaceDescriptionOption::IfTsResol(resolution) = *option {
            return if resolution & 0x80 == 0 {
                10u64
                    .checked_pow(u32::from(resolution))
                    .unwrap_or(DEFAULT_TS_DIVISOR)
            } else {
                1u64.checked_shl(u32::from(resolution & 0x7f))
                    .unwrap_or(DEFAULT_TS_DIVISOR)
            };
        }
    }
    DEFAULT_TS_DIVISOR
}

/// Undo `pcap-file`'s assumption that a pcapng timestamp counts nanoseconds.
///
/// It hands the raw 64-bit counter back as `Duration::from_nanos(counter)`, so
/// the counter is recovered from the nanoseconds and rescaled by the interface's
/// real resolution.
fn rescale(raw: Duration, divisor: u64) -> Duration {
    let ticks = raw.as_nanos();
    let divisor = u128::from(divisor);
    let Some(seconds) = ticks.checked_div(divisor) else {
        return Duration::ZERO;
    };
    let nanos = ticks
        .checked_rem(divisor)
        .and_then(|fraction| fraction.checked_mul(1_000_000_000))
        .and_then(|scaled| scaled.checked_div(divisor))
        .unwrap_or(0);
    Duration::new(
        u64::try_from(seconds).unwrap_or(u64::MAX),
        u32::try_from(nanos).unwrap_or(0),
    )
}

/// Ethernet → IPv4 → UDP or TCP, or nothing.
fn dissect(
    defragmenter: &mut Defragmenter,
    frame: &[u8],
    interface: u32,
    index: u64,
    timestamp: Duration,
) -> Option<Packet> {
    let sliced = LaxSlicedPacket::from_ethernet(frame).ok()?;
    let Some(LaxNetSlice::Ipv4(ipv4)) = sliced.net else {
        return None;
    };
    let header = ipv4.header();
    let source_ip = header.source_addr();
    let destination_ip = header.destination_addr();
    let ip_payload = ipv4.payload();

    let assembled;
    let (protocol, payload) = if ip_payload.fragmented {
        let key = FragmentKey {
            interface,
            source: source_ip,
            destination: destination_ip,
            identification: header.identification(),
            protocol: ip_payload.ip_number,
        };
        assembled = defragmenter.add(
            key,
            index,
            u64::from(header.fragments_offset().byte_offset()),
            ip_payload.payload,
            header.more_fragments(),
        )?;
        (ip_payload.ip_number, assembled.as_slice())
    } else {
        (ip_payload.ip_number, ip_payload.payload)
    };

    let segment = parse_transport(protocol, payload)?;
    Some(Packet {
        index,
        timestamp,
        interface,
        source: SocketAddrV4::new(source_ip, segment.source_port),
        destination: SocketAddrV4::new(destination_ip, segment.destination_port),
        transport: segment.transport,
        payload: segment.payload.to_vec(),
    })
}

struct Segment<'a> {
    source_port: u16,
    destination_port: u16,
    transport: Transport,
    payload: &'a [u8],
}

fn parse_transport(protocol: IpNumber, payload: &[u8]) -> Option<Segment<'_>> {
    if protocol == IpNumber::UDP {
        let udp = UdpSlice::from_slice_lax(payload).ok()?;
        return Some(Segment {
            source_port: udp.source_port(),
            destination_port: udp.destination_port(),
            transport: Transport::Udp,
            payload: udp.payload(),
        });
    }
    if protocol == IpNumber::TCP {
        let tcp = TcpSlice::from_slice(payload).ok()?;
        return Some(Segment {
            source_port: tcp.source_port(),
            destination_port: tcp.destination_port(),
            transport: Transport::Tcp {
                sequence: tcp.sequence_number(),
                syn: tcp.syn(),
                fin: tcp.fin(),
                reset: tcp.rst(),
            },
            payload: tcp.payload(),
        });
    }
    None
}

/// The RFC 791 reassembly tuple, plus the capture interface.
///
/// The interface belongs in the key because a capture of a bridge records the
/// same datagram on both of its interfaces, with the same addresses and the
/// same identification. Without it the two copies' fragments would be folded
/// into one datagram, so fragmented traffic would be silently de-duplicated
/// while unfragmented traffic stayed doubled — the two counts would then
/// disagree for a reason nothing in the output would explain.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct FragmentKey {
    interface: u32,
    source: Ipv4Addr,
    destination: Ipv4Addr,
    identification: u16,
    protocol: IpNumber,
}

struct Pending {
    bytes: Sparse,
    /// Total length, known only once the fragment without `more fragments`
    /// arrives — which is not necessarily the last one to be captured.
    total: Option<u64>,
    last_seen: u64,
}

/// Reassembles fragmented IPv4 datagrams.
///
/// No timeout is modelled in seconds, because a capture is finite and a
/// datagram left incomplete at the end of one is not interesting. The only
/// eviction is a bound on how many part-assembled datagrams may be held at
/// once, so a lossy capture cannot grow this without limit.
struct Defragmenter {
    pending: HashMap<FragmentKey, Pending>,
}

impl Defragmenter {
    fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    /// Add one fragment; return the whole datagram once it is complete.
    fn add(
        &mut self,
        key: FragmentKey,
        index: u64,
        offset: u64,
        payload: &[u8],
        more_fragments: bool,
    ) -> Option<Vec<u8>> {
        if self.pending.len() > MAX_PENDING_DATAGRAMS {
            let horizon = index.saturating_sub(
                u64::try_from(MAX_PENDING_DATAGRAMS).unwrap_or(u64::from(u32::MAX)),
            );
            self.pending
                .retain(|_, pending| pending.last_seen >= horizon);
        }

        let entry = self.pending.entry(key).or_insert_with(|| Pending {
            bytes: Sparse::new(),
            total: None,
            last_seen: index,
        });
        entry.last_seen = index;
        entry.bytes.insert(offset, payload);
        if !more_fragments {
            entry.total = Some(offset.saturating_add(payload.len().try_into().unwrap_or(u64::MAX)));
        }

        let total = entry.total?;
        let complete = entry
            .bytes
            .contiguous()
            .is_some_and(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX) >= total);
        if !complete {
            return None;
        }
        let mut assembled = self.pending.remove(&key)?.bytes.contiguous()?.to_vec();
        assembled.truncate(usize::try_from(total).unwrap_or(usize::MAX));
        Some(assembled)
    }
}

/// Every UDP datagram addressed to one port. Built by [`Capture::udp_to`].
#[derive(Debug)]
pub struct UdpTo<R: Read> {
    capture: Capture<R>,
    port: u16,
}

impl<R: Read> Iterator for UdpTo<R> {
    type Item = Result<Packet>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.capture.next()? {
                Ok(packet) => {
                    if packet.transport.is_udp() && packet.destination.port() == self.port {
                        return Some(Ok(packet));
                    }
                }
                Err(error) => return Some(Err(error)),
            }
        }
    }
}

/// Translate the reader's errors into this crate's, keeping the one
/// distinction that matters.
///
/// A file that ends part-way through a record reaches us as an
/// `UnexpectedEof`, not as `IncompleteBuffer`: the underlying reader asks for
/// more bytes, gets none, and reports the read rather than the parse. Both
/// mean the same thing here and both are [`Error::Truncated`], so a corpus
/// walker can tell a `tcpdump` that was killed from a file that is not a
/// capture at all.
fn convert(error: PcapError) -> Error {
    match error {
        PcapError::IncompleteBuffer => Error::Truncated,
        PcapError::IoError(inner) if inner.kind() == std::io::ErrorKind::UnexpectedEof => {
            Error::Truncated
        }
        PcapError::IoError(inner) => Error::Io(inner),
        other => Error::Malformed {
            reason: other.to_string(),
        },
    }
}
