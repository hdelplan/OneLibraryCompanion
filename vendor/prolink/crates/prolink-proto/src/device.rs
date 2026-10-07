// SPDX-License-Identifier: GPL-3.0-only

//! Device identity: numbers, names, hardware addresses and kinds.
//!
//! These types are shared by every layer of the protocol, and two of them exist
//! to make a documented failure impossible to write rather than merely
//! documented.

use std::fmt;
use std::num::NonZeroU8;

use binrw::binrw;

/// A device number a peer can hold.
///
/// Non-zero by construction: `0` on the wire means "no device", and a peer
/// table that accepted it would grow an entry no request can ever be addressed
/// to.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceNumber(NonZeroU8);

impl DeviceNumber {
    /// The lowest number a real player can be set to.
    pub const MIN_PLAYER: u8 = 1;
    /// The highest number a real player can be set to.
    pub const MAX_PLAYER: u8 = 6;
    /// The highest number a peer will offer as a browsable source (F45).
    pub const MAX_BROWSABLE: u8 = 4;

    /// Device number 1, the lowest a player can hold.
    ///
    /// Exists so that a caller needing a total fallback has one that is not a
    /// panic.
    pub const ONE: Self = Self(NonZeroU8::MIN);

    /// Parse a device number, rejecting zero.
    pub const fn new(raw: u8) -> Option<Self> {
        match NonZeroU8::new(raw) {
            Some(number) => Some(Self(number)),
            None => None,
        }
    }

    /// The number as it goes on the wire.
    pub const fn get(self) -> u8 {
        self.0.get()
    }

    /// Whether this is a number a real CDJ or mixer can be set to (1–6).
    ///
    /// Numbers above this range are what observer tools take, and they are
    /// safe: they can never collide with hardware.
    pub const fn is_player(self) -> bool {
        self.get() >= Self::MIN_PLAYER && self.get() <= Self::MAX_PLAYER
    }

    /// This number as a [`BrowsableDeviceNumber`], or `None` if it is too high.
    pub const fn browsable(self) -> Option<BrowsableDeviceNumber> {
        if self.get() <= Self::MAX_BROWSABLE {
            Some(BrowsableDeviceNumber(self))
        } else {
            None
        }
    }
}

impl fmt::Debug for DeviceNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "device {}", self.get())
    }
}

impl fmt::Display for DeviceNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.get())
    }
}

/// A device number in 1–4: the range a peer will actually browse.
///
/// **This is a requirement, not a preference** (F45). At device 5 a CDJ accepts
/// an announcement completely — it puts the announcer in its device table and
/// unicasts hundreds of status packets to it — and then never sends a single
/// media query, so it never offers the announcer as a source. The check
/// precedes the whole browse path and the failure is entirely silent: every
/// later step still works and none of them is ever reached.
///
/// Having a distinct type means a server that can never be browsed cannot be
/// configured. Where 1–4 are all taken, the answer is to degrade to an observer
/// number with serving switched off, and the type system makes that an explicit
/// decision rather than an accident.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrowsableDeviceNumber(DeviceNumber);

impl BrowsableDeviceNumber {
    /// Parse a browsable device number, rejecting zero and anything above 4.
    pub const fn new(raw: u8) -> Option<Self> {
        match DeviceNumber::new(raw) {
            Some(number) => number.browsable(),
            None => None,
        }
    }

    /// The number as it goes on the wire.
    pub const fn get(self) -> u8 {
        self.0.get()
    }

    /// Widen to a plain device number.
    pub const fn number(self) -> DeviceNumber {
        self.0
    }

    /// Every browsable number, highest first.
    ///
    /// Highest first because 1 and 2 are the decks a DJ reaches for, so a
    /// virtual CDJ should leave them alone as long as it can.
    pub fn descending() -> impl Iterator<Item = Self> {
        (DeviceNumber::MIN_PLAYER..=DeviceNumber::MAX_BROWSABLE)
            .rev()
            .filter_map(Self::new)
    }
}

impl From<BrowsableDeviceNumber> for DeviceNumber {
    fn from(number: BrowsableDeviceNumber) -> Self {
        number.0
    }
}

impl fmt::Debug for BrowsableDeviceNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for BrowsableDeviceNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The device model byte. Critical for impersonation.
#[binrw]
#[brw(big)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceKind(pub u8);

impl DeviceKind {
    /// A DJM mixer.
    pub const MIXER: Self = Self(0x01);
    /// A CDJ player.
    pub const CDJ: Self = Self(0x02);
    /// rekordbox desktop, and also the CDJ-3000.
    pub const REKORDBOX_OR_CDJ3000: Self = Self(0x03);
    /// Seen only in the CDJ-3000 hello.
    pub const CDJ3000_HELLO: Self = Self(0x04);

    /// The `0x01` CDJ / `0x02` mixer role byte that recurs across packet kinds.
    ///
    /// Appears at offset `0x30` of the stage-2 claim and at `0x34` of the
    /// keep-alive, and tracks the device kind in both. The pre-hardware
    /// literature documents the first as a constant `0x01`; a real
    /// DJM-2000nexus sends `0x02` (C1).
    pub fn role(self) -> u8 {
        if self == Self::MIXER { 0x02 } else { 0x01 }
    }
}

impl fmt::Debug for DeviceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::MIXER => f.write_str("mixer"),
            Self::CDJ => f.write_str("cdj"),
            Self::REKORDBOX_OR_CDJ3000 => f.write_str("rekordbox_or_cdj3000"),
            Self::CDJ3000_HELLO => f.write_str("cdj3000_hello"),
            Self(raw) => write!(f, "DeviceKind({raw:#04x})"),
        }
    }
}

/// A 6-byte hardware address.
#[binrw]
#[brw(big)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MacAddress(pub [u8; 6]);

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [o0, o1, o2, o3, o4, o5] = self.0;
        write!(f, "{o0:02x}:{o1:02x}:{o2:02x}:{o3:02x}:{o4:02x}:{o5:02x}")
    }
}

impl fmt::Debug for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// A 20-byte NUL-padded device name.
///
/// Kept as the literal bytes rather than a `String` because the padding is part
/// of what makes an announcement indistinguishable from a real one, and because
/// `CDJ-2000nexus` is the exact casing observed on hardware (F1) — a decoded
/// string cannot pin that.
///
/// The field sits at `0x0c` on UDP 50000 and at `0x0b` on UDP 50002. The type
/// is shared; the offset is not.
#[binrw]
#[brw(big)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceName(pub [u8; Self::LEN]);

impl DeviceName {
    /// Bytes the field occupies on both ports.
    pub const LEN: usize = 20;

    /// The name a CDJ-2000nexus announces, in its exact casing (F1).
    pub const CDJ_2000_NEXUS: &'static str = "CDJ-2000nexus";

    /// Build a padded name, truncating anything past [`Self::LEN`] bytes.
    pub fn new(name: &str) -> Self {
        let mut raw = [0u8; Self::LEN];
        for (slot, byte) in raw.iter_mut().zip(name.bytes()) {
            *slot = byte;
        }
        Self(raw)
    }

    /// The name up to its first NUL, with non-ASCII bytes replaced.
    pub fn as_str(&self) -> String {
        self.0
            .iter()
            .copied()
            .take_while(|&byte| byte != 0)
            .map(|byte| {
                if byte.is_ascii() {
                    char::from(byte)
                } else {
                    char::REPLACEMENT_CHARACTER
                }
            })
            .collect()
    }
}

impl Default for DeviceName {
    fn default() -> Self {
        Self::new(Self::CDJ_2000_NEXUS)
    }
}

impl fmt::Display for DeviceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

impl fmt::Debug for DeviceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_not_a_device_number() {
        assert!(DeviceNumber::new(0).is_none());
        assert!(BrowsableDeviceNumber::new(0).is_none());
    }

    #[test]
    fn five_announces_fine_and_is_never_browsed() {
        let five = DeviceNumber::new(5).expect("5 is a device number");
        assert!(five.is_player(), "5 is a number a real player can hold");
        assert!(
            five.browsable().is_none(),
            "but a peer will never browse it (F45)"
        );
    }

    #[test]
    fn browsable_numbers_come_out_highest_first() {
        let numbers: Vec<u8> = BrowsableDeviceNumber::descending()
            .map(BrowsableDeviceNumber::get)
            .collect();
        assert_eq!(
            numbers,
            vec![4, 3, 2, 1],
            "leave 1 and 2 alone as long as possible"
        );
    }

    #[test]
    fn a_name_keeps_its_padding() {
        let name = DeviceName::new(DeviceName::CDJ_2000_NEXUS);
        assert_eq!(name.as_str(), "CDJ-2000nexus");
        assert_eq!(name.0, *b"CDJ-2000nexus\0\0\0\0\0\0\0");
    }

    #[test]
    fn a_name_longer_than_the_field_is_truncated() {
        let name = DeviceName::new("a name far longer than twenty bytes");
        assert_eq!(name.as_str(), "a name far longer th");
    }
}
