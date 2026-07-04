use get_if_addrs::IfAddr;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use crate::{
    InterfaceAddresses, InterfaceStatus, InterfaceType, NetworkInterface, NetworkSnapshot,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PlatformInterfaceDetails {
    interface_type: Option<InterfaceType>,
    gateway: Option<String>,
    dns_servers: Vec<String>,
    mac: Option<String>,
}

pub fn read_network_snapshot(include_mac_address: bool) -> crate::Result<NetworkSnapshot> {
    if include_mac_address {
        return Err(crate::Error::invalid_config(
            "MAC address collection is not supported by the get_if_addrs backend",
        ));
    }

    let interface_details = read_platform_interface_details();
    let mut interfaces = build_interfaces_from_entries_with_platform_details(
        get_if_addrs::get_if_addrs()?.into_iter().map(|interface| {
            let ip = match interface.addr {
                IfAddr::V4(addr) => IpAddr::V4(addr.ip),
                IfAddr::V6(addr) => IpAddr::V6(addr.ip),
            };

            (interface.name, ip)
        }),
        &interface_details,
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

#[cfg(test)]
fn build_interfaces_from_entries(
    entries: impl IntoIterator<Item = (String, IpAddr)>,
) -> Vec<NetworkInterface> {
    build_interfaces_from_entries_with_platform_details(entries, &BTreeMap::new())
}

fn build_interfaces_from_entries_with_platform_details(
    entries: impl IntoIterator<Item = (String, IpAddr)>,
    interface_details: &BTreeMap<String, PlatformInterfaceDetails>,
) -> Vec<NetworkInterface> {
    let mut interfaces = entries
        .into_iter()
        .fold(BTreeMap::new(), |mut interfaces, (name, ip)| {
            let interface = interfaces.entry(name.clone()).or_insert_with(|| {
                let details = interface_details
                    .get(&normalize_interface_key(&name))
                    .cloned()
                    .unwrap_or_default();
                let interface_type = resolve_interface_type(&name, &details);
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
                        mac: details.mac,
                        ..InterfaceAddresses::default()
                    },
                    gateway: details.gateway,
                    dns_servers: details.dns_servers,
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

fn resolve_interface_type(name: &str, details: &PlatformInterfaceDetails) -> InterfaceType {
    let name_type = classify_interface(name);

    if should_prefer_name_classification(name_type.clone()) {
        return name_type;
    }

    details
        .interface_type
        .clone()
        .filter(|interface_type| *interface_type != InterfaceType::Unknown)
        .unwrap_or(name_type)
}

fn normalize_interface_key(name: &str) -> String {
    name.trim_matches(|ch| ch == '{' || ch == '}')
        .to_ascii_lowercase()
}

#[cfg(target_os = "macos")]
fn should_prefer_name_classification(interface_type: InterfaceType) -> bool {
    matches!(
        interface_type,
        InterfaceType::Wifi | InterfaceType::Vpn | InterfaceType::Loopback
    )
}

#[cfg(not(target_os = "macos"))]
fn should_prefer_name_classification(_interface_type: InterfaceType) -> bool {
    false
}

#[cfg(any(target_os = "macos", test))]
struct MacosPrimaryServiceInterface {
    bsd_name: String,
    hardware: Option<String>,
    interface_type: Option<String>,
}

#[cfg(any(target_os = "macos", test))]
fn apply_macos_primary_service_interface_type(
    interface_details: &mut BTreeMap<String, PlatformInterfaceDetails>,
    interface: MacosPrimaryServiceInterface,
) {
    if let Some(interface_type) = macos_service_interface_type(
        interface.hardware.as_deref(),
        interface.interface_type.as_deref(),
    ) {
        interface_details
            .entry(normalize_interface_key(&interface.bsd_name))
            .or_default()
            .interface_type = Some(interface_type);
    }
}

#[cfg(any(target_os = "macos", test))]
fn macos_service_interface_type(
    hardware: Option<&str>,
    interface_type: Option<&str>,
) -> Option<InterfaceType> {
    [hardware, interface_type]
        .into_iter()
        .flatten()
        .find_map(|value| match value.to_ascii_lowercase().as_str() {
            "airport" | "ieee80211" => Some(InterfaceType::Wifi),
            "ethernet" | "bond" | "vlan" => Some(InterfaceType::Ethernet),
            "ipsec" | "l2tp" | "ppp" | "pptp" => Some(InterfaceType::Vpn),
            _ => None,
        })
}

#[cfg(target_os = "macos")]
fn read_platform_interface_details() -> BTreeMap<String, PlatformInterfaceDetails> {
    use system_configuration::network_configuration::{get_interfaces, SCNetworkInterfaceType};

    let mut interface_details = get_interfaces()
        .into_iter()
        .filter_map(|interface| {
            let name = interface.bsd_name()?.to_string();
            let interface_type = match interface.interface_type()? {
                SCNetworkInterfaceType::IEEE80211 => InterfaceType::Wifi,
                SCNetworkInterfaceType::Ethernet
                | SCNetworkInterfaceType::Bond
                | SCNetworkInterfaceType::VLAN => InterfaceType::Ethernet,
                SCNetworkInterfaceType::IPSec
                | SCNetworkInterfaceType::L2TP
                | SCNetworkInterfaceType::PPP
                | SCNetworkInterfaceType::PPTP => InterfaceType::Vpn,
                _ => return None,
            };

            Some((
                normalize_interface_key(&name),
                PlatformInterfaceDetails {
                    interface_type: Some(interface_type),
                    ..Default::default()
                },
            ))
        })
        .collect();

    if let Some(interface) = read_macos_primary_service_interface() {
        apply_macos_primary_service_interface_type(&mut interface_details, interface);
    }

    interface_details
}

#[cfg(target_os = "macos")]
fn read_macos_primary_service_interface() -> Option<MacosPrimaryServiceInterface> {
    use system_configuration::{
        core_foundation::{
            base::{TCFType, ToVoid},
            dictionary::CFDictionary,
            propertylist::CFPropertyList,
            string::{CFString, CFStringRef},
        },
        dynamic_store::SCDynamicStoreBuilder,
        sys::schema_definitions::{
            kSCDynamicStorePropNetPrimaryInterface, kSCDynamicStorePropNetPrimaryService,
            kSCPropNetInterfaceDeviceName, kSCPropNetInterfaceHardware, kSCPropNetInterfaceType,
        },
    };

    fn string_value(dictionary: &CFDictionary, key: CFStringRef) -> Option<String> {
        dictionary
            .find(key.to_void())
            .map(|ptr| unsafe { CFString::wrap_under_get_rule(*ptr as CFStringRef).to_string() })
    }

    let store = SCDynamicStoreBuilder::new("tauri-plugin-net-watcher").build()?;
    let global_ipv4 = store
        .get("State:/Network/Global/IPv4")
        .and_then(CFPropertyList::downcast_into::<CFDictionary>)?;

    let primary_interface = string_value(&global_ipv4, unsafe {
        kSCDynamicStorePropNetPrimaryInterface
    })?;
    let primary_service = string_value(&global_ipv4, unsafe {
        kSCDynamicStorePropNetPrimaryService
    })?;
    let service_interface_key = format!("Setup:/Network/Service/{primary_service}/Interface");
    let service_interface = store
        .get(service_interface_key.as_str())
        .and_then(CFPropertyList::downcast_into::<CFDictionary>)?;

    Some(MacosPrimaryServiceInterface {
        bsd_name: string_value(&service_interface, unsafe { kSCPropNetInterfaceDeviceName })
            .unwrap_or(primary_interface),
        hardware: string_value(&service_interface, unsafe { kSCPropNetInterfaceHardware }),
        interface_type: string_value(&service_interface, unsafe { kSCPropNetInterfaceType }),
    })
}

#[cfg(target_os = "windows")]
fn read_platform_interface_details() -> BTreeMap<String, PlatformInterfaceDetails> {
    let mut details = read_windows_adapter_details().unwrap_or_default();
    for (key, value) in read_windows_current_profile_interface_type().unwrap_or_default() {
        details.entry(key).or_default().merge(value);
    }

    details
}

#[cfg(target_os = "windows")]
fn read_windows_current_profile_interface_type(
) -> Option<BTreeMap<String, PlatformInterfaceDetails>> {
    use windows::Networking::Connectivity::NetworkInformation;

    let profile = NetworkInformation::GetInternetConnectionProfile().ok()?;
    let adapter = profile.NetworkAdapter().ok()?;
    let adapter_id = format!("{:?}", adapter.NetworkAdapterId().ok()?);

    if profile.IsWlanConnectionProfile().ok()? {
        return Some(platform_interface_type_overrides_from_type(
            Some(&adapter_id),
            InterfaceType::Wifi,
        ));
    }

    Some(platform_interface_type_overrides(
        Some(&adapter_id),
        adapter.IanaInterfaceType().ok(),
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn read_platform_interface_details() -> BTreeMap<String, PlatformInterfaceDetails> {
    BTreeMap::new()
}

#[cfg(any(target_os = "windows", test))]
fn platform_interface_type_overrides(
    adapter_id: Option<&str>,
    iana_interface_type: Option<u32>,
) -> BTreeMap<String, PlatformInterfaceDetails> {
    platform_interface_type_overrides_from_type(
        adapter_id,
        iana_interface_type
            .map(interface_type_from_windows_iana_type)
            .unwrap_or(InterfaceType::Unknown),
    )
}

#[cfg(any(target_os = "windows", test))]
fn platform_interface_type_overrides_from_type(
    adapter_id: Option<&str>,
    interface_type: InterfaceType,
) -> BTreeMap<String, PlatformInterfaceDetails> {
    let mut interface_details = BTreeMap::new();
    if interface_type == InterfaceType::Unknown {
        return interface_details;
    }

    if let Some(adapter_id) = adapter_id {
        interface_details.insert(
            normalize_interface_key(adapter_id),
            PlatformInterfaceDetails {
                interface_type: Some(interface_type),
                ..Default::default()
            },
        );
    }

    interface_details
}

impl PlatformInterfaceDetails {
    fn merge(&mut self, other: Self) {
        if other.interface_type.is_some() {
            self.interface_type = other.interface_type;
        }

        if other.gateway.is_some() {
            self.gateway = other.gateway;
        }

        if !other.dns_servers.is_empty() {
            self.dns_servers = other.dns_servers;
        }

        if other.mac.is_some() {
            self.mac = other.mac;
        }
    }
}

#[cfg(target_os = "windows")]
fn read_windows_adapter_details() -> crate::Result<BTreeMap<String, PlatformInterfaceDetails>> {
    use std::ffi::CStr;
    use windows_sys::Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR},
        NetworkManagement::{
            IpHelper::{GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, IP_ADAPTER_ADDRESSES_LH},
            Ndis::IfOperStatusUp,
        },
        Networking::WinSock::AF_UNSPEC,
    };

    let mut buffer_len = 15_000_u32;
    let mut buffer = vec![0_u8; buffer_len as usize];
    let mut error = unsafe {
        GetAdaptersAddresses(
            AF_UNSPEC as u32,
            GAA_FLAG_INCLUDE_GATEWAYS,
            std::ptr::null(),
            buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
            &mut buffer_len,
        )
    };

    if error == ERROR_BUFFER_OVERFLOW {
        buffer.resize(buffer_len as usize, 0);
        error = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC as u32,
                GAA_FLAG_INCLUDE_GATEWAYS,
                std::ptr::null(),
                buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &mut buffer_len,
            )
        };
    }

    if error != NO_ERROR {
        return Err(crate::Error::internal(format!(
            "failed to read Windows adapter addresses: {error}"
        )));
    }

    let mut details = BTreeMap::new();
    let mut adapter =
        buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>() as *mut IP_ADAPTER_ADDRESSES_LH;
    while !adapter.is_null() {
        let adapter_ref = unsafe { &*adapter };
        let adapter_name = unsafe {
            (!adapter_ref.AdapterName.is_null()).then(|| {
                CStr::from_ptr(adapter_ref.AdapterName.cast())
                    .to_string_lossy()
                    .into_owned()
            })
        };
        let friendly_name = unsafe { wide_ptr_to_string(adapter_ref.FriendlyName) };
        let interface_type = interface_type_from_windows_iana_type(adapter_ref.IfType);
        let gateway = first_socket_address_to_string(adapter_ref.FirstGatewayAddress);
        let dns_servers = socket_address_list_to_strings(adapter_ref.FirstDnsServerAddress);

        let mut entry = PlatformInterfaceDetails {
            interface_type: Some(interface_type),
            gateway,
            dns_servers,
            mac: None,
        };

        if adapter_ref.OperStatus != IfOperStatusUp {
            entry.gateway = None;
        }

        if let Some(name) = adapter_name {
            details.insert(normalize_interface_key(&name), entry.clone());
        }

        if let Some(name) = friendly_name {
            details.insert(normalize_interface_key(&name), entry);
        }

        adapter = adapter_ref.Next;
    }

    Ok(details)
}

#[cfg(target_os = "windows")]
unsafe fn wide_ptr_to_string(value: windows_sys::core::PWSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }

    let mut len = 0;
    while unsafe { *value.add(len) } != 0 {
        len += 1;
    }

    let slice = unsafe { std::slice::from_raw_parts(value, len) };
    Some(String::from_utf16_lossy(slice))
}

#[cfg(target_os = "windows")]
fn first_socket_address_to_string(
    address: *mut windows_sys::Win32::NetworkManagement::IpHelper::IP_ADAPTER_GATEWAY_ADDRESS_LH,
) -> Option<String> {
    if address.is_null() {
        return None;
    }

    socket_address_to_string(unsafe { (*address).Address })
}

#[cfg(target_os = "windows")]
fn socket_address_list_to_strings(
    mut address: *mut windows_sys::Win32::NetworkManagement::IpHelper::IP_ADAPTER_DNS_SERVER_ADDRESS_XP,
) -> Vec<String> {
    let mut addresses = Vec::new();

    while !address.is_null() {
        if let Some(value) = socket_address_to_string(unsafe { (*address).Address }) {
            addresses.push(value);
        }
        address = unsafe { (*address).Next };
    }

    addresses.sort();
    addresses.dedup();
    addresses
}

#[cfg(target_os = "windows")]
fn socket_address_to_string(
    address: windows_sys::Win32::Networking::WinSock::SOCKET_ADDRESS,
) -> Option<String> {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR_IN, SOCKADDR_IN6};

    if address.lpSockaddr.is_null() {
        return None;
    }

    let family = unsafe { (*address.lpSockaddr).sa_family };
    match family {
        AF_INET if address.iSockaddrLength as usize >= std::mem::size_of::<SOCKADDR_IN>() => {
            let socket = unsafe { &*(address.lpSockaddr.cast::<SOCKADDR_IN>()) };
            let octets = unsafe { socket.sin_addr.S_un.S_un_b };
            Some(Ipv4Addr::new(octets.s_b1, octets.s_b2, octets.s_b3, octets.s_b4).to_string())
        }
        AF_INET6 if address.iSockaddrLength as usize >= std::mem::size_of::<SOCKADDR_IN6>() => {
            let socket = unsafe { &*(address.lpSockaddr.cast::<SOCKADDR_IN6>()) };
            let octets = unsafe { socket.sin6_addr.u.Byte };
            Some(Ipv6Addr::from(octets).to_string())
        }
        _ => None,
    }
}

#[cfg(any(target_os = "windows", test))]
fn interface_type_from_windows_iana_type(interface_type: u32) -> InterfaceType {
    match interface_type {
        6 | 135 => InterfaceType::Ethernet,
        71 => InterfaceType::Wifi,
        _ => InterfaceType::Unknown,
    }
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
        || normalized == "en1"
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
    fn platform_interface_types_classify_unknown_names() {
        let mut interface_types = BTreeMap::new();
        interface_types.insert(
            "port-a".to_string(),
            PlatformInterfaceDetails {
                interface_type: Some(InterfaceType::Ethernet),
                ..Default::default()
            },
        );
        interface_types.insert(
            "port-b".to_string(),
            PlatformInterfaceDetails {
                interface_type: Some(InterfaceType::Wifi),
                ..Default::default()
            },
        );

        let interfaces = build_interfaces_from_entries_with_platform_details(
            vec![
                (
                    "port-a".to_string(),
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
                ),
                (
                    "port-b".to_string(),
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 11)),
                ),
            ],
            &interface_types,
        );

        assert_eq!(interfaces[0].name, "port-a");
        assert_eq!(interfaces[0].interface_type, InterfaceType::Ethernet);
        assert_eq!(interfaces[1].name, "port-b");
        assert_eq!(interfaces[1].interface_type, InterfaceType::Wifi);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_en1_with_ip_is_wifi_like_logger_enrichment() {
        let mut interface_types = BTreeMap::new();
        interface_types.insert(
            "en1".to_string(),
            PlatformInterfaceDetails {
                interface_type: Some(InterfaceType::Ethernet),
                ..Default::default()
            },
        );

        let interfaces = build_interfaces_from_entries_with_platform_details(
            vec![(
                "en1".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 11)),
            )],
            &interface_types,
        );

        assert_eq!(interfaces[0].interface_type, InterfaceType::Wifi);
    }

    #[test]
    fn platform_interface_details_enrich_gateway_and_dns() {
        let mut interface_details = BTreeMap::new();
        interface_details.insert(
            "wlan".to_string(),
            PlatformInterfaceDetails {
                interface_type: Some(InterfaceType::Wifi),
                gateway: Some("192.168.1.1".to_string()),
                dns_servers: vec!["1.1.1.1".to_string(), "8.8.8.8".to_string()],
                ..Default::default()
            },
        );

        let interfaces = build_interfaces_from_entries_with_platform_details(
            vec![(
                "WLAN".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            )],
            &interface_details,
        );

        assert_eq!(interfaces[0].interface_type, InterfaceType::Wifi);
        assert_eq!(interfaces[0].gateway.as_deref(), Some("192.168.1.1"));
        assert_eq!(interfaces[0].dns_servers, vec!["1.1.1.1", "8.8.8.8"]);
        assert_eq!(interfaces[0].addresses.mac, None);
    }

    #[test]
    fn unknown_platform_interface_type_does_not_mask_name_classification() {
        let mut interface_details = BTreeMap::new();
        interface_details.insert(
            "wlan".to_string(),
            PlatformInterfaceDetails {
                interface_type: Some(InterfaceType::Unknown),
                gateway: Some("192.168.1.1".to_string()),
                ..Default::default()
            },
        );

        let interfaces = build_interfaces_from_entries_with_platform_details(
            vec![(
                "WLAN".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            )],
            &interface_details,
        );

        assert_eq!(interfaces[0].interface_type, InterfaceType::Wifi);
        assert_eq!(interfaces[0].gateway.as_deref(), Some("192.168.1.1"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_socket_address_to_string_reads_ipv4() {
        use windows_sys::Win32::Networking::WinSock::{
            AF_INET, IN_ADDR_0_0, SOCKADDR_IN, SOCKET_ADDRESS,
        };

        let mut socket = SOCKADDR_IN {
            sin_family: AF_INET,
            ..Default::default()
        };
        socket.sin_addr.S_un.S_un_b = IN_ADDR_0_0 {
            s_b1: 192,
            s_b2: 168,
            s_b3: 1,
            s_b4: 1,
        };
        let address = SOCKET_ADDRESS {
            lpSockaddr: (&mut socket as *mut SOCKADDR_IN).cast(),
            iSockaddrLength: std::mem::size_of::<SOCKADDR_IN>() as i32,
        };

        assert_eq!(
            socket_address_to_string(address).as_deref(),
            Some("192.168.1.1")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_socket_address_to_string_reads_ipv6() {
        use windows_sys::Win32::Networking::WinSock::{
            AF_INET6, IN6_ADDR_0, SOCKADDR_IN6, SOCKET_ADDRESS,
        };

        let mut socket = SOCKADDR_IN6 {
            sin6_family: AF_INET6,
            ..Default::default()
        };
        socket.sin6_addr.u = IN6_ADDR_0 {
            Byte: [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        };
        let address = SOCKET_ADDRESS {
            lpSockaddr: (&mut socket as *mut SOCKADDR_IN6).cast(),
            iSockaddrLength: std::mem::size_of::<SOCKADDR_IN6>() as i32,
        };

        assert_eq!(
            socket_address_to_string(address).as_deref(),
            Some("2001:db8::1")
        );
    }

    #[test]
    fn macos_primary_service_type_overrides_generic_en_name() {
        let mut interface_details = BTreeMap::new();
        interface_details.insert(
            "en5".to_string(),
            PlatformInterfaceDetails {
                interface_type: Some(InterfaceType::Ethernet),
                ..Default::default()
            },
        );

        apply_macos_primary_service_interface_type(
            &mut interface_details,
            MacosPrimaryServiceInterface {
                bsd_name: "en5".to_string(),
                hardware: Some("AirPort".to_string()),
                interface_type: Some("IEEE80211".to_string()),
            },
        );

        let interfaces = build_interfaces_from_entries_with_platform_details(
            vec![(
                "en5".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 12)),
            )],
            &interface_details,
        );

        assert_eq!(interfaces[0].interface_type, InterfaceType::Wifi);
    }

    #[test]
    fn platform_interface_type_overrides_match_adapter_guids_case_insensitively() {
        let interface_types = platform_interface_type_overrides(
            Some("{ABCDEF00-1111-2222-3333-444444444444}"),
            Some(71),
        );

        let interfaces = build_interfaces_from_entries_with_platform_details(
            vec![
                (
                    "abcdef00-1111-2222-3333-444444444444".to_string(),
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
                ),
                (
                    "other-adapter".to_string(),
                    IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                ),
            ],
            &interface_types,
        );

        assert_eq!(interfaces[0].interface_type, InterfaceType::Wifi);
        assert_ne!(interfaces[1].interface_type, InterfaceType::Wifi);
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
