use get_if_addrs::IfAddr;

use crate::{
    InterfaceAddresses, InterfaceStatus, InterfaceType, NetworkInterface, NetworkSnapshot,
};

pub fn read_network_snapshot(include_mac_address: bool) -> crate::Result<NetworkSnapshot> {
    let _ = include_mac_address;

    let mut interfaces = get_if_addrs::get_if_addrs()?
        .into_iter()
        .map(|interface| {
            let interface_type = classify_interface(&interface.name);
            let status = if interface_type == InterfaceType::Loopback {
                InterfaceStatus::Down
            } else {
                InterfaceStatus::Up
            };

            let mut addresses = InterfaceAddresses::default();
            match interface.addr {
                IfAddr::V4(addr) => addresses.ipv4.push(addr.ip.to_string()),
                IfAddr::V6(addr) => addresses.ipv6.push(addr.ip.to_string()),
            }

            NetworkInterface {
                id: sanitize_id(&interface.name),
                name: interface.name.clone(),
                display_name: interface.name,
                interface_type,
                status,
                is_primary: false,
                addresses,
                gateway: None,
                dns_servers: Vec::new(),
            }
        })
        .collect::<Vec<_>>();

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

    #[test]
    fn classifies_common_interface_names() {
        assert_eq!(classify_interface("Wi-Fi"), InterfaceType::Wifi);
        assert_eq!(classify_interface("en0"), InterfaceType::Wifi);
        assert_eq!(classify_interface("Ethernet"), InterfaceType::Ethernet);
        assert_eq!(classify_interface("eth0"), InterfaceType::Ethernet);
        assert_eq!(classify_interface("utun0"), InterfaceType::Vpn);
        assert_eq!(classify_interface("lo0"), InterfaceType::Loopback);
    }
}
