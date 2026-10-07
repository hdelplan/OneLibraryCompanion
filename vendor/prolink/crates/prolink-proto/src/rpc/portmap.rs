// SPDX-License-Identifier: GPL-3.0-only

//! The portmapper — program 100000 v2, UDP 111.
//!
//! The only RPC program at a well-known port. Everything else is registered
//! dynamically and has to be looked up here first, which on a CDJ means mountd
//! (48276) and nfsd (2049).
//!
//! # Serving requires this, and there is no way around it
//!
//! A virtual CDJ *must* bind UDP/111. With the portmapper moved elsewhere and
//! mountd and nfsd bound to exactly the numbers a real player uses, a deck
//! sent `GETPORT` once a second for the rest of the capture — 31 attempts, no
//! sign of giving up — and **never tried 48276 or 2049**, though both were
//! bound and idle. It never opened the dbserver port query on TCP 12523,
//! never opened dbserver, and so never listed us at all (F46).
//!
//! That is worth stating precisely because the failure does not look like a
//! file-access failure. It looks like "we do not appear on LINK", and the
//! whole browse path is downstream of it. Binding a privileged port is
//! therefore a hard requirement of serving, not an optimisation.
//!
//! # What a CDJ registers
//!
//! `rpcinfo` against a CDJ-2000NXS with a stick inserted (F10):
//!
//! ```text
//! program   v  prot   port  name
//!  100003   2  udp    2049  nfs
//!  100005   1  udp   48276  mountd
//!  100000   2  udp     111  portmapper
//! ```
//!
//! Three independent observations across three different devices gave the same
//! numbers (F6).
//!
//! A deck asks for exactly two things and nothing else: `(100005, 1, udp)` and
//! then `(100003, 2, udp)`, mountd always first — 91 of 91 deck-originated
//! `GETPORT` calls in the corpus. It never calls `NULL`, never calls `DUMP`,
//! and of course never calls `SET`. So a server needs only `GETPORT` to satisfy
//! real hardware.
//!
//! `DUMP` is implemented anyway because the two answer different questions when
//! *we* are the client: a `GETPORT` of zero and a portmapper that does not
//! answer at all look identical to a client that asks only the first, and "no
//! RPC stack" and "RPC but nothing exported" lead to different conclusions.

use crate::rpc::xdr;
use crate::rpc::{IpProtocol, Program};
use crate::{Error, Result};

/// The portmapper's program number.
pub const PROGRAM: Program = Program::PORTMAP;

/// The only version anything here speaks.
pub const VERSION: u32 = 2;

/// The well-known port. **Binding this is mandatory to be browsable** (F46).
pub const PORT: u16 = 111;

/// Cap on a `DUMP` listing, so a malformed reply cannot loop or allocate
/// without bound. A CDJ registers three programs; a general-purpose host a few
/// dozen.
const MAX_MAPPINGS: usize = 512;

/// A portmapper procedure number.
///
/// Meaningful only alongside program 100000.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Proc(pub u32);

impl Proc {
    /// Do nothing. The cheapest probe that anything is answering on 111.
    pub const NULL: Self = Self(0);
    /// Register a mapping. A remote caller has no business doing this and we
    /// do not answer it.
    pub const SET: Self = Self(1);
    /// Unregister a mapping. Likewise.
    pub const UNSET: Self = Self(2);
    /// "Which port serves this program?" The gate on everything (F46).
    pub const GETPORT: Self = Self(3);
    /// Everything registered.
    pub const DUMP: Self = Self(4);
    /// Indirect call. Never used here.
    pub const CALLIT: Self = Self(5);

    /// A name for logs, or `None` for a procedure the portmapper does not
    /// define.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::NULL => "NULL",
            Self::SET => "SET",
            Self::UNSET => "UNSET",
            Self::GETPORT => "GETPORT",
            Self::DUMP => "DUMP",
            Self::CALLIT => "CALLIT",
            _ => return None,
        })
    }
}

impl core::fmt::Debug for Proc {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "portmap::Proc({})", self.0),
        }
    }
}

/// One `(program, version, protocol, port)` registration.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mapping {
    /// Which program.
    pub program: Program,
    /// Which version of it.
    pub version: u32,
    /// Which transport. Always [`IpProtocol::UDP`] on a CDJ.
    pub protocol: IpProtocol,
    /// The port it answers on. **Zero means "not registered"** in a `GETPORT`
    /// reply; in a `GETPORT` *request* this field is ignored entirely.
    pub port: u32,
}

impl Mapping {
    /// The mapping to look up, with the ignored `port` field zeroed.
    pub fn query(program: Program, version: u32, protocol: IpProtocol) -> Self {
        Self {
            program,
            version,
            protocol,
            port: 0,
        }
    }

    /// A registration a server is publishing.
    pub fn registered(program: Program, version: u32, protocol: IpProtocol, port: u16) -> Self {
        Self {
            program,
            version,
            protocol,
            port: u32::from(port),
        }
    }

    fn write(&self, out: &mut xdr::Writer) {
        out.u32(self.program.0);
        out.u32(self.version);
        out.u32(self.protocol.0);
        out.u32(self.port);
    }

    fn read(input: &mut xdr::Reader<'_>) -> Result<Self> {
        Ok(Self {
            program: Program(input.u32()?),
            version: input.u32()?,
            protocol: IpProtocol(input.u32()?),
            port: input.u32()?,
        })
    }
}

/// The three registrations a CDJ publishes, with the ports substituted.
///
/// The observed table is portmapper 100000 v2, mountd 100005 v1 and nfsd
/// 100003 v2, all UDP (F10). A server that has had to fall back to an
/// ephemeral port for mountd or nfsd — because a real `rpcbind` already holds
/// the number — still answers correctly, since that is what a portmapper is
/// for.
pub fn cdj_registrations(portmap_port: u16, mount_port: u16, nfs_port: u16) -> [Mapping; 3] {
    [
        Mapping::registered(
            Program::NFS,
            super::nfs2::VERSION,
            IpProtocol::UDP,
            nfs_port,
        ),
        Mapping::registered(
            Program::MOUNT,
            super::mount::VERSION,
            IpProtocol::UDP,
            mount_port,
        ),
        Mapping::registered(Program::PORTMAP, VERSION, IpProtocol::UDP, portmap_port),
    ]
}

/// One portmapper call's arguments.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Request {
    /// `NULL`: no arguments.
    Null,
    /// `GETPORT`: the mapping to resolve.
    GetPort(Mapping),
    /// `DUMP`: no arguments.
    Dump,
    /// A procedure this crate does not model, including `SET` and `UNSET`,
    /// which a remote caller has no business invoking.
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
            Self::GetPort(_) => Proc::GETPORT,
            Self::Dump => Proc::DUMP,
            Self::Unknown { procedure, .. } => *procedure,
        }
    }

    /// Encode the argument block that follows an RPC call header.
    pub fn encode_arguments(&self) -> Vec<u8> {
        let mut out = xdr::Writer::with_capacity(16);
        match self {
            Self::Null | Self::Dump => {}
            Self::GetPort(mapping) => mapping.write(&mut out),
            Self::Unknown { arguments, .. } => out.raw(arguments),
        }
        out.into_bytes()
    }

    /// Decode the argument block of a call to `procedure`.
    pub fn parse(procedure: Proc, arguments: &[u8]) -> Result<Self> {
        Ok(match procedure {
            Proc::NULL => Self::Null,
            Proc::DUMP => Self::Dump,
            Proc::GETPORT => Self::GetPort(Mapping::read(&mut xdr::Reader::new(arguments))?),
            other => Self::Unknown {
                procedure: other,
                arguments: arguments.to_vec(),
            },
        })
    }
}

/// One portmapper reply's results.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Response {
    /// `NULL`: no results.
    Null,
    /// `GETPORT`. **`None` is the wire's zero: the program is not
    /// registered** — a successful reply, not an error, and the answer a
    /// portmapper gives for a slot with no medium in it.
    GetPort(Option<u16>),
    /// `DUMP`: everything registered, in the order the server listed it.
    Dump(Vec<Mapping>),
}

impl Response {
    /// Which procedure this answers.
    pub fn procedure(&self) -> Proc {
        match self {
            Self::Null => Proc::NULL,
            Self::GetPort(_) => Proc::GETPORT,
            Self::Dump(_) => Proc::DUMP,
        }
    }

    /// Encode the result block that follows an RPC reply header.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = xdr::Writer::with_capacity(32);
        match self {
            Self::Null => {}
            Self::GetPort(port) => out.u32(u32::from(port.unwrap_or(0))),
            Self::Dump(mappings) => {
                // An XDR linked list: each element preceded by "a value
                // follows", a false ending it.
                for mapping in mappings {
                    out.bool(true);
                    mapping.write(&mut out);
                }
                out.bool(false);
            }
        }
        out.into_bytes()
    }

    /// Decode the result block of a reply to `procedure`.
    pub fn parse(procedure: Proc, results: &[u8]) -> Result<Self> {
        let mut input = xdr::Reader::new(results);
        Ok(match procedure {
            Proc::NULL => Self::Null,
            Proc::GETPORT => {
                let raw = input.u32()?;
                if raw == 0 {
                    Self::GetPort(None)
                } else {
                    Self::GetPort(Some(u16::try_from(raw).map_err(|_| {
                        Error::malformed(0, format!("{raw} is not a port number"))
                    })?))
                }
            }
            Proc::DUMP => {
                let mut mappings = Vec::new();
                while input.bool()? {
                    mappings.push(Mapping::read(&mut input)?);
                    // `>`, not `>=`: a listing of exactly the cap is legal,
                    // and refusing it would mean our own encoder produces
                    // replies our own decoder rejects.
                    if mappings.len() > MAX_MAPPINGS {
                        return Err(Error::ImplausibleLength {
                            what: "a portmap DUMP listing",
                            length: u64::try_from(mappings.len()).unwrap_or(u64::MAX),
                            limit: u64::try_from(MAX_MAPPINGS).unwrap_or(u64::MAX),
                        });
                    }
                }
                Self::Dump(mappings)
            }
            other => {
                return Err(Error::malformed(
                    0,
                    format!("no reply decoder for portmap procedure {other:?}"),
                ));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::mount;

    #[test]
    fn getport_arguments_are_four_words() {
        let request = Request::GetPort(Mapping::query(Program::NFS, 2, IpProtocol::UDP));
        let args = request.encode_arguments();
        assert_eq!(
            args,
            [
                0x00, 0x01, 0x86, 0xa3, // 100003
                0x00, 0x00, 0x00, 0x02, // version 2
                0x00, 0x00, 0x00, 0x11, // IPPROTO_UDP
                0x00, 0x00, 0x00, 0x00, // port, ignored in a query
            ]
        );
        assert_eq!(Request::parse(Proc::GETPORT, &args).unwrap(), request);
    }

    /// F46's timeline starts here: the deck asks for mountd, then nfsd, then
    /// mounts. If nothing answers this, nothing else happens.
    #[test]
    fn a_deck_asks_for_mountd_by_program_number_not_by_port() {
        let request = Request::GetPort(Mapping::query(Program::MOUNT, 1, IpProtocol::UDP));
        assert_eq!(
            request.encode_arguments().get(..8),
            Some([0x00, 0x01, 0x86, 0xa5, 0x00, 0x00, 0x00, 0x01].as_slice()),
            "100005 v1"
        );
    }

    #[test]
    fn a_getport_reply_is_a_single_word() {
        let encoded = Response::GetPort(Some(2049)).encode();
        assert_eq!(encoded, 2049u32.to_be_bytes());
        assert_eq!(
            Response::parse(Proc::GETPORT, &encoded).unwrap(),
            Response::GetPort(Some(2049))
        );
    }

    /// Zero is not an error and not a port; it is "that program is not
    /// registered", which is the honest answer for an empty media slot.
    #[test]
    fn getport_zero_means_the_program_is_not_registered() {
        assert_eq!(
            Response::parse(Proc::GETPORT, &[0, 0, 0, 0]).unwrap(),
            Response::GetPort(None)
        );
        assert_eq!(Response::GetPort(None).encode(), [0, 0, 0, 0]);
    }

    #[test]
    fn a_port_outside_sixteen_bits_is_refused() {
        let error = Response::parse(Proc::GETPORT, &[0x00, 0x01, 0x00, 0x00]).unwrap_err();
        assert!(matches!(error, Error::Malformed { .. }), "{error:?}");
    }

    /// The exact table `rpcinfo` printed against a CDJ-2000NXS (F10), and
    /// three devices agreed on the port numbers (F6).
    #[test]
    fn a_dump_reply_reproduces_the_observed_cdj_registration_table() {
        let mappings = cdj_registrations(PORT, mount::PIONEER_PORT, super::super::nfs2::PORT);
        assert_eq!(
            mappings,
            [
                Mapping::registered(Program::NFS, 2, IpProtocol::UDP, 2049),
                Mapping::registered(Program::MOUNT, 1, IpProtocol::UDP, 48276),
                Mapping::registered(Program::PORTMAP, 2, IpProtocol::UDP, 111),
            ]
        );

        let encoded = Response::Dump(mappings.to_vec()).encode();
        assert_eq!(
            encoded.len(),
            3 * (4 + 16) + 4,
            "three elements, each behind a value-follows word, then a false"
        );
        assert_eq!(
            encoded.get(encoded.len() - 4..),
            Some([0, 0, 0, 0].as_slice()),
            "the list terminator"
        );
        assert_eq!(
            Response::parse(Proc::DUMP, &encoded).unwrap(),
            Response::Dump(mappings.to_vec())
        );
    }

    #[test]
    fn an_empty_dump_is_a_bare_false() {
        assert_eq!(Response::Dump(Vec::new()).encode(), [0, 0, 0, 0]);
        assert_eq!(
            Response::parse(Proc::DUMP, &[0, 0, 0, 0]).unwrap(),
            Response::Dump(Vec::new())
        );
    }

    #[test]
    fn null_takes_and_returns_nothing() {
        assert!(Request::Null.encode_arguments().is_empty());
        assert_eq!(Request::parse(Proc::NULL, &[]).unwrap(), Request::Null);
        assert!(Response::Null.encode().is_empty());
    }

    #[test]
    fn set_and_unset_decode_as_unknown_so_they_can_be_refused() {
        for proc in [Proc::SET, Proc::UNSET, Proc::CALLIT] {
            let request = Request::parse(proc, &[9; 16]).unwrap();
            assert_eq!(request.procedure(), proc);
            assert!(matches!(request, Request::Unknown { .. }));
        }
    }

    #[test]
    fn a_truncated_getport_is_truncation_not_garbage() {
        let error = Request::parse(Proc::GETPORT, &[0, 0, 0, 1]).unwrap_err();
        assert!(error.is_truncated(), "{error:?}");
    }

    #[test]
    fn the_portmapper_is_on_the_well_known_port() {
        assert_eq!(PROGRAM.0, 100_000);
        assert_eq!(VERSION, 2);
        assert_eq!(PORT, 111);
    }
}
