//! Passive interface selection. Observer registration remains in the live session.
use prolink::{Discovery, Interface};
use std::time::Duration;

pub(crate) fn matching_interfaces(
    interfaces: &[Interface],
    peer: std::net::Ipv4Addr,
) -> Vec<Interface> {
    interfaces
        .iter()
        .filter(|i| i.contains(peer) && i.ip != peer)
        .cloned()
        .collect()
}

fn is_player(device: &prolink::Device) -> bool {
    let name = device.name.to_string();
    device.number.get() <= 6
        && (device.kind == prolink_proto::DeviceKind::CDJ
            || name.starts_with("CDJ-")
            || name.starts_with("XDJ-"))
}

pub(crate) async fn select(mode: &str) -> Result<(Interface, Discovery), String> {
    let interfaces = Interface::list().map_err(|e| e.to_string())?;
    let mut listeners = Vec::new();
    let mut errors = Vec::new();
    for interface in interfaces
        .iter()
        .filter(|i| mode == "auto" || i.name == mode)
    {
        match Discovery::start(interface.clone()).await {
            Ok(discovery) => listeners.push((interface.clone(), discovery)),
            Err(error) => errors.push(format!("{}: {error}", interface.name)),
        }
    }
    if listeners.is_empty() {
        return Err(if errors.is_empty() {
            "No usable CDJ network interface. Connect Ethernet or Wi-Fi; retrying automatically."
                .into()
        } else {
            format!(
                "Cannot listen for CDJs: {}. Check network permissions; retrying automatically.",
                errors.join("; ")
            )
        });
    }
    // A settled CDJ announces every two seconds. Allow a missed announcement.
    tokio::time::sleep(Duration::from_millis(4500)).await;
    let candidates: Vec<_> = listeners
        .iter()
        .enumerate()
        .filter_map(|(index, (interface, discovery))| {
            discovery
                .online()
                .iter()
                .any(|device| {
                    is_player(device)
                        && interface.contains(device.ip)
                        && interface.ip != device.ip
                        && (mode != "auto"
                            || matching_interfaces(&interfaces, device.ip).len() == 1)
                })
                .then_some(index)
        })
        .collect();
    match candidates.as_slice() {
        [index] => Ok(listeners.swap_remove(*index)),
        [] => Err("Searching for CDJs. Check the shared network and firewall. If networks overlap, select the CDJ interface manually.".into()),
        _ => Err("CDJs found on multiple networks. Select the CDJ discovery interface in Settings.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn interface(name: &str, ip: [u8; 4]) -> Interface {
        Interface {
            name: name.into(),
            index: 1,
            ip: ip.into(),
            netmask: [255, 255, 255, 0].into(),
            mac: prolink_proto::MacAddress([1, 2, 3, 4, 5, 6]),
        }
    }
    #[test]
    fn chooses_cdj_subnet_and_excludes_own_address() {
        let networks = vec![
            interface("eth0", [192, 168, 1, 2]),
            interface("wlan0", [192, 168, 2, 2]),
        ];
        assert_eq!(
            matching_interfaces(&networks, [192, 168, 1, 3].into()),
            vec![networks[0].clone()]
        );
        assert!(matching_interfaces(&networks, [192, 168, 1, 2].into()).is_empty());
        assert!(matching_interfaces(&networks, [10, 0, 0, 4].into()).is_empty());
    }
    #[test]
    fn overlapping_networks_are_ambiguous() {
        let networks = vec![
            interface("eth0", [192, 168, 1, 2]),
            interface("wlan0", [192, 168, 1, 4]),
        ];
        assert_eq!(
            matching_interfaces(&networks, [192, 168, 1, 3].into()).len(),
            2
        );
    }
}
