// SPDX-License-Identifier: GPL-3.0-only

//! The peer table: who is on the Pro DJ Link network right now.
//!
//! # Keyed by MAC
//!
//! Not by device number, and not by address. Both of the obvious alternatives
//! are unstable in ways that matter: a player's number can be reassigned during
//! the startup handshake, and its address changes if the network switches
//! between DHCP and link-local self-assignment. The MAC is the one identifier
//! that survives both, which also makes it the right key for a library cache.
//!
//! # Two-tier lifetime
//!
//! A CDJ can drop off for a couple of seconds — a nudged cable, a switch
//! reconverging — and come straight back. Treating that as "device removed"
//! would tear down whatever the caller has built on top of it for a blip. So a
//! device that stops sending keep-alives first goes **stale** (still listed,
//! marked offline, all state retained) and only later is **forgotten**.
//!
//! The stale threshold is five missed keep-alives at the measured 2.0 s cadence,
//! not the six or seven that a mistaken 1.5 s cadence implies (C12).
//!
//! # Numbers are remembered after their owner is not
//!
//! [`DeviceTable::numbers_seen`] is never pruned, and that is deliberate. An
//! XDJ-XZ and an Opus Quad do not defend their numbers with conflict packets at
//! all, so "I have not seen a conflict" is *not* evidence that a number is free
//! — only "I have never seen anyone use it" is, and that requires remembering
//! numbers belonging to devices that have since gone quiet.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use prolink_proto::djl::{self, Body};
use prolink_proto::{DeviceKind, DeviceName, DeviceNumber, MacAddress};

/// A device silent for this long is marked offline, but kept.
pub const STALE_AFTER: Duration = djl::DEVICE_TIMEOUT;

/// ...and dropped entirely this long after that.
pub const FORGET_AFTER: Duration = Duration::from_secs(60);

/// One peer, as learned purely by listening.
#[derive(Clone, PartialEq, Eq)]
pub struct Device {
    /// The key. Stable across renumbering and readdressing.
    pub mac: MacAddress,
    /// Where to unicast to.
    pub ip: Ipv4Addr,
    /// The name it announces, and the literal bytes of that field — the padding
    /// is part of what makes an announcement recognisable.
    pub name: DeviceName,
    /// The number it currently holds.
    pub number: DeviceNumber,
    /// What kind of device it says it is.
    pub kind: DeviceKind,
    /// How many peers it says it can see, including itself.
    pub peer_count: u8,
    /// Keep-alive byte `0x35`, a product-generation code.
    pub generation: u8,
    /// When we first heard from it.
    pub first_seen: Instant,
    /// When we last heard from it.
    pub last_seen: Instant,
    /// How many keep-alives we have folded in.
    pub keep_alives: u64,
    /// Whether it has gone quiet.
    pub offline: bool,
}

impl Device {
    /// Product generation, from keep-alive byte `0x35`.
    ///
    /// The pre-hardware literature reads this byte as "00 classic, 01 typical
    /// CDJ, 64 CDJ-3000", which would make every nexus deck "classic". It is
    /// wrong: both a CDJ-2000nexus and a DJM-2000nexus send **`0x00`** (C3), and
    /// `0x01` has never been observed on hardware at all.
    pub fn generation_name(&self) -> &'static str {
        match self.generation {
            0x00 => "nexus or earlier",
            0x01 => "0x01 (documented, never observed)",
            0x20 => "Stagehand",
            0x64 => "CDJ-3000",
            _ => "unknown",
        }
    }

    /// How long since its last keep-alive.
    pub fn silence(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.last_seen)
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:>2}  {:<20}  {:<15}  {}  {}",
            self.number,
            self.name,
            self.ip,
            self.mac,
            self.generation_name()
        )?;
        if self.offline {
            f.write_str("  (offline)")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Device")
            .field("number", &self.number)
            .field("name", &self.name)
            .field("ip", &self.ip)
            .field("mac", &self.mac)
            .field("kind", &self.kind)
            .field("offline", &self.offline)
            .finish_non_exhaustive()
    }
}

/// Something changed in the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceEvent {
    /// A device we had not seen before.
    Found(Device),
    /// A device whose number, address or name changed.
    Updated(Device),
    /// A device that stopped sending keep-alives.
    WentOffline(Device),
    /// A device that came back before being forgotten.
    CameBack(Device),
    /// A device dropped from the table entirely.
    Forgotten(Device),
}

impl DeviceEvent {
    /// The device this event is about.
    pub fn device(&self) -> &Device {
        match self {
            Self::Found(device)
            | Self::Updated(device)
            | Self::WentOffline(device)
            | Self::CameBack(device)
            | Self::Forgotten(device) => device,
        }
    }
}

/// Peers keyed by MAC, with staleness and forgetting.
#[derive(Debug)]
pub struct DeviceTable {
    devices: BTreeMap<[u8; 6], Device>,
    numbers_seen: BTreeSet<u8>,
    ignored: BTreeSet<[u8; 6]>,
    stale_after: Duration,
    forget_after: Duration,
}

impl Default for DeviceTable {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceTable {
    /// An empty table with the observed timings.
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
            numbers_seen: BTreeSet::new(),
            ignored: BTreeSet::new(),
            stale_after: STALE_AFTER,
            forget_after: FORGET_AFTER,
        }
    }

    /// Ignore packets from this hardware address.
    ///
    /// We bind `0.0.0.0` and so receive our own broadcasts. Without this, an
    /// announcing virtual CDJ lists itself as a peer — confusing in output, and
    /// it would corrupt the peer count we put in our own keep-alive.
    pub fn ignore(&mut self, mac: MacAddress) {
        self.ignored.insert(mac.0);
    }

    /// Fold one decoded UDP-50000 packet into the table.
    ///
    /// **Only keep-alives create devices.** They are the only packet carrying
    /// the full set — number, name, MAC, address — and the only one a settled
    /// device keeps sending. Claim packets contribute their proposed number to
    /// [`Self::numbers_seen`] but do not create entries, because a device
    /// mid-handshake may never end up with the number it is proposing.
    pub fn observe(&mut self, packet: &djl::Packet, now: Instant) -> Option<DeviceEvent> {
        if packet
            .body
            .mac()
            .is_some_and(|mac| self.ignored.contains(&mac.0))
        {
            return None;
        }

        let Body::KeepAlive {
            device_number,
            mac,
            ip,
            peer_count,
            trailing,
            ..
        } = packet.body
        else {
            if let Some(number) = packet.body.device_number().filter(|&number| number != 0) {
                self.numbers_seen.insert(number);
            }
            return None;
        };

        self.numbers_seen.insert(device_number);
        // A keep-alive naming device 0 cannot be addressed, so it cannot become
        // a peer. Its number still counts as seen, above.
        let number = DeviceNumber::new(device_number)?;

        match self.devices.get_mut(&mac.0) {
            None => {
                let device = Device {
                    mac,
                    ip,
                    name: packet.name,
                    number,
                    kind: packet.device_kind,
                    peer_count,
                    generation: trailing,
                    first_seen: now,
                    last_seen: now,
                    keep_alives: 1,
                    offline: false,
                };
                self.devices.insert(mac.0, device.clone());
                Some(DeviceEvent::Found(device))
            }
            Some(existing) => {
                let was_offline = existing.offline;
                let changed =
                    existing.number != number || existing.ip != ip || existing.name != packet.name;
                existing.last_seen = now;
                existing.keep_alives = existing.keep_alives.saturating_add(1);
                existing.number = number;
                existing.ip = ip;
                existing.name = packet.name;
                existing.kind = packet.device_kind;
                existing.peer_count = peer_count;
                existing.generation = trailing;
                existing.offline = false;

                if was_offline {
                    Some(DeviceEvent::CameBack(existing.clone()))
                } else if changed {
                    Some(DeviceEvent::Updated(existing.clone()))
                } else {
                    None
                }
            }
        }
    }

    /// Mark newly-silent devices offline and drop long-gone ones.
    pub fn reap(&mut self, now: Instant) -> Vec<DeviceEvent> {
        let mut events = Vec::new();
        self.devices.retain(|_, device| {
            let silence = device.silence(now);
            if silence > self.stale_after + self.forget_after {
                events.push(DeviceEvent::Forgotten(device.clone()));
                false
            } else {
                if silence > self.stale_after && !device.offline {
                    device.offline = true;
                    events.push(DeviceEvent::WentOffline(device.clone()));
                }
                true
            }
        });
        events
    }

    /// Drop every offline device now, without waiting out the grace period.
    pub fn forget_offline(&mut self) -> Vec<DeviceEvent> {
        let mut events = Vec::new();
        self.devices.retain(|_, device| {
            if device.offline {
                events.push(DeviceEvent::Forgotten(device.clone()));
                false
            } else {
                true
            }
        });
        events
    }

    /// Every device, ordered by number then address.
    pub fn all(&self) -> Vec<Device> {
        let mut devices: Vec<Device> = self.devices.values().cloned().collect();
        devices.sort_by_key(|device| (device.number, device.ip));
        devices
    }

    /// Devices currently answering.
    pub fn online(&self) -> Vec<Device> {
        self.all()
            .into_iter()
            .filter(|device| !device.offline)
            .collect()
    }

    /// The device holding this number, if any.
    pub fn by_number(&self, number: DeviceNumber) -> Option<Device> {
        self.devices
            .values()
            .find(|device| device.number == number)
            .cloned()
    }

    /// The device at this address, if any.
    pub fn by_ip(&self, ip: Ipv4Addr) -> Option<Device> {
        self.devices
            .values()
            .find(|device| device.ip == ip)
            .cloned()
    }

    /// Every device number ever observed, including from devices since gone.
    pub fn numbers_seen(&self) -> &BTreeSet<u8> {
        &self.numbers_seen
    }

    /// How many devices are in the table.
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// Whether the table is empty — which is also how a virtual CDJ decides
    /// whether it was first onto the network.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keep_alive(number: u8, mac: [u8; 6], ip: [u8; 4]) -> djl::Packet {
        djl::Packet::new(
            DeviceName::new("CDJ-2000nexus"),
            DeviceKind::CDJ,
            Body::KeepAlive {
                device_number: number,
                was_first_on_network: 0x02,
                mac: MacAddress(mac),
                ip: Ipv4Addr::from(ip),
                peer_count: 1,
                pad_31: [0; 3],
                flags: 0x01,
                trailing: 0x00,
            },
        )
    }

    #[test]
    fn a_keep_alive_creates_a_device() {
        let mut table = DeviceTable::new();
        let event = table.observe(
            &keep_alive(2, [1, 2, 3, 4, 5, 6], [169, 254, 1, 2]),
            Instant::now(),
        );
        assert!(matches!(event, Some(DeviceEvent::Found(_))));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn a_claim_contributes_its_number_without_creating_a_device() {
        // A device mid-handshake may never end up with the number it proposes.
        let mut table = DeviceTable::new();
        let claim = djl::Packet::new(
            DeviceName::default(),
            DeviceKind::CDJ,
            Body::ClaimNumber {
                device_number: 3,
                iteration: 1,
            },
        );
        assert!(table.observe(&claim, Instant::now()).is_none());
        assert!(table.is_empty());
        assert!(
            table.numbers_seen().contains(&3),
            "but the number is remembered"
        );
    }

    #[test]
    fn renumbering_is_an_update_not_a_new_device() {
        let mut table = DeviceTable::new();
        let now = Instant::now();
        table.observe(&keep_alive(2, [1, 2, 3, 4, 5, 6], [169, 254, 1, 2]), now);
        let event = table.observe(&keep_alive(4, [1, 2, 3, 4, 5, 6], [169, 254, 1, 2]), now);
        assert!(matches!(event, Some(DeviceEvent::Updated(_))));
        assert_eq!(table.len(), 1, "keyed by MAC, so this is the same device");
    }

    #[test]
    fn readdressing_is_an_update_not_a_new_device() {
        // DHCP giving up and link-local taking over is exactly this.
        let mut table = DeviceTable::new();
        let now = Instant::now();
        table.observe(&keep_alive(2, [1, 2, 3, 4, 5, 6], [192, 168, 1, 5]), now);
        let event = table.observe(&keep_alive(2, [1, 2, 3, 4, 5, 6], [169, 254, 1, 2]), now);
        assert!(matches!(event, Some(DeviceEvent::Updated(_))));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn a_repeated_keep_alive_is_not_an_event() {
        let mut table = DeviceTable::new();
        let now = Instant::now();
        let packet = keep_alive(2, [1, 2, 3, 4, 5, 6], [169, 254, 1, 2]);
        table.observe(&packet, now);
        assert!(table.observe(&packet, now).is_none(), "nothing changed");
    }

    #[test]
    fn silence_makes_a_device_offline_before_it_is_forgotten() {
        let mut table = DeviceTable::new();
        let start = Instant::now();
        table.observe(&keep_alive(2, [1, 2, 3, 4, 5, 6], [169, 254, 1, 2]), start);

        assert!(
            table.reap(start + Duration::from_secs(5)).is_empty(),
            "still fresh"
        );

        let events = table.reap(start + STALE_AFTER + Duration::from_secs(1));
        assert!(matches!(events.as_slice(), [DeviceEvent::WentOffline(_)]));
        assert_eq!(table.len(), 1, "kept, so a blip does not tear down state");
        assert!(table.online().is_empty());

        let events = table.reap(start + STALE_AFTER + FORGET_AFTER + Duration::from_secs(1));
        assert!(matches!(events.as_slice(), [DeviceEvent::Forgotten(_)]));
        assert!(table.is_empty());
    }

    #[test]
    fn a_device_that_comes_back_is_not_a_new_device() {
        let mut table = DeviceTable::new();
        let start = Instant::now();
        let packet = keep_alive(2, [1, 2, 3, 4, 5, 6], [169, 254, 1, 2]);
        table.observe(&packet, start);
        table.reap(start + STALE_AFTER + Duration::from_secs(1));

        let event = table.observe(&packet, start + STALE_AFTER + Duration::from_secs(2));
        assert!(matches!(event, Some(DeviceEvent::CameBack(_))));
        assert_eq!(table.online().len(), 1);
    }

    #[test]
    fn a_forgotten_devices_number_is_still_remembered() {
        // Silence is not evidence a number is free: an XDJ-XZ and an Opus Quad
        // do not defend their numbers at all.
        let mut table = DeviceTable::new();
        let start = Instant::now();
        table.observe(&keep_alive(2, [1, 2, 3, 4, 5, 6], [169, 254, 1, 2]), start);
        table.reap(start + STALE_AFTER + FORGET_AFTER + Duration::from_secs(1));
        assert!(table.is_empty());
        assert!(table.numbers_seen().contains(&2));
    }

    #[test]
    fn our_own_broadcasts_are_not_peers() {
        let mut table = DeviceTable::new();
        table.ignore(MacAddress([1, 2, 3, 4, 5, 6]));
        let event = table.observe(
            &keep_alive(2, [1, 2, 3, 4, 5, 6], [169, 254, 1, 2]),
            Instant::now(),
        );
        assert!(event.is_none());
        assert!(table.is_empty());
    }
}
