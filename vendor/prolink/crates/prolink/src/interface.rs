// SPDX-License-Identifier: GPL-3.0-only

//! Choosing the network interface that faces the CDJs.
//!
//! Three things are needed from it and none of them is optional:
//!
//! - the **MAC and IP** go verbatim into our keep-alive, and they must be the
//!   real ones: peers put the advertised address into their unicasts, so
//!   spoofing breaks the return path;
//! - the **broadcast address** is where discovery packets go;
//! - the **interface itself** matters on a multi-homed host. A Raspberry Pi has
//!   `eth0` on the CDJ network and `wlan0` elsewhere, and if a socket binds a
//!   source address on the wrong subnet, link-local routing silently sends the
//!   datagrams out the wrong NIC and every request times out with no error.
//!
//! An [`Interface`] therefore *is* the proof that all three are available:
//! enumeration drops anything without both an IPv4 address and a hardware
//! address, so a value of this type can always be announced from.
//!
//! Pro DJ Link addressing is link-local (`169.254.0.0/16`), self-assigned. A CDJ
//! tries DHCP about three times first and takes ~9 s to send its first packet
//! after power-on — worth knowing when timing a capture. **A CDJ does not answer
//! ICMP**, so ping is useless as a reachability test.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr};

use network_interface::{NetworkInterface, NetworkInterfaceConfig};
use prolink_proto::MacAddress;

use crate::{Error, Result};

/// A usable network interface: one with both an IPv4 address and a MAC.
#[derive(Clone, PartialEq, Eq)]
pub struct Interface {
    /// The kernel's name for it, e.g. `en9` or `eth0`.
    pub name: String,
    /// The index a socket can be pinned to.
    pub index: u32,
    /// Our address on this interface, which goes into our keep-alive.
    pub ip: Ipv4Addr,
    /// The subnet mask, used to derive the broadcast address.
    pub netmask: Ipv4Addr,
    /// Our hardware address, which also goes into our keep-alive.
    pub mac: MacAddress,
}

impl Interface {
    /// The link-local block Pro DJ Link self-assigns from.
    pub const LINK_LOCAL: Ipv4Addr = Ipv4Addr::new(169, 254, 0, 0);

    /// The subnet broadcast address.
    ///
    /// Computed from the address and mask rather than read from the system's
    /// `ifa_broadaddr`, because link-local interfaces do not always populate
    /// that field.
    pub fn broadcast(&self) -> Ipv4Addr {
        let host = u32::from(self.ip);
        let mask = u32::from(self.netmask);
        Ipv4Addr::from(host | !mask)
    }

    /// Whether this interface is on the link-local block CDJs use.
    pub fn is_link_local(&self) -> bool {
        self.ip.is_link_local()
    }

    /// Whether `peer` is on this interface's subnet.
    pub fn contains(&self, peer: Ipv4Addr) -> bool {
        let mask = u32::from(self.netmask);
        u32::from(self.ip) & mask == u32::from(peer) & mask
    }

    /// Every interface that could carry Pro DJ Link traffic.
    ///
    /// Loopback is excluded, and so is anything without both an IPv4 address
    /// and a hardware address — neither can announce, and a caller that got one
    /// back would have to re-check what this type is meant to establish.
    pub fn list() -> Result<Vec<Self>> {
        let mut found = Vec::new();
        for interface in NetworkInterface::show().map_err(Error::interfaces)? {
            let Some(mac) = interface.mac_addr.as_deref().and_then(parse_mac) else {
                continue;
            };
            for address in &interface.addr {
                let IpAddr::V4(ip) = address.ip() else {
                    continue;
                };
                if ip.is_loopback() || ip.is_unspecified() {
                    continue;
                }
                let netmask = match address.netmask() {
                    Some(IpAddr::V4(mask)) => mask,
                    _ => Ipv4Addr::new(255, 255, 0, 0),
                };
                found.push(Self {
                    name: interface.name.clone(),
                    index: interface.index,
                    ip,
                    netmask,
                    mac,
                });
            }
        }
        Ok(found)
    }

    /// The interface with this name.
    pub fn named(name: &str) -> Result<Self> {
        Self::list()?
            .into_iter()
            .find(|interface| interface.name == name)
            .ok_or_else(|| Error::NoSuchInterface {
                name: name.to_owned(),
            })
    }

    /// The interface whose subnet contains `peer`.
    pub fn for_peer(peer: Ipv4Addr) -> Result<Option<Self>> {
        Ok(Self::list()?
            .into_iter()
            .find(|interface| interface.contains(peer)))
    }

    /// The interface most likely to face the CDJs.
    ///
    /// Link-local first, since that is what Pro DJ Link self-assigns from and
    /// nothing else on a normal machine uses it. Falls back to any private
    /// address, then to whatever is left — but a caller that cares should ask
    /// the user, because guessing wrong on a multi-homed host fails silently.
    pub fn best_guess() -> Result<Self> {
        let interfaces = Self::list()?;
        let pick = interfaces
            .iter()
            .find(|interface| interface.is_link_local())
            .or_else(|| {
                interfaces
                    .iter()
                    .find(|interface| interface.ip.is_private())
            })
            .or_else(|| interfaces.first());
        pick.cloned().ok_or(Error::NoUsableInterface)
    }
}

impl fmt::Display for Interface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}/{} {}", self.name, self.ip, self.netmask, self.mac)?;
        if self.is_link_local() {
            f.write_str("  (link-local)")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Interface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Interface")
            .field("name", &self.name)
            .field("ip", &self.ip)
            .field("netmask", &self.netmask)
            .field("mac", &self.mac)
            .field("broadcast", &self.broadcast())
            .finish_non_exhaustive()
    }
}

/// Parse the `aa:bb:cc:dd:ee:ff` form the enumeration hands back.
fn parse_mac(text: &str) -> Option<MacAddress> {
    let mut octets = [0u8; 6];
    let mut parts = text.split(':');
    for octet in &mut octets {
        *octet = u8::from_str_radix(parts.next()?, 16).ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    // An all-zero MAC is what some virtual interfaces report; it cannot be
    // announced, so it is not a usable interface.
    (octets != [0; 6]).then_some(MacAddress(octets))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface(ip: [u8; 4], netmask: [u8; 4]) -> Interface {
        Interface {
            name: "test0".to_owned(),
            index: 1,
            ip: Ipv4Addr::from(ip),
            netmask: Ipv4Addr::from(netmask),
            mac: MacAddress([0xa0, 0xce, 0xc8, 0xe2, 0x26, 0xde]),
        }
    }

    #[test]
    fn the_broadcast_address_comes_from_the_mask() {
        let link_local = interface([169, 254, 99, 100], [255, 255, 0, 0]);
        assert_eq!(link_local.broadcast(), Ipv4Addr::new(169, 254, 255, 255));

        let narrow = interface([192, 168, 1, 5], [255, 255, 255, 0]);
        assert_eq!(narrow.broadcast(), Ipv4Addr::new(192, 168, 1, 255));
    }

    #[test]
    fn a_peer_on_the_same_subnet_is_recognised() {
        let link_local = interface([169, 254, 99, 100], [255, 255, 0, 0]);
        assert!(link_local.contains(Ipv4Addr::new(169, 254, 244, 181)));
        assert!(!link_local.contains(Ipv4Addr::new(192, 168, 1, 5)));
    }

    #[test]
    fn a_mac_is_parsed_from_the_colon_form() {
        assert_eq!(
            parse_mac("a0:ce:c8:e2:26:de"),
            Some(MacAddress([0xa0, 0xce, 0xc8, 0xe2, 0x26, 0xde]))
        );
    }

    #[test]
    fn an_unusable_mac_is_rejected_rather_than_zeroed() {
        assert_eq!(
            parse_mac("00:00:00:00:00:00"),
            None,
            "cannot announce from this"
        );
        assert_eq!(parse_mac("a0:ce:c8"), None, "too short");
        assert_eq!(parse_mac("a0:ce:c8:e2:26:de:ff"), None, "too long");
        assert_eq!(parse_mac("not a mac"), None);
    }

    #[test]
    fn enumeration_only_returns_interfaces_that_could_announce() {
        // Runs against the real machine; the assertion is on the invariant, not
        // on which interfaces this particular host happens to have.
        for interface in Interface::list().expect("enumeration works on this host") {
            assert_ne!(
                interface.mac.0, [0u8; 6],
                "{} has no usable MAC",
                interface.name
            );
            assert!(
                !interface.ip.is_loopback(),
                "{} is loopback",
                interface.name
            );
        }
    }
}
