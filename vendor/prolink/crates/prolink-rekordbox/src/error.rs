// SPDX-License-Identifier: GPL-3.0-only

//! The one error type every reader in this crate returns.

use std::fmt;

/// What went wrong reading a file off a rekordbox medium.
///
/// The distinction that matters is [`Error::is_truncated`]. These files arrive
/// over NFS from a device we do not control, so a short read and a corrupt one
/// are genuinely different: the first says "the transfer is not finished", the
/// second says "this is not a rekordbox export".
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The buffer ends before a field the format requires.
    #[error("truncated: need {need} bytes at offset {at}, {have} available")]
    Truncated {
        /// How many bytes the field needs.
        need: u64,
        /// Where in the buffer it starts.
        at: u64,
        /// How many bytes remain from there.
        have: u64,
    },

    /// A magic value did not match. More bytes will not help.
    #[error("bad magic at offset {at}: expected {expected}, got {got}")]
    BadMagic {
        /// Where the magic starts.
        at: u64,
        /// What the format requires.
        expected: HexBytes,
        /// What the bytes held.
        got: HexBytes,
    },

    /// A field holds a value the format does not allow.
    ///
    /// Deliberately also covers "this could be read as an empty database": a
    /// buffer of zeroes parses as a pdb with a valid-looking header and no
    /// tables, and reporting that as an empty medium is the worst outcome for a
    /// file that arrived over a network — a truncated download would look
    /// exactly like a stick with nothing on it.
    #[error("malformed at offset {at}: {reason}")]
    Malformed {
        /// Where the offending field starts.
        at: u64,
        /// What is wrong with it.
        reason: String,
    },

    /// A `binrw`-derived reader failed.
    #[error(transparent)]
    Binrw(#[from] binrw::Error),
}

impl Error {
    /// True when more bytes could make this parse succeed.
    pub fn is_truncated(&self) -> bool {
        match self {
            Self::Truncated { .. } => true,
            Self::Binrw(inner) => inner.is_eof(),
            _ => false,
        }
    }

    pub(crate) fn malformed(at: u64, reason: impl Into<String>) -> Self {
        Self::Malformed {
            at,
            reason: reason.into(),
        }
    }

    pub(crate) fn truncated(at: u64, need: u64, have: u64) -> Self {
        Self::Truncated { need, at, have }
    }

    pub(crate) fn bad_magic(at: u64, expected: &[u8], got: &[u8]) -> Self {
        Self::BadMagic {
            at,
            expected: HexBytes(expected.to_vec()),
            got: HexBytes(got.to_vec()),
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
