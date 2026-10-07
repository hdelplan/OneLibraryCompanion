// SPDX-License-Identifier: GPL-3.0-only

//! Finding the rekordbox sticks plugged into *this* machine.
//!
//! A host that wants to offer its own media to the players on the network has
//! to know what it has, and there is no event for that worth waiting on — so
//! this is a scan, cheap enough to run every couple of seconds forever.
//!
//! # Why the label is not the mount point
//!
//! On macOS the mount point *is* the label, so the two agree. On Linux they do
//! not: an automounter names the directory whatever it likes — a Raspberry Pi
//! set up for this puts sticks at `/media/DJ_USB_1` — while the deck shows the
//! filesystem's own label. A DJ reading `DJ_USB_1` off one screen and `NHK_2024`
//! off the other has no way to tell they are the same stick.
//!
//! So the label is resolved properly: mount point → device, through
//! `/proc/self/mounts`, then device → label, through the `/dev/disk/by-label`
//! symlinks. The mount point's own name is the fallback, which is correct on
//! macOS and merely unhelpful elsewhere.

use std::path::{Path, PathBuf};

/// Where `export.pdb` lives on a rekordbox medium, relative to its root.
///
/// The presence of this file *is* the definition of a rekordbox stick: a deck
/// looks for nothing else before it will browse one.
pub const PDB_PATH: &str = "PIONEER/rekordbox/export.pdb";

/// A rekordbox medium mounted on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// Where it is mounted — the directory holding `PIONEER/`.
    pub path: PathBuf,
    /// What the DJ's deck calls it, resolved as described above.
    pub label: String,
}

/// Every rekordbox medium currently mounted on this machine.
///
/// Ordered by path, so two scans of an unchanged machine agree — a caller
/// assigning slots by position needs that or a stick would move between slots
/// for no reason.
#[must_use]
pub fn rekordbox_volumes() -> Vec<Volume> {
    let mut found: Vec<Volume> = candidates()
        .into_iter()
        .filter(|path| path.join(PDB_PATH).is_file())
        .map(|path| Volume {
            label: label_of(&path),
            path,
        })
        .collect();
    found.sort_by(|left, right| left.path.cmp(&right.path));
    found.dedup_by(|left, right| left.path == right.path);
    found
}

/// The directories a removable medium is mounted under, one level deep.
///
/// **Immediate children only.** An automounter puts a stick at
/// `/media/DJ_USB_1` or `/media/<user>/NHK`, never deeper, and recursing would
/// walk the contents of every stick looking for another stick.
fn candidates() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/media"), PathBuf::from("/Volumes")];
    if let Ok(user) = std::env::var("USER")
        && !user.is_empty()
    {
        roots.push(PathBuf::from("/media").join(&user));
        roots.push(PathBuf::from("/run/media").join(&user));
    }

    let mut volumes = Vec::new();
    for root in roots {
        let Ok(listing) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in listing.flatten() {
            if entry.path().is_dir() {
                volumes.push(entry.path());
            }
        }
    }
    volumes
}

/// The filesystem label of whatever is mounted at `mount_point`.
fn label_of(mount_point: &Path) -> String {
    let fallback = mount_point
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let Some(device) = device_at(mount_point) else {
        return fallback;
    };
    let Ok(listing) = std::fs::read_dir("/dev/disk/by-label") else {
        return fallback;
    };
    for entry in listing.flatten() {
        // These are symlinks to block devices, and the target is relative
        // (`../../sda1`); canonicalising resolves it into the absolute form the
        // mount table gives.
        if entry
            .path()
            .canonicalize()
            .is_ok_and(|target| target == device)
        {
            // udev escapes a label the same way the mount table escapes a path.
            return unescape(&entry.file_name().to_string_lossy());
        }
    }
    fallback
}

/// The block device mounted at a path, from the kernel's mount table.
fn device_at(mount_point: &Path) -> Option<PathBuf> {
    let table = std::fs::read_to_string("/proc/self/mounts").ok()?;
    for line in table.lines() {
        let mut fields = line.split(' ');
        let device = fields.next()?;
        let Some(at) = fields.next() else { continue };
        if Path::new(&unescape(at)) == mount_point {
            return Path::new(device).canonicalize().ok();
        }
    }
    None
}

/// Decode the octal escapes the mount table uses for spaces and the like.
///
/// A stick called `MY SET` appears as `MY\040SET`, and comparing the escaped
/// form against a real path never matches — silently, and the symptom is a
/// stick that is found but shows the wrong name.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'\\')
            && let Some(digits) = text.get(index + 1..index + 4)
            && let Ok(value) = u8::from_str_radix(digits, 8)
        {
            out.push(char::from(value));
            index += 4;
            continue;
        }
        if let Some(rest) = text.get(index..)
            && let Some(character) = rest.chars().next()
        {
            out.push(character);
            index += character.len_utf8();
        } else {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_escapes_are_decoded() {
        assert_eq!(unescape("/media/MY\\040SET"), "/media/MY SET");
        assert_eq!(unescape("plain"), "plain");
        // A backslash that starts nothing is kept, rather than eaten.
        assert_eq!(unescape("a\\b"), "a\\b");
        assert_eq!(unescape("trailing\\"), "trailing\\");
    }

    #[test]
    fn a_label_falls_back_to_the_mount_point_name() {
        // Nothing is mounted here, so neither lookup can succeed and the
        // fallback is all there is. On a machine where /proc does not exist at
        // all this is also the only answer, which is why it is not an error.
        assert_eq!(label_of(Path::new("/nonexistent/NHK_2024")), "NHK_2024");
    }

    #[test]
    fn scanning_a_machine_with_no_sticks_finds_none_rather_than_failing() {
        // Runs against the real machine: the assertion is on the invariant, not
        // on what happens to be plugged in.
        for volume in rekordbox_volumes() {
            assert!(
                volume.path.join(PDB_PATH).is_file(),
                "{} was returned without an export.pdb",
                volume.path.display()
            );
        }
    }
}
