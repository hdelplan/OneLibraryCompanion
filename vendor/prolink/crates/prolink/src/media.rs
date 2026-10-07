// SPDX-License-Identifier: GPL-3.0-only

//! What a virtual CDJ says is in its slots.
//!
//! This is the seam between announcing and serving. The virtual CDJ has to
//! answer three questions about each slot — is anything in it, what is it
//! called and how much does it hold, and what settings does it carry — without
//! knowing anything about rekordbox, filesystems or databases. So it asks a
//! [`MediaSource`], and the serve side implements one.
//!
//! # Why the counts must be true
//!
//! **A deck asks what a slot actually contains and will not offer a medium it
//! believes is empty** (F24). Announcing and emitting status are not enough:
//! the deck sends a media query, once per slot, when it first browses that
//! slot, and a reply saying zero tracks means it has no reason ever to ask
//! again. This is the step no reference implementation performs, because none
//! of them serve.
//!
//! # A slot is more than occupied or not
//!
//! Media presence is published at `0x6f` and `0x73` of every status packet
//! (F20), and hardware uses four values there rather than two. An eject is a
//! sequence — loaded, [`MediaState::UNMOUNTING`], [`MediaState::UNMOUNTING_ALT`],
//! empty — and it is the third of those that a consuming deck reacts to by
//! releasing its NFS mount. So a source reports a *state* rather than a
//! boolean; the boolean would leave no way to say the medium is going.

use std::collections::BTreeSet;

use prolink_proto::Slot;
use prolink_proto::status::MediaState;

/// What a peer is told about one of our slots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaDescription {
    /// The volume label, shown on the deck.
    ///
    /// **Legitimately empty**: an unlabelled stick reports no name while
    /// carrying a full library, so emptiness here is not emptiness of the slot.
    pub volume_name: String,
    /// The medium's creation date, as `YYYY-MM-DD`.
    ///
    /// Carried by both replies that describe a medium — the UDP `0x06` and the
    /// dbserver `0x4902` — so they are filled from one place and cannot
    /// disagree. Empty when unknown, which no deck has been observed to mind.
    pub created: String,
    /// How many tracks the medium holds. Must be the true count.
    pub track_count: u32,
    /// How many playlists the medium holds. Must be the true count.
    pub playlist_count: u32,
    /// Capacity in bytes, if known. NFSv2 is a 32-bit protocol, so this is too.
    pub total_bytes: Option<u32>,
    /// Free space in bytes, if known.
    pub free_bytes: Option<u32>,
}

/// The media a virtual CDJ presents.
///
/// Implemented by the serve side. A device with nothing to serve uses
/// [`NoMedia`], which reports every slot empty — the correct answer for a
/// consumer that announces only so that peers will talk to it.
pub trait MediaSource: Send + Sync + std::fmt::Debug {
    /// Which slots currently hold something.
    ///
    /// A slot not listed here is reported empty in every status packet, and a
    /// slot reported empty is a slot no player will ever ask about.
    fn occupied_slots(&self) -> BTreeSet<Slot>;

    /// Describe one slot, or `None` if it holds nothing.
    ///
    /// Answering `None` for a slot a deck asked about is right: an empty reply
    /// would tell the deck the slot exists and holds no tracks, and it would
    /// then offer an empty medium.
    fn describe(&self, slot: Slot) -> Option<MediaDescription>;

    /// What to publish at `0x6f`/`0x73` for a slot.
    ///
    /// The default is the two-valued reading of [`Self::occupied_slots`], which
    /// is all a source that never ejects anything needs. A source that does —
    /// the serve side, on its way out — overrides this to walk a medium through
    /// the unmounting states, because a consumer releases its mount on
    /// [`MediaState::UNMOUNTING_ALT`] and never sees that state if the slot
    /// jumps straight from loaded to empty.
    fn slot_state(&self, slot: Slot) -> MediaState {
        if self.occupied_slots().contains(&slot) {
            MediaState::LOADED
        } else {
            MediaState::EMPTY
        }
    }

    /// The 32 bytes from this medium's `PIONEER/MYSETTING.DAT`, if it has any.
    ///
    /// An empty block is a legitimate answer — a medium with no saved settings
    /// — so a missing file is not an error. The default returns nothing, since
    /// only a serving device needs to answer this at all.
    fn settings(&self, slot: Slot) -> Vec<u8> {
        let _ = slot;
        Vec::new()
    }
}

/// A [`MediaSource`] with empty slots.
///
/// What a pure consumer announces with: it needs a device number and a status
/// stream so that peers will unicast to it, but it has nothing to offer.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoMedia;

impl MediaSource for NoMedia {
    fn occupied_slots(&self) -> BTreeSet<Slot> {
        BTreeSet::new()
    }

    fn describe(&self, _slot: Slot) -> Option<MediaDescription> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_consumer_reports_every_slot_empty() {
        assert!(NoMedia.occupied_slots().is_empty());
        assert_eq!(NoMedia.describe(Slot::USB), None);
        assert!(NoMedia.settings(Slot::USB).is_empty());
        assert_eq!(NoMedia.slot_state(Slot::USB), MediaState::EMPTY);
    }

    #[test]
    fn a_source_that_never_ejects_publishes_only_loaded_and_empty() {
        #[derive(Debug)]
        struct UsbOnly;
        impl MediaSource for UsbOnly {
            fn occupied_slots(&self) -> BTreeSet<Slot> {
                [Slot::USB].into_iter().collect()
            }
            fn describe(&self, slot: Slot) -> Option<MediaDescription> {
                (slot == Slot::USB).then(MediaDescription::default)
            }
        }

        assert_eq!(UsbOnly.slot_state(Slot::USB), MediaState::LOADED);
        assert_eq!(UsbOnly.slot_state(Slot::SD), MediaState::EMPTY);
    }
}
