// SPDX-License-Identifier: GPL-3.0-only

//! The MOUNT protocol — program 100005 v1, UDP 48276 on a Pioneer player.
//!
//! `MNT` turns an export path into the 32-byte root filehandle every
//! subsequent `LOOKUP` starts from. That is the whole reason this program
//! exists, and on a CDJ it is essentially the only procedure that matters.
//!
//! # The port is not a registered number, and it is still the same everywhere
//!
//! mountd answers on **48276**, which is not assigned to anything. Three
//! independent observations across three different devices gave that number
//! (F6), which makes it look like a Pioneer constant rather than a per-boot
//! allocation — but portmap discovery is still how a client should find it,
//! and a deck talking to *us* certainly does (F46).
//!
//! # Exports
//!
//! | Slot | Export |
//! |---|---|
//! | SD | `/B/` |
//! | USB | `/C/` |
//! | rekordbox collection | `/` |
//!
//! USB was confirmed on a CDJ-2000NXS in F12 and SD in F37. But one capture
//! shows the same player mounting `/C/` on one peer and **`/C/EXPORT`** on
//! another in the same session, so the drive-letter prefix identifies the slot
//! and the remainder varies by device or firmware (C6). Match on the prefix:
//! [`slot_for_export`] does, and hardcoding the whole string fails against
//! half the devices in that capture.
//!
//! Only a *populated* slot is listed. A deck with no SD card offers no `/B/`.
//!
//! # A real deck never calls `EXPORT`
//!
//! Not once in any session: it goes straight to `MNT` with the documented path
//! (F37). Enumerating is still the more robust *client* behaviour — it is what
//! survives C6 — but a server that answers only `MNT` satisfies real hardware,
//! which is worth knowing before treating `EXPORT` as load-bearing.
//!
//! Real players **do** call `UMNT`, once per slot, following the physical
//! eject: ejecting SD then USB produced `UMNT('/B/')` then `UMNT('/C/')`
//! twelve seconds apart (C9, F37). The pre-hardware literature lists it as
//! unused.
//!
//! # One structure, two string encodings
//!
//! In an `EXPORT` reply the directory path is UTF-16LE and the group names are
//! plain **ASCII** (C7). Pioneer's convention is not applied uniformly even
//! within a single reply; decoding the group as UTF-16LE turns
//! `169.254.244.181/255.255.255.255` into CJK mojibake. This was flagged in
//! the reference implementation as an explicit assumption *before* the capture
//! was taken, and the capture falsified it.
//!
//! The groups are `host/netmask` pairs naming the clients permitted to mount.
//! A CDJ-2000NXS exports to `169.254.0.0/255.255.0.0` — the entire link-local
//! range — and *that* is the mechanism behind passive access: a host that has
//! never announced itself is inside the permitted set by default (F11, F12).
//! One device in another capture instead listed two per-host entries, so treat
//! [`Status::ACCES`] on `MNT` as "try announcing first", not as fatal.
//!
//! # The status numbering is NFS's
//!
//! `MNT` answers with an `fhstatus`, whose non-zero values are the same
//! [`Status`] codes NFS uses. There is no separate MOUNT error table.

use crate::rpc::Program;
use crate::rpc::nfs2::{ErrorStatus, FHANDLE_LEN, FileHandle, NfsResult, Status};
use crate::rpc::xdr::{self, Utf16LeString};
use crate::{Error, Result, Slot};

/// The MOUNT program number.
pub const PROGRAM: Program = Program::MOUNT;

/// The only version anything in this protocol speaks.
pub const VERSION: u32 = 1;

/// The port a real player answers on (F6).
///
/// Not a registered number. Discover it through the portmapper rather than
/// assuming it — but it is stable enough to *serve* on when impersonating a
/// player, so a client that skips discovery still finds us.
pub const PIONEER_PORT: u16 = 48276;

/// The export a CDJ serves its SD card as (F37).
pub const EXPORT_SD: &str = "/B/";
/// The export a CDJ serves its USB slot as (F12).
pub const EXPORT_USB: &str = "/C/";
/// The export a rekordbox collection is served as.
pub const EXPORT_REKORDBOX: &str = "/";

/// Cap on an `EXPORT` or `DUMP` listing, so a malformed reply cannot loop or
/// allocate without bound.
const MAX_ENTRIES: usize = 64;

/// A MOUNT procedure number.
///
/// Meaningful only alongside program 100005: NFS's `GETATTR` is also `1`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Proc(pub u32);

impl Proc {
    /// Do nothing.
    pub const NULL: Self = Self(0);
    /// Mount an export and return its root filehandle. The one procedure a
    /// real deck actually calls (F37).
    pub const MNT: Self = Self(1);
    /// List who has mounted what.
    pub const DUMP: Self = Self(2);
    /// Unmount one export. Real players do call this, per slot, after a
    /// physical eject (C9).
    pub const UMNT: Self = Self(3);
    /// Unmount everything this client has mounted.
    pub const UMNTALL: Self = Self(4);
    /// List the exports on offer. **Never called by a real deck** (F37).
    pub const EXPORT: Self = Self(5);

    /// A name for logs, or `None` for a procedure MOUNT v1 does not define.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::NULL => "NULL",
            Self::MNT => "MNT",
            Self::DUMP => "DUMP",
            Self::UMNT => "UMNT",
            Self::UMNTALL => "UMNTALL",
            Self::EXPORT => "EXPORT",
            _ => return None,
        })
    }
}

impl core::fmt::Debug for Proc {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "mount::Proc({})", self.0),
        }
    }
}

/// The documented export path for a slot, or `None` for a slot that is never
/// exported.
///
/// A *fallback*. Prefer [`slot_for_export`] against a path a device actually
/// named, because the spelling varies (C6). The CD slot has no export; `/A/`
/// is used by no observed client and is presumed internal.
pub fn export_path_for(slot: Slot) -> Option<&'static str> {
    Some(match slot {
        Slot::SD => EXPORT_SD,
        Slot::USB => EXPORT_USB,
        Slot::REKORDBOX => EXPORT_REKORDBOX,
        _ => return None,
    })
}

/// The slot an export path names, matched on the drive-letter **prefix**.
///
/// Returns the parsed slot rather than a boolean, so nothing downstream
/// re-derives it. Prefix matching is what survives C6: one capture shows the
/// same player mounting `/C/` on one peer and `/C/EXPORT` on another, and a
/// client that hardcoded the whole string failed against half the devices in
/// it.
///
/// `/` is the rekordbox collection and is matched exactly, because it prefixes
/// every other path and would otherwise swallow them.
pub fn slot_for_export(path: &str) -> Option<Slot> {
    if path.starts_with(EXPORT_SD) {
        Some(Slot::SD)
    } else if path.starts_with(EXPORT_USB) {
        Some(Slot::USB)
    } else if path == EXPORT_REKORDBOX {
        Some(Slot::REKORDBOX)
    } else {
        None
    }
}

/// One entry in an `EXPORT` listing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Export {
    /// The export path, kept in its wire form. **UTF-16LE** (C7).
    ///
    /// Held as bytes rather than as a decoded string because the `/B/` and
    /// `/C/` names were confirmed only against XDJ-class hardware before F12,
    /// and what a device actually says has to be recordable verbatim rather
    /// than only as our reading of it.
    pub path: Utf16LeString,
    /// The clients permitted to mount it, as `host/netmask` pairs. **Plain
    /// ASCII**, in the same reply as a UTF-16LE path (C7).
    pub groups: Vec<String>,
}

impl Export {
    /// An export offered to everyone, which is what a CDJ-2000NXS does in
    /// practice by naming the whole link-local subnet (F12).
    pub fn new(path: &str, groups: &[&str]) -> Self {
        Self {
            path: Utf16LeString::new(path),
            groups: groups.iter().map(|group| (*group).to_owned()).collect(),
        }
    }

    /// The access list a CDJ-2000NXS was observed publishing, and the
    /// mechanism behind passive access (F11, F12).
    pub const LINK_LOCAL_SUBNET: &'static str = "169.254.0.0/255.255.0.0";
}

/// One entry in a `DUMP` listing: who has mounted what.
///
/// **Inferred, not captured.** No capture in the corpus contains a MOUNT
/// `DUMP`, so the encoding of the two fields is taken from RFC 1094 Appendix A
/// plus the convention `EXPORT` uses — an ASCII hostname and a UTF-16LE
/// directory. Nothing depends on it; a deck never calls this.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MountEntry {
    /// The client holding the mount.
    pub hostname: String,
    /// What it has mounted.
    pub directory: Utf16LeString,
}

/// One MOUNT call's arguments.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Request {
    /// `NULL`: no arguments.
    Null,
    /// `MNT`: the export path, UTF-16LE.
    Mnt(Utf16LeString),
    /// `DUMP`: no arguments.
    Dump,
    /// `UMNT`: the export path, UTF-16LE. Real players send these after an
    /// eject (C9).
    Umnt(Utf16LeString),
    /// `UMNTALL`: no arguments.
    UmntAll,
    /// `EXPORT`: no arguments. Never sent by a real deck (F37).
    Export,
    /// A procedure this crate does not model.
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
            Self::Mnt(_) => Proc::MNT,
            Self::Dump => Proc::DUMP,
            Self::Umnt(_) => Proc::UMNT,
            Self::UmntAll => Proc::UMNTALL,
            Self::Export => Proc::EXPORT,
            Self::Unknown { procedure, .. } => *procedure,
        }
    }

    /// The export path this call names, if it names one.
    pub fn path(&self) -> Option<&Utf16LeString> {
        match self {
            Self::Mnt(path) | Self::Umnt(path) => Some(path),
            _ => None,
        }
    }

    /// Encode the argument block that follows an RPC call header.
    pub fn encode_arguments(&self) -> Vec<u8> {
        let mut out = xdr::Writer::with_capacity(16);
        match self {
            Self::Null | Self::Dump | Self::UmntAll | Self::Export => {}
            Self::Mnt(path) | Self::Umnt(path) => out.utf16le_string(path),
            Self::Unknown { arguments, .. } => out.raw(arguments),
        }
        out.into_bytes()
    }

    /// Decode the argument block of a call to `procedure`.
    pub fn parse(procedure: Proc, arguments: &[u8]) -> Result<Self> {
        let mut input = xdr::Reader::new(arguments);
        Ok(match procedure {
            Proc::NULL => Self::Null,
            Proc::DUMP => Self::Dump,
            Proc::UMNTALL => Self::UmntAll,
            Proc::EXPORT => Self::Export,
            Proc::MNT => Self::Mnt(input.utf16le_string(xdr::MAX_STRING)?),
            Proc::UMNT => Self::Umnt(input.utf16le_string(xdr::MAX_STRING)?),
            other => Self::Unknown {
                procedure: other,
                arguments: arguments.to_vec(),
            },
        })
    }
}

/// One MOUNT reply's results.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Response {
    /// `NULL`: no results.
    Null,
    /// `MNT`: the root filehandle, or the reason there is none.
    ///
    /// `Err(`[`Status::ACCES`]`)` means "try announcing first" rather than
    /// "give up" — see the module documentation.
    Mnt(NfsResult<FileHandle>),
    /// `DUMP`: who has mounted what.
    Dump(Vec<MountEntry>),
    /// `UMNT`: no results.
    Umnt,
    /// `UMNTALL`: no results.
    UmntAll,
    /// `EXPORT`: what is on offer.
    Export(Vec<Export>),
}

impl Response {
    /// Which procedure this answers.
    pub fn procedure(&self) -> Proc {
        match self {
            Self::Null => Proc::NULL,
            Self::Mnt(_) => Proc::MNT,
            Self::Dump(_) => Proc::DUMP,
            Self::Umnt => Proc::UMNT,
            Self::UmntAll => Proc::UMNTALL,
            Self::Export(_) => Proc::EXPORT,
        }
    }

    /// Encode the result block that follows an RPC reply header.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = xdr::Writer::with_capacity(48);
        match self {
            Self::Null | Self::Umnt | Self::UmntAll => {}
            Self::Mnt(Ok(handle)) => {
                out.u32(Status::OK.0);
                out.opaque_fixed(handle.as_bytes());
            }
            Self::Mnt(Err(status)) => out.u32(status.status().0),
            Self::Dump(entries) => {
                for entry in entries {
                    out.bool(true);
                    out.ascii_string(&entry.hostname);
                    out.utf16le_string(&entry.directory);
                }
                out.bool(false);
            }
            Self::Export(exports) => {
                for export in exports {
                    out.bool(true);
                    // The path is UTF-16LE and the groups are ASCII, in the
                    // same structure. See C7 and the module documentation.
                    out.utf16le_string(&export.path);
                    for group in &export.groups {
                        out.bool(true);
                        out.ascii_string(group);
                    }
                    out.bool(false);
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
            Proc::UMNT => Self::Umnt,
            Proc::UMNTALL => Self::UmntAll,
            Proc::MNT => match ErrorStatus::new(Status(input.u32()?)) {
                // Zero is the only status followed by a filehandle.
                None => Self::Mnt(Ok(FileHandle::parse(input.opaque_fixed(FHANDLE_LEN)?)?)),
                Some(status) => Self::Mnt(Err(status)),
            },
            Proc::DUMP => Self::Dump(parse_dump(&mut input)?),
            Proc::EXPORT => Self::Export(parse_exports(&mut input)?),
            other => {
                return Err(Error::malformed(
                    0,
                    format!("no reply decoder for MOUNT procedure {other:?}"),
                ));
            }
        })
    }
}

fn parse_dump(input: &mut xdr::Reader<'_>) -> Result<Vec<MountEntry>> {
    let mut entries = Vec::new();
    while input.bool()? {
        entries.push(MountEntry {
            hostname: input.ascii_string(xdr::MAX_STRING)?,
            directory: input.utf16le_string(xdr::MAX_STRING)?,
        });
        if entries.len() > MAX_ENTRIES {
            return Err(too_many("a MOUNT DUMP listing", entries.len()));
        }
    }
    Ok(entries)
}

fn parse_exports(input: &mut xdr::Reader<'_>) -> Result<Vec<Export>> {
    let mut exports = Vec::new();
    while input.bool()? {
        let path = input.utf16le_string(xdr::MAX_STRING)?;
        let mut groups = Vec::new();
        while input.bool()? {
            // ASCII, deliberately not the UTF-16LE the path beside it uses.
            groups.push(input.ascii_string(xdr::MAX_STRING)?);
            if groups.len() > MAX_ENTRIES {
                return Err(too_many("an EXPORT group list", groups.len()));
            }
        }
        exports.push(Export { path, groups });
        if exports.len() > MAX_ENTRIES {
            return Err(too_many("an EXPORT listing", exports.len()));
        }
    }
    Ok(exports)
}

/// The cap is on what we will *accept*, so it is checked after the push and
/// against `>`: a listing of exactly [`MAX_ENTRIES`] is legal, and refusing it
/// would mean our own encoder could produce a reply our own decoder rejects.
fn too_many(what: &'static str, seen: usize) -> Error {
    Error::ImplausibleLength {
        what,
        length: u64::try_from(seen).unwrap_or(u64::MAX),
        limit: u64::try_from(MAX_ENTRIES).unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT_HANDLE: [u8; 32] = [
        0x8a, 0x5e, 0xda, 0xb2, 0x82, 0x63, 0x24, 0x43, 0x21, 0x9e, 0x05, 0x1e, 0x4a, 0xde, 0x2d,
        0x1d, 0x5b, 0xbc, 0x67, 0x1c, 0x78, 0x10, 0x51, 0xbf, 0x14, 0x37, 0x89, 0x7c, 0xbd, 0xfe,
        0xa0, 0xf1,
    ];

    /// The one call a real deck actually makes here (F37), byte for byte. The
    /// six-byte prefix is F12's `raw=2f0043002f00` in its wire framing.
    #[test]
    fn a_mnt_call_carries_a_utf16le_path_counted_in_bytes() {
        let request = Request::Mnt(Utf16LeString::new(EXPORT_USB));
        assert_eq!(
            request.encode_arguments(),
            [
                0x00, 0x00, 0x00, 0x06, 0x2f, 0x00, 0x43, 0x00, 0x2f, 0x00, 0x00, 0x00
            ]
        );
        assert_eq!(
            Request::parse(Proc::MNT, &request.encode_arguments()).unwrap(),
            request
        );
    }

    #[test]
    fn a_mnt_reply_is_a_status_then_a_bare_filehandle() {
        let response = Response::Mnt(Ok(FileHandle(ROOT_HANDLE)));
        let encoded = response.encode();
        assert_eq!(encoded.len(), 4 + 32);
        assert_eq!(encoded.get(..4), Some([0, 0, 0, 0].as_slice()));
        assert_eq!(encoded.get(4..), Some(ROOT_HANDLE.as_slice()));
        assert_eq!(Response::parse(Proc::MNT, &encoded).unwrap(), response);
    }

    /// F12's caveat: a device whose export list is per-host would refuse an
    /// unannounced client, so this is "announce and retry", not fatal.
    #[test]
    fn a_refused_mount_carries_only_a_status() {
        let response = Response::Mnt(Err(ErrorStatus::ACCES));
        assert_eq!(response.encode(), [0, 0, 0, 13]);
        assert_eq!(
            Response::parse(Proc::MNT, &[0, 0, 0, 13]).unwrap(),
            response
        );
    }

    /// C9/F37: ejecting SD then USB produced these two calls, twelve seconds
    /// apart. `research/06` lists UMNT as unused.
    #[test]
    fn umnt_is_a_mnt_by_another_procedure_number() {
        let sd = Request::Umnt(Utf16LeString::new(EXPORT_SD));
        let usb = Request::Umnt(Utf16LeString::new(EXPORT_USB));
        assert_eq!(sd.procedure(), Proc::UMNT);
        assert_eq!(
            sd.encode_arguments(),
            Request::Mnt(Utf16LeString::new(EXPORT_SD)).encode_arguments(),
            "same argument shape, different procedure"
        );
        assert_eq!(
            Request::parse(Proc::UMNT, &usb.encode_arguments()).unwrap(),
            usb
        );
        assert!(Response::Umnt.encode().is_empty());
    }

    /// C7, the correction a capture forced: one structure, two encodings.
    #[test]
    fn an_export_reply_has_a_utf16le_path_and_ascii_groups() {
        let export = Export::new(EXPORT_USB, &[Export::LINK_LOCAL_SUBNET]);
        let encoded = Response::Export(vec![export.clone()]).encode();
        assert_eq!(
            encoded.get(..4),
            Some([0, 0, 0, 1].as_slice()),
            "a value follows"
        );
        assert_eq!(
            encoded.get(4..16),
            Some(
                [
                    0x00, 0x00, 0x00, 0x06, 0x2f, 0x00, 0x43, 0x00, 0x2f, 0x00, 0x00, 0x00
                ]
                .as_slice()
            ),
            "the path is UTF-16LE, six bytes for three characters"
        );
        assert_eq!(
            encoded.get(20..24),
            Some(23u32.to_be_bytes().as_slice()),
            "the group is ASCII, so 23 characters announce 23 bytes, not 46"
        );
        assert_eq!(
            encoded.get(24..47),
            Some(b"169.254.0.0/255.255.0.0".as_slice()),
            "one byte per character; decoding this as UTF-16LE gives mojibake"
        );

        let parsed = Response::parse(Proc::EXPORT, &encoded).unwrap();
        assert_eq!(parsed, Response::Export(vec![export]));
        let Response::Export(exports) = parsed else {
            panic!("expected an export listing");
        };
        assert_eq!(
            exports.first().map(|e| e.path.to_string_lossy()),
            Some("/C/".to_owned())
        );
        assert_eq!(
            exports
                .first()
                .and_then(|e| e.groups.first())
                .map(String::as_str),
            Some("169.254.0.0/255.255.0.0"),
            "the whole link-local subnet: the mechanism behind passive access"
        );
    }

    #[test]
    fn an_export_with_two_per_host_groups_round_trips() {
        // The other device in the corpus scoped its export this way, which is
        // the case that would refuse an unannounced client (F12's caveat).
        let export = Export::new(
            "/C/EXPORT",
            &[
                "169.254.244.181/255.255.255.255",
                "169.254.192.112/255.255.255.255",
            ],
        );
        let encoded = Response::Export(vec![export.clone()]).encode();
        assert_eq!(
            Response::parse(Proc::EXPORT, &encoded).unwrap(),
            Response::Export(vec![export])
        );
    }

    #[test]
    fn an_empty_export_list_is_a_bare_false() {
        assert_eq!(Response::Export(Vec::new()).encode(), [0, 0, 0, 0]);
        assert_eq!(
            Response::parse(Proc::EXPORT, &[0, 0, 0, 0]).unwrap(),
            Response::Export(Vec::new())
        );
    }

    /// C6: `/C/` on one peer and `/C/EXPORT` on another, in the same session.
    #[test]
    fn an_export_path_is_matched_on_its_prefix() {
        assert_eq!(slot_for_export("/C/"), Some(Slot::USB));
        assert_eq!(slot_for_export("/C/EXPORT"), Some(Slot::USB));
        assert_eq!(slot_for_export("/B/"), Some(Slot::SD));
        assert_eq!(slot_for_export("/B/EXPORT"), Some(Slot::SD));
        assert_eq!(slot_for_export("/"), Some(Slot::REKORDBOX));
        assert_eq!(
            slot_for_export("/A/"),
            None,
            "used by no observed client and presumed internal"
        );
        assert_eq!(slot_for_export(""), None);
    }

    #[test]
    fn the_rekordbox_export_does_not_swallow_the_others() {
        // "/" prefixes every path there is, so it is matched exactly and last.
        assert_eq!(slot_for_export("/C/"), Some(Slot::USB));
        assert_ne!(slot_for_export("/C/"), Some(Slot::REKORDBOX));
    }

    #[test]
    fn the_documented_export_table_round_trips_through_the_matcher() {
        for slot in [Slot::SD, Slot::USB, Slot::REKORDBOX] {
            let path = export_path_for(slot).unwrap();
            assert_eq!(slot_for_export(path), Some(slot), "{slot:?} -> {path}");
        }
        assert_eq!(export_path_for(Slot::CD), None, "the CD slot has no export");
        assert_eq!(export_path_for(Slot::NONE), None);
    }

    #[test]
    fn a_dump_listing_round_trips() {
        let entries = vec![MountEntry {
            hostname: "169.254.99.100".to_owned(),
            directory: Utf16LeString::new(EXPORT_USB),
        }];
        let encoded = Response::Dump(entries.clone()).encode();
        assert_eq!(
            Response::parse(Proc::DUMP, &encoded).unwrap(),
            Response::Dump(entries)
        );
    }

    #[test]
    fn export_and_umntall_take_no_arguments() {
        for request in [
            Request::Export,
            Request::UmntAll,
            Request::Dump,
            Request::Null,
        ] {
            assert!(request.encode_arguments().is_empty());
            assert_eq!(Request::parse(request.procedure(), &[]).unwrap(), request);
        }
    }

    #[test]
    fn a_truncated_mnt_path_is_truncation_not_garbage() {
        let error = Request::parse(Proc::MNT, &[0, 0, 0, 6, 0x2f]).unwrap_err();
        assert!(error.is_truncated(), "{error:?}");
    }

    #[test]
    fn a_hostile_mnt_path_length_is_refused_before_allocating() {
        let error = Request::parse(Proc::MNT, &[0xff, 0xff, 0xff, 0xff]).unwrap_err();
        assert!(
            matches!(error, Error::ImplausibleLength { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn the_observed_port_and_program_numbers() {
        assert_eq!(PROGRAM.0, 100_005);
        assert_eq!(VERSION, 1);
        assert_eq!(PIONEER_PORT, 48276, "three devices, same number (F6)");
    }
}
