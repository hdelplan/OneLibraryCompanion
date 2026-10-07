// SPDX-License-Identifier: GPL-3.0-only

//! Holding on to a track a player has loaded, before the stick can go.
//!
//! # Why this exists at all
//!
//! **A real CDJ does not buffer the whole track.** It holds an emergency loop
//! of what is playing right now and streams the rest off the medium. So a stick
//! pulled while a player is playing from us is a player that stops several
//! seconds later, mid-set, with no warning and nothing anyone can do about it.
//!
//! The fix is to have a copy before that happens.
//!
//! # When, and why not as it is served
//!
//! Caching bytes *as they are delivered* keeps exactly the part the player no
//! longer needs — it has already played it — and none of the part it is about
//! to ask for. So the copy is made **whole, when a player is first seen to have
//! loaded the track**. A load is announced in the player's own status packets,
//! which name the source player, the slot and the track id, so this needs no
//! hook in the read path and cannot miss a load that was served from a cache.
//!
//! It is our own stick, plugged into this machine, and the copy is a local file
//! read: a few seconds for a lossless track, and it happens while the player is
//! still on the first bars.
//!
//! # Where the copies go
//!
//! Wherever the host says — on the deck that is tmpfs, so this costs no writes
//! to the SD card and evaporates at reboot, which is right for a copy of
//! somebody's stick. Bounded, because a peer that loads track after track would
//! otherwise fill it; past the cap nothing new is preserved and the log says
//! so, rather than the machine being taken down by a full filesystem.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

/// Copies of files a player is using, keyed by their path in the served tree.
#[derive(Debug)]
pub struct Preserve {
    root: PathBuf,
    /// Served path → where its copy is. What [`Vfs::preserve`] is handed.
    ///
    /// [`Vfs::preserve`]: crate::serve::vfs::Vfs::preserve
    copies: BTreeMap<String, PathBuf>,
    /// Paths already attempted, successfully or not. A second attempt at a file
    /// that could not be read would be made on every poll for as long as a
    /// player held it.
    attempted: std::collections::BTreeSet<String>,
    bytes: u64,
    cap: u64,
    /// Said once. A cap reached during a set would otherwise print a line every
    /// two seconds for the rest of the night.
    warned: bool,
}

impl Preserve {
    /// A store under `root`, holding at most `cap` bytes.
    pub fn new(root: PathBuf, cap: u64) -> Self {
        Self {
            root,
            copies: BTreeMap::new(),
            attempted: std::collections::BTreeSet::new(),
            bytes: 0,
            cap,
            warned: false,
        }
    }

    /// What has been preserved, for [`Vfs::preserve`].
    ///
    /// [`Vfs::preserve`]: crate::serve::vfs::Vfs::preserve
    pub fn copies(&self) -> &BTreeMap<String, PathBuf> {
        &self.copies
    }

    /// How much has been copied.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// How many files are held.
    pub fn len(&self) -> usize {
        self.copies.len()
    }

    /// Whether nothing has been preserved.
    pub fn is_empty(&self) -> bool {
        self.copies.is_empty()
    }

    /// Copy one file, if it has not been copied already.
    ///
    /// `served` is its path in the tree — the key a later swap looks it up by —
    /// and `disk` is where to read it from now, while the medium is still here.
    ///
    /// Failure is recorded as an attempt rather than retried: a file that
    /// cannot be read now will not read differently in two seconds, and the
    /// alternative is trying again on every poll for as long as a player holds
    /// it.
    pub fn keep(&mut self, served: &str, disk: &Path) {
        if self.attempted.contains(served) {
            return;
        }
        self.attempted.insert(served.to_owned());

        let Ok(metadata) = std::fs::metadata(disk) else {
            warn!(file = %disk.display(), "cannot preserve a file that is not there");
            return;
        };
        let size = metadata.len();
        if self.bytes.saturating_add(size) > self.cap {
            if !self.warned {
                self.warned = true;
                warn!(
                    held = self.bytes,
                    cap = self.cap,
                    "the preserve cache is full; further loads will not survive an eject"
                );
            }
            return;
        }

        // Flattened, and hashed rather than mirrored: a served path can be any
        // depth and hold anything a DJ typed, and the tree it belongs to is
        // about to stop existing anyway. Only the mapping matters.
        let target = self.root.join(flat_name(served));
        if let Some(parent) = target.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            warn!(directory = %parent.display(), %error, "cannot make the preserve directory");
            return;
        }
        // Via a partial file: a copy interrupted half way is worse than no copy
        // at all, because a partial one would be swapped in and served as a
        // truncated track.
        let partial = target.with_extension("part");
        let _ = std::fs::remove_file(&partial);
        if let Err(error) = std::fs::copy(disk, &partial) {
            warn!(file = %disk.display(), %error, "could not preserve a file");
            let _ = std::fs::remove_file(&partial);
            return;
        }
        if let Err(error) = std::fs::rename(&partial, &target) {
            warn!(file = %target.display(), %error, "could not finish preserving a file");
            let _ = std::fs::remove_file(&partial);
            return;
        }

        self.bytes = self.bytes.saturating_add(size);
        self.copies.insert(served.to_owned(), target);
        info!(
            served,
            size,
            held = self.copies.len(),
            "preserved a file a player is using"
        );
    }

    /// Forget everything, removing the copies.
    pub fn clear(&mut self) {
        for path in self.copies.values() {
            let _ = std::fs::remove_file(path);
        }
        self.copies.clear();
        self.attempted.clear();
        self.bytes = 0;
        self.warned = false;
    }
}

/// A filename that cannot collide and cannot escape the directory.
///
/// A hash rather than a sanitised version of the path, because the input is
/// somebody else's stick: it can be any depth, hold anything a DJ typed, and be
/// longer than a filesystem allows a name to be — and `..` in it must not be
/// able to write outside this directory. The extension is carried through, so a
/// copy still looks like what it is.
fn flat_name(served: &str) -> String {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(served.as_bytes());
    let mut out = String::with_capacity(48);
    for byte in digest.iter().take(12) {
        // `write!` rather than push_str(&format!(..)): one allocation for the
        // whole name instead of one per byte.
        let _ = write!(out, "{byte:02x}");
    }
    // The extension too, because a decoder somewhere may care what it is.
    if let Some(dot) = served.rfind('.')
        && served.len() - dot <= 6
    {
        let extension: String = served[dot..]
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
            .collect();
        out.push_str(&extension);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own, removed when it drops.
    ///
    /// Hand-rolled rather than pulling in `tempfile`: this crate has one
    /// dev-dependency on purpose, and what is wanted here is a dozen lines.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "prolink-preserve-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_flat_name_is_a_bare_filename() {
        // No separators and no dot-dot: a served path is attacker-adjacent
        // input -- it comes off somebody else's stick -- and this becomes a
        // filename on ours.
        let name = flat_name("/C/Contents/../../etc/passwd");
        assert!(!name.contains('/'));
        assert!(!name.contains(".."));
    }

    #[test]
    fn two_paths_do_not_collide() {
        assert_ne!(flat_name("/C/a/track.mp3"), flat_name("/C/b/track.mp3"));
    }

    #[test]
    fn the_extension_is_kept() {
        assert!(flat_name("/C/Contents/x/track.mp3").contains(".mp3"));
        assert!(flat_name("/C/PIONEER/USBANLZ/x/ANLZ0000.DAT").contains(".DAT"));
    }

    #[test]
    fn a_name_with_no_extension_is_still_a_name() {
        let name = flat_name("/C/Contents/whatever");
        assert!(!name.is_empty());
        assert!(!name.contains('/'));
    }

    #[test]
    fn a_file_is_copied_once() {
        let directory = Scratch::new();
        let source = directory.path().join("track.mp3");
        std::fs::write(&source, b"0123456789").expect("write");

        let mut preserve = Preserve::new(directory.path().join("cache"), 1024);
        preserve.keep("/C/Contents/track.mp3", &source);
        assert_eq!(preserve.len(), 1);
        assert_eq!(preserve.bytes(), 10);

        // Again: no second copy, and the byte count does not double.
        preserve.keep("/C/Contents/track.mp3", &source);
        assert_eq!(preserve.len(), 1);
        assert_eq!(preserve.bytes(), 10);
    }

    #[test]
    fn the_copy_holds_the_whole_file() {
        // Not the part already delivered -- that is precisely the part a player
        // no longer needs.
        let directory = Scratch::new();
        let source = directory.path().join("track.mp3");
        let content: Vec<u8> = (0..=255u8).cycle().take(100_000).collect();
        std::fs::write(&source, &content).expect("write");

        let mut preserve = Preserve::new(directory.path().join("cache"), 1_000_000);
        preserve.keep("/C/Contents/track.mp3", &source);
        let copy = preserve
            .copies()
            .get("/C/Contents/track.mp3")
            .expect("copied");
        assert_eq!(std::fs::read(copy).expect("read back"), content);
    }

    #[test]
    fn a_missing_file_is_not_an_error_and_is_not_retried() {
        let directory = Scratch::new();
        let mut preserve = Preserve::new(directory.path().join("cache"), 1024);
        preserve.keep("/C/Contents/gone.mp3", &directory.path().join("gone.mp3"));
        assert!(preserve.is_empty());
        // Recorded as attempted, so a poll every two seconds does not retry it
        // for the rest of the night.
        assert!(preserve.attempted.contains("/C/Contents/gone.mp3"));
    }

    #[test]
    fn the_cap_is_not_exceeded() {
        let directory = Scratch::new();
        let mut preserve = Preserve::new(directory.path().join("cache"), 25);
        for index in 0..5 {
            let source = directory.path().join(format!("{index}.mp3"));
            std::fs::write(&source, vec![0u8; 10]).expect("write");
            preserve.keep(&format!("/C/Contents/{index}.mp3"), &source);
        }
        // Two fit, the third would not, and nothing after it is taken either.
        assert_eq!(preserve.len(), 2);
        assert!(preserve.bytes() <= 25);
    }

    #[test]
    fn clearing_removes_the_copies() {
        let directory = Scratch::new();
        let source = directory.path().join("track.mp3");
        std::fs::write(&source, b"bytes").expect("write");

        let mut preserve = Preserve::new(directory.path().join("cache"), 1024);
        preserve.keep("/C/Contents/track.mp3", &source);
        let copy = preserve
            .copies()
            .get("/C/Contents/track.mp3")
            .expect("copied")
            .clone();
        assert!(copy.exists());

        preserve.clear();
        assert!(preserve.is_empty());
        assert!(!copy.exists());
        // The original is untouched. It is somebody's stick.
        assert!(source.exists());
    }

    #[test]
    fn a_partial_copy_is_never_left_behind_as_the_real_one() {
        // The `.part` name is what makes this true; if a copy dies half way the
        // target never appears, so nothing can serve a truncated track.
        let directory = Scratch::new();
        let source = directory.path().join("track.mp3");
        std::fs::write(&source, b"whole").expect("write");

        let mut preserve = Preserve::new(directory.path().join("cache"), 1024);
        preserve.keep("/C/Contents/track.mp3", &source);
        let copy = preserve
            .copies()
            .get("/C/Contents/track.mp3")
            .expect("copied");
        assert_eq!(std::fs::read(copy).expect("read"), b"whole");
        assert!(!copy.with_extension("part").exists());
    }
}
