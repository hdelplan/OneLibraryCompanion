// SPDX-License-Identifier: GPL-3.0-only

//! The one error type this crate returns.

/// What went wrong reading a capture.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The leading bytes are neither a pcap magic nor a pcapng section header.
    ///
    /// Whatever the file is — a text log, a JSONL journal, a truncated
    /// download — it is not a capture, and no amount of further reading will
    /// make it one.
    #[error("not a capture: leading bytes {magic:02x?} are no pcap or pcapng magic")]
    NotACapture {
        /// The four bytes that were there instead.
        magic: [u8; 4],
    },

    /// The file ends part-way through a record.
    ///
    /// Kept apart from [`Error::Malformed`] because it is the *expected* state
    /// of a capture whose `tcpdump` was killed rather than stopped, which any
    /// real corpus contains. Everything yielded before it is good; only the
    /// tail is missing. See [`Error::is_truncated`].
    #[error("the capture ends part-way through a record")]
    Truncated,

    /// A record's fields contradict the format or each other. More bytes will
    /// not help.
    #[error("malformed capture: {reason}")]
    Malformed {
        /// What the underlying reader objected to.
        reason: String,
    },

    /// Reading the file failed.
    #[error("i/o error reading the capture")]
    Io(#[from] std::io::Error),

    /// Frames recorded from a link layer this crate does not dissect.
    ///
    /// Raised rather than silently skipped: a capture taken on a `pktap` or
    /// `usbmon` interface would otherwise yield zero packets and look exactly
    /// like a capture with no Pro DJ Link traffic in it.
    #[error("link type {link_type} is not dissected; this crate handles Ethernet (1) only")]
    UnsupportedLinkType {
        /// The [tcpdump.org] link-layer header type code.
        ///
        /// [tcpdump.org]: https://www.tcpdump.org/linktypes.html
        link_type: u32,
    },
}

impl Error {
    /// True when the file was cut short rather than being the wrong thing.
    ///
    /// The distinction a corpus walker needs: a truncated capture is worth
    /// keeping the packets from, a malformed one is worth a warning.
    pub fn is_truncated(&self) -> bool {
        matches!(self, Self::Truncated)
    }
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
