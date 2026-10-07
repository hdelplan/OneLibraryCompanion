// SPDX-License-Identifier: GPL-3.0-only

//! Finding the capture corpus, or finding that there is none.
//!
//! The corpus is ~272 MB of recordings of real Pioneer hardware, committed to
//! this repository under `captures/` so that the evidence behind every codec
//! ships with the code. Tests reach it through the `PROLINK_CAPTURES`
//! environment variable, falling back to that directory, and **skip cleanly
//! when it is absent** — a shallow clone or a vendored copy of the sources may
//! not carry it, which is why this returns `Option<Corpus>` rather than a path
//! that may or may not exist.
//!
//! Skipping is not the same as passing. Every test file that consumes the
//! corpus also carries a committed fixture floor, so a coverage regression
//! cannot hide behind an empty corpus on a machine that has no captures.

use std::path::{Path, PathBuf};

/// Environment variable naming the corpus directory.
pub const CORPUS_ENV: &str = "PROLINK_CAPTURES";

/// Where the corpus lives if the variable is unset, relative to the workspace.
const DEFAULT_RELATIVE: &str = "captures";

/// A directory of capture files that exists.
///
/// Constructing one proves the directory is there, so nothing downstream has
/// to re-check or decide what to do when it is not: that decision is made once,
/// by whoever called [`Corpus::locate`] and got `None`.
#[derive(Clone, Debug)]
pub struct Corpus {
    root: PathBuf,
}

impl Corpus {
    /// Locate the corpus, or `None` on a machine that has none.
    ///
    /// `PROLINK_CAPTURES` wins when it is set; an explicitly-set variable
    /// pointing somewhere that does not exist yields `None` rather than
    /// silently falling back, because the caller meant that directory.
    pub fn locate() -> Option<Self> {
        if let Some(path) = std::env::var_os(CORPUS_ENV) {
            let root = PathBuf::from(path);
            return root.is_dir().then_some(Self { root });
        }
        let root = workspace_root()?.join(DEFAULT_RELATIVE);
        root.is_dir().then_some(Self { root })
    }

    /// Use a specific directory, if it is one.
    pub fn at(root: impl Into<PathBuf>) -> Option<Self> {
        let root = root.into();
        root.is_dir().then_some(Self { root })
    }

    /// The directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every capture file under it, sorted, so a run is reproducible.
    ///
    /// Selected by extension — `pcap` or `pcapng` — which says nothing about
    /// the format inside: every file in this project's corpus is named
    /// `run.pcap` and eleven of the thirty-seven are pcapng.
    /// [`crate::Capture`] dispatches on the magic, not on the name.
    pub fn captures(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        collect(&self.root, &mut found);
        found.sort();
        found
    }
}

fn collect(directory: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, into);
        } else if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("pcap" | "pcapng")
        ) {
            into.push(path);
        }
    }
}

/// The workspace root, derived from where this crate was compiled.
fn workspace_root() -> Option<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_that_is_not_there_is_not_a_corpus() {
        assert!(Corpus::at("/nonexistent/prolink/captures").is_none());
    }

    #[test]
    fn the_workspace_root_is_two_levels_above_this_crate() {
        let root = workspace_root().expect("a workspace root");
        assert!(
            root.join("Cargo.toml").is_file(),
            "expected the workspace manifest at {root:?}"
        );
        assert!(root.join("crates/prolink-capture").is_dir());
    }
}
