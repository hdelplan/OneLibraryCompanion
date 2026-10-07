// SPDX-License-Identifier: GPL-3.0-only

//! What can go wrong once sockets are involved.

use std::net::Ipv4Addr;

use prolink_proto::DeviceNumber;

/// An error from the Pro DJ Link runtime.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A socket operation failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being attempted.
        context: &'static str,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },

    /// The system's interface list could not be read.
    #[error("cannot enumerate network interfaces: {0}")]
    Interfaces(String),

    /// The named interface does not exist, or has no IPv4 address and MAC.
    #[error("no interface named {name} with both an IPv4 address and a MAC")]
    NoSuchInterface {
        /// The name that was asked for.
        name: String,
    },

    /// Nothing on this host could carry Pro DJ Link traffic.
    #[error("no usable network interface")]
    NoUsableInterface,

    /// A server was asked to serve no media.
    ///
    /// An error rather than an empty server, because a device that advertises
    /// no slots is one no player will ever ask about (F24), so it would sit
    /// there looking correct and doing nothing.
    #[error("nothing to serve: give at least one medium")]
    NothingToServe,

    /// A peer sent something this crate could not decode.
    #[error(transparent)]
    Protocol(#[from] prolink_proto::Error),

    /// A file on a rekordbox medium could not be read.
    #[error(transparent)]
    Medium(#[from] prolink_rekordbox::Error),

    /// Every device number a peer would browse is taken.
    ///
    /// Serving is impossible in this state. The right response is to degrade to
    /// an observer number with serving switched off, which is why this is an
    /// error the caller has to handle rather than a silent fallback (F45).
    #[error("device numbers 1-4 are all in use ({} taken); nothing browsable is free", .taken.len())]
    NoBrowsableNumber {
        /// The numbers observed in use.
        taken: Vec<u8>,
    },

    /// Another device defended the number we were claiming, and we ran out of
    /// candidates.
    #[error("device {number} is held by {holder}, and no candidate remains")]
    NumberConflict {
        /// The number we wanted.
        number: DeviceNumber,
        /// Who defended it.
        holder: Ipv4Addr,
    },

    /// A port serving requires could not be bound.
    ///
    /// Only UDP/111 is in this position. With nothing there a deck retries
    /// `GETPORT` once a second indefinitely, never falls back to the well-known
    /// ports, and so never lists us at all (F46) — so this is a hard failure
    /// rather than a warning. It carries its own variant because the remedy is
    /// platform-specific and belongs in front of a user rather than in a log.
    #[error("cannot bind UDP {port}, which serving files requires: {source} — {remedy}")]
    PrivilegedPort {
        /// The port that could not be bound.
        port: u16,
        /// What the operating system said.
        #[source]
        source: std::io::Error,
        /// What a user can do about it on this platform.
        remedy: &'static str,
    },

    /// Asking a peer about its slots needs UDP 50002, and this device does not
    /// hold it.
    ///
    /// A media response is sent to the port it was asked from, and only one
    /// socket in a `SO_REUSEPORT` group receives a given unicast datagram — so
    /// a virtual CDJ started with `emit_status: false`, which is how a
    /// [`crate::Monitor`] is given the port instead, cannot ask. Two things
    /// wanting one port is a configuration mistake rather than a failure to
    /// retry, which is why it is not a timeout.
    #[error(
        "this virtual CDJ does not hold UDP {port}, which a media query needs; it was started \
         with status emission off"
    )]
    NoStatusPort {
        /// The port a media query needs.
        port: u16,
    },

    /// A peer stopped answering.
    #[error("{what} timed out after {}ms", .after.as_millis())]
    Timeout {
        /// What we were waiting for.
        what: &'static str,
        /// How long we waited.
        after: std::time::Duration,
    },

    /// A peer answered a file-access call with a filesystem status.
    ///
    /// A well-formed reply reporting a failure, not a decoding failure, and the
    /// status is the whole of what a caller can act on:
    /// [`ACCES`](prolink_proto::rpc::nfs2::ErrorStatus::ACCES) on `MNT`
    /// means "announce first" rather than "give up" (F12),
    /// [`NOENT`](prolink_proto::rpc::nfs2::ErrorStatus::NOENT) on a `LOOKUP` may
    /// mean the medium spells the name differently from its own database (O6),
    /// and [`STALE`](prolink_proto::rpc::nfs2::ErrorStatus::STALE) means the mount
    /// has to be redone.
    #[error("{operation} {path}: {status}")]
    Nfs {
        /// Which procedure failed.
        operation: &'static str,
        /// The path or export it was called on.
        path: String,
        /// What the peer reported.
        status: prolink_proto::rpc::nfs2::ErrorStatus,
    },

    /// A peer understood a request and would not answer it.
    ///
    /// Distinct from [`Error::Protocol`], which means the bytes did not decode
    /// at all: this one is a peer that is speaking the protocol and saying no.
    #[error("{what}: {detail}")]
    Refused {
        /// What was being attempted.
        what: &'static str,
        /// What the peer said, in whatever terms its layer has.
        detail: String,
    },
}

impl Error {
    /// Whether a peer said its filehandles no longer refer to anything.
    ///
    /// The one NFS status with a defined remedy: re-`MNT` and walk again,
    /// once (F28). Worth asking here rather than making every caller match on
    /// the variant, because getting it wrong means either an unrecoverable
    /// transfer or a loop against a genuinely missing file.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        matches!(
            self,
            Self::Nfs {
                status: prolink_proto::rpc::nfs2::ErrorStatus::STALE,
                ..
            }
        )
    }

    pub(crate) fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> Self {
        move |source| Self::Io { context, source }
    }

    pub(crate) fn interfaces(error: impl std::fmt::Display) -> Self {
        Self::Interfaces(error.to_string())
    }
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
