use get_if_addrs::IfAddr;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use crate::{
    InterfaceAddresses, InterfaceStatus, InterfaceType, NetworkInterface, NetworkSnapshot,
};

pub fn read_network_snapshot(include_mac_address: bool) -> crate::Result<NetworkSnapshot> {
    if include_mac_address {
        return Err(crate::Error::invalid_config(
            "MAC address collection is not supported by the get_if_addrs backend",
        ));
    }

    let mut interfaces =
        build_interfaces_from_entries(get_if_addrs::get_if_addrs()?.into_iter().map(|interface| {
            let ip = match interface.addr {
                IfAddr::V4(addr) => IpAddr::V4(addr.ip),
                IfAddr::V6(addr) => IpAddr::V6(addr.ip),
            };

            (interface.name, ip)
        }));

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
                    addresses: InterfaceAddresses {
                        // get_if_addrs does not expose MAC addresses, so this backend always leaves it unset.
                        mac: None,
                        ..InterfaceAddresses::default()
                    },
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
    let mut assigned = BTreeSet::<String>::new();
    let reserved_base_ids = interfaces
        .iter()
        .map(|interface| sanitize_id(&interface.name))
        .collect::<BTreeSet<_>>();

    for interface in interfaces {
        let base_id = sanitize_id(&interface.name);
        let mut candidate = base_id.clone();
        let mut suffix = 2;

        while assigned.contains(&candidate)
            || (candidate != base_id && reserved_base_ids.contains(&candidate))
        {
            candidate = format!("{base_id}_{suffix}");
            suffix += 1;
        }

        assigned.insert(candidate.clone());
        interface.id = candidate;
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

    if is_loopback_name(&normalized) {
        InterfaceType::Loopback
    } else if normalized.contains("wi-fi")
        || normalized.contains("wifi")
        || normalized.contains("wlan")
        || normalized.contains("wireless")
        || normalized.contains("\u{65e0}\u{7ebf}")
        || normalized == "en0"
    {
        InterfaceType::Wifi
    } else if normalized.starts_with("utun")
        || normalized.starts_with("tun")
        || normalized.starts_with("tap")
        || normalized.contains("vpn")
        || normalized.contains("tunnel")
        || normalized.contains("meta")
        || normalized.contains("tailscale")
        || normalized.contains("zerotier")
        || normalized.contains("clash")
        || normalized.contains("mihomo")
    {
        InterfaceType::Vpn
    } else if normalized.contains("ethernet")
        || normalized.contains("\u{4ee5}\u{592a}")
        || normalized.contains("local area connection")
        || normalized.contains("\u{672c}\u{5730}\u{8fde}\u{63a5}")
        || normalized.starts_with("eth")
        || normalized.starts_with("en")
    {
        InterfaceType::Ethernet
    } else {
        InterfaceType::Unknown
    }
}

fn is_loopback_name(normalized: &str) -> bool {
    normalized == "lo"
        || normalized.strip_prefix("lo").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit())
        })
        || normalized.contains("loopback")
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
    if let Some(index) = interfaces
        .iter()
        .enumerate()
        .filter(|(_, interface)| can_be_primary(interface))
        .min_by_key(|(_, interface)| primary_priority(interface))
        .map(|(index, _)| index)
    {
        let interface = &mut interfaces[index];
        interface.is_primary = true;
    }
}

fn can_be_primary(interface: &NetworkInterface) -> bool {
    interface.status == InterfaceStatus::Up
        && interface.interface_type != InterfaceType::Loopback
        && !interface.addresses.ipv4.is_empty()
}

fn primary_priority(interface: &NetworkInterface) -> (u8, u8) {
    (
        if has_non_link_local_ipv4(interface) {
            0
        } else {
            1
        },
        match interface.interface_type {
            InterfaceType::Wifi | InterfaceType::Ethernet => 0,
            InterfaceType::Vpn => 1,
            InterfaceType::Unknown => 2,
            InterfaceType::Loopback => 3,
        },
    )
}

fn has_non_link_local_ipv4(interface: &NetworkInterface) -> bool {
    interface.addresses.ipv4.iter().any(|address| {
        address
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| !ip.is_link_local())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn chinese(text: &[char]) -> String {
        text.iter().collect()
    }

    fn wireless_name() -> String {
        chinese(&[
            '\u{65e0}', '\u{7ebf}', '\u{7f51}', '\u{7edc}', '\u{8fde}', '\u{63a5}',
        ])
    }

    fn ethernet_name() -> String {
        chinese(&['\u{4ee5}', '\u{592a}', '\u{7f51}'])
    }

    fn local_connection_name() -> String {
        chinese(&['\u{672c}', '\u{5730}', '\u{8fde}', '\u{63a5}'])
    }

    #[test]
    fn classifies_common_interface_names() {
        assert_eq!(classify_interface("Wi-Fi"), InterfaceType::Wifi);
        assert_eq!(classify_interface("WLAN"), InterfaceType::Wifi);
        assert_eq!(classify_interface(&wireless_name()), InterfaceType::Wifi);
        assert_eq!(classify_interface("en0"), InterfaceType::Wifi);
        assert_eq!(classify_interface("Ethernet"), InterfaceType::Ethernet);
        assert_eq!(
            classify_interface(&ethernet_name()),
            InterfaceType::Ethernet
        );
        assert_eq!(
            classify_interface("Local Area Connection"),
            InterfaceType::Ethernet
        );
        assert_eq!(
            classify_interface(&local_connection_name()),
            InterfaceType::Ethernet
        );
        assert_eq!(classify_interface("eth0"), InterfaceType::Ethernet);
        assert_eq!(classify_interface("utun0"), InterfaceType::Vpn);
        assert_eq!(classify_interface("Meta"), InterfaceType::Vpn);
        assert_eq!(classify_interface("Meta Tunnel"), InterfaceType::Vpn);
        assert_eq!(classify_interface("lo0"), InterfaceType::Loopback);
    }

    #[test]
    fn aggregates_addresses_for_same_interface() {
        let interfaces = build_interfaces_from_entries(vec![
            (
                "en0".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            ),
            ("en0".to_string(), IpAddr::V6(Ipv6Addr::LOCALHOST)),
        ]);

        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].id, "if_en0");
        assert_eq!(interfaces[0].addresses.ipv4, vec!["192.168.1.10"]);
        assert_eq!(interfaces[0].addresses.ipv6, vec!["::1"]);
    }

    #[test]
    fn primary_interface_prefers_physical_network_over_tunnel() {
        let mut interfaces = build_interfaces_from_entries(vec![
            ("Meta".to_string(), IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))),
            (
                "WLAN".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 5, 35)),
            ),
        ]);
        mark_primary(&mut interfaces);

        assert_eq!(
            interfaces
                .iter()
                .find(|interface| interface.is_primary)
                .map(|interface| interface.name.as_str()),
            Some("WLAN")
        );
    }

    #[test]
    fn primary_interface_prefers_non_link_local_physical_network() {
        let mut interfaces = build_interfaces_from_entries(vec![
            (
                local_connection_name(),
                IpAddr::V4(Ipv4Addr::new(169, 254, 235, 83)),
            ),
            (
                "WLAN".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 5, 35)),
            ),
        ]);
        mark_primary(&mut interfaces);

        assert_eq!(
            interfaces
                .iter()
                .find(|interface| interface.is_primary)
                .map(|interface| interface.name.as_str()),
            Some("WLAN")
        );
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

    #[test]
    fn avoids_final_id_collisions_after_suffixing() {
        let interfaces = build_interfaces_from_entries(vec![
            (
                "Ethernet 1".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            ),
            (
                "Ethernet-1".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 11)),
            ),
            (
                "Ethernet_1_2".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 12)),
            ),
        ]);

        let ids = interfaces
            .iter()
            .map(|interface| interface.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec!["if_ethernet_1", "if_ethernet_1_3", "if_ethernet_1_2"]
        );
    }

    #[test]
    fn rejects_mac_address_collection_request() {
        let error = read_network_snapshot(true).unwrap_err();

        assert_eq!(error.code(), "invalid_config");
    }
}
