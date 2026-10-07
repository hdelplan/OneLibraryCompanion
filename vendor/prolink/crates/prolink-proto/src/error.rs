// SPDX-License-Identifier: GPL-3.0-only

//! The one error type every codec in this crate returns.

use std::fmt;

/// What went wrong decoding or encoding a Pro DJ Link message.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The datagram does not carry the Pro DJ Link magic, or a message does not
    /// carry the magic its format requires.
    #[error("bad magic: expected {expected}, got {got}")]
    BadMagic {
        /// What the format requires.
        expected: HexBytes,
        /// What the bytes held.
        got: HexBytes,
    },

    /// The buffer ends before a field this format requires.
    ///
    /// For a stream protocol this is the *expected* outcome of trying to decode
    /// too early, so callers distinguish it from [`Error::Malformed`] to decide
    /// between "wait for more bytes" and "drop the connection".
    #[error("truncated: need {need} bytes at offset {at}, {have} available")]
    Truncated {
        /// How many bytes the field needs.
        need: usize,
        /// Where in the buffer it starts.
        at: usize,
        /// How many bytes remain from there.
        have: usize,
    },

    /// A field holds a value the format does not allow. Unlike
    /// [`Error::Truncated`], more bytes will not help.
    #[error("malformed at offset {at}: {reason}")]
    Malformed {
        /// Where the offending field starts.
        at: usize,
        /// What is wrong with it.
        reason: String,
    },

    /// A length prefix claims more than any real message carries. Rejected
    /// before allocating, so a corrupt or hostile word costs a parse failure
    /// rather than four gigabytes.
    #[error("implausible length {length} for {what} (limit {limit})")]
    ImplausibleLength {
        /// What was being read.
        what: &'static str,
        /// The length the bytes claimed.
        length: u64,
        /// The ceiling this codec enforces.
        limit: u64,
    },

    /// A `binrw`-derived codec failed.
    #[error(transparent)]
    Binrw(#[from] binrw::Error),
}

impl Error {
    /// True when more bytes could make this parse succeed.
    ///
    /// The distinction that matters on a TCP stream: a dbserver message is
    /// framed by nothing but its own contents, so running off the end of the
    /// buffer means "wait", and anything else means "this connection is not
    /// speaking the protocol".
    pub fn is_truncated(&self) -> bool {
        match self {
            Self::Truncated { .. } => true,
            Self::Binrw(inner) => inner.is_eof(),
            _ => false,
        }
    }

    pub(crate) fn malformed(at: usize, reason: impl Into<String>) -> Self {
        Self::Malformed {
            at,
            reason: reason.into(),
        }
    }
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A short byte string rendered as hex in error messages.
#[derive(Clone, PartialEq, Eq)]
pub struct HexBytes(pub Vec<u8>);

impl fmt::Debug for HexBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for HexBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl From<&[u8]> for HexBytes {
    fn from(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}
