use get_if_addrs::IfAddr;
use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::{
    InterfaceAddresses, InterfaceStatus, InterfaceType, NetworkInterface, NetworkSnapshot,
};

pub fn read_network_snapshot(include_mac_address: bool) -> crate::Result<NetworkSnapshot> {
    let _ = include_mac_address;

    let mut interfaces = build_interfaces_from_entries(
        get_if_addrs::get_if_addrs()?
            .into_iter()
            .map(|interface| {
                let ip = match interface.addr {
                    IfAddr::V4(addr) => IpAddr::V4(addr.ip),
                    IfAddr::V6(addr) => IpAddr::V6(addr.ip),
                };

                (interface.name, ip)
            }),
    );

    mark_primary(&mut interfaces);
    let primary_interface_id = interfaces
        .iter()
        .find(|interface| interface.is_primary)
        .map(|interface| interface.id.clone());

    Ok(NetworkSnapshot {
        primary_interface_id,
        interfaces,
    })
}

fn build_interfaces_from_entries(
    entries: impl IntoIterator<Item = (String, IpAddr)>,
) -> Vec<NetworkInterface> {
    let mut interfaces = entries
        .into_iter()
        .fold(BTreeMap::new(), |mut interfaces, (name, ip)| {
            let interface = interfaces.entry(name.clone()).or_insert_with(|| {
                let interface_type = classify_interface(&name);
                let status = if interface_type == InterfaceType::Loopback {
                    InterfaceStatus::Down
                } else {
                    InterfaceStatus::Up
                };

                NetworkInterface {
                    id: sanitize_id(&name),
                    name: name.clone(),
                    display_name: name,
                    interface_type,
                    status,
                    is_primary: false,
                    addresses: InterfaceAddresses::default(),
                    gateway: None,
                    dns_servers: Vec::new(),
                }
            });

            match ip {
                IpAddr::V4(ip) => interface.addresses.ipv4.push(ip.to_string()),
                IpAddr::V6(ip) => interface.addresses.ipv6.push(ip.to_string()),
            }

            interfaces
        })
        .into_values()
        .collect::<Vec<_>>();

    assign_unique_ids(&mut interfaces);

    for interface in &mut interfaces {
        interface.addresses.ipv4.sort();
        interface.addresses.ipv4.dedup();
        interface.addresses.ipv6.sort();
        interface.addresses.ipv6.dedup();
    }

    interfaces
}

fn assign_unique_ids(interfaces: &mut [NetworkInterface]) {
    let mut seen = BTreeMap::<String, usize>::new();

    for interface in interfaces {
        let base_id = sanitize_id(&interface.name);
        let count = seen.entry(base_id.clone()).or_insert(0);
        *count += 1;

        interface.id = if *count == 1 {
            base_id
        } else {
            format!("{base_id}_{count}")
        };
    }
}

pub fn has_available_interface(snapshot: &NetworkSnapshot) -> bool {
    snapshot.interfaces.iter().any(|interface| {
        interface.status == InterfaceStatus::Up
            && interface.interface_type != InterfaceType::Loopback
            && (!interface.addresses.ipv4.is_empty() || !interface.addresses.ipv6.is_empty())
    })
}

pub fn classify_interface(name: &str) -> InterfaceType {
    let normalized = name.to_ascii_lowercase();

    if normalized.starts_with("lo") || normalized.contains("loopback") {
        InterfaceType::Loopback
    } else if normalized.contains("wi-fi")
        || normalized.contains("wifi")
        || normalized.contains("wireless")
        || normalized == "en0"
    {
        InterfaceType::Wifi
    } else if normalized.starts_with("utun")
        || normalized.starts_with("tun")
        || normalized.starts_with("tap")
        || normalized.contains("vpn")
    {
        InterfaceType::Vpn
    } else if normalized.contains("ethernet")
        || normalized.starts_with("eth")
        || normalized.starts_with("en")
    {
        InterfaceType::Ethernet
    } else {
        InterfaceType::Unknown
    }
}

fn sanitize_id(name: &str) -> String {
    let mut sanitized = String::from("if_");
    let mut last_was_separator = false;

    for character in name.chars().flat_map(|character| character.to_lowercase()) {
        if character.is_ascii_alphanumeric() {
            sanitized.push(character);
            last_was_separator = false;
        } else if !last_was_separator && sanitized.len() > 3 {
            sanitized.push('_');
            last_was_separator = true;
        }
    }

    while sanitized.ends_with('_') {
        sanitized.pop();
    }

    if sanitized.len() == 3 {
        sanitized.push_str("unknown");
    }

    sanitized
}

fn mark_primary(interfaces: &mut [NetworkInterface]) {
    if let Some(interface) = interfaces.iter_mut().find(|interface| {
        interface.status == InterfaceStatus::Up
            && interface.interface_type != InterfaceType::Loopback
            && !interface.addresses.ipv4.is_empty()
    }) {
        interface.is_primary = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn classifies_common_interface_names() {
        assert_eq!(classify_interface("Wi-Fi"), InterfaceType::Wifi);
        assert_eq!(classify_interface("en0"), InterfaceType::Wifi);
        assert_eq!(classify_interface("Ethernet"), InterfaceType::Ethernet);
        assert_eq!(classify_interface("eth0"), InterfaceType::Ethernet);
        assert_eq!(classify_interface("utun0"), InterfaceType::Vpn);
        assert_eq!(classify_interface("lo0"), InterfaceType::Loopback);
    }

    #[test]
    fn aggregates_addresses_for_same_interface() {
        let interfaces = build_interfaces_from_entries(vec![
            ("en0".to_string(), IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))),
            ("en0".to_string(), IpAddr::V6(Ipv6Addr::LOCALHOST)),
        ]);

        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].id, "if_en0");
        assert_eq!(interfaces[0].addresses.ipv4, vec!["192.168.1.10"]);
        assert_eq!(interfaces[0].addresses.ipv6, vec!["::1"]);
    }

    #[test]
    fn keeps_interfaces_with_colliding_sanitized_ids_separate() {
        let interfaces = build_interfaces_from_entries(vec![
            (
                "Ethernet 1".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            ),
            (
                "Ethernet-1".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 11)),
            ),
        ]);

        assert_eq!(interfaces.len(), 2);
        assert_eq!(interfaces[0].name, "Ethernet 1");
        assert_eq!(interfaces[0].id, "if_ethernet_1");
        assert_eq!(interfaces[1].name, "Ethernet-1");
        assert_eq!(interfaces[1].id, "if_ethernet_1_2");
    }
}
