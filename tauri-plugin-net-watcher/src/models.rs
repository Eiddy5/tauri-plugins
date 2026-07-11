use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::NetWatcherConfig;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OverallState {
    Unknown,
    Offline,
    Online,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NetworkLayerState {
    Unknown,
    Disconnected,
    Connected,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TargetQualityState {
    Unknown,
    Unstable,
    Stable,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetWatcherSnapshot {
    pub meta: SnapshotMeta,
    pub state: SnapshotState,
    pub network: NetworkSnapshot,
    pub reachability: ReachabilitySnapshot,
    pub changes: SnapshotChanges,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkUpdatedPayload {
    pub snapshot_id: String,
    pub timestamp: DateTime<Utc>,
    pub platform: String,
    pub state: SnapshotState,
    pub network: NetworkSnapshot,
}

impl NetworkUpdatedPayload {
    pub(crate) fn from_snapshot(snapshot: &NetWatcherSnapshot) -> Self {
        Self {
            snapshot_id: snapshot.meta.snapshot_id.clone(),
            timestamp: snapshot.meta.timestamp,
            platform: snapshot.meta.platform.clone(),
            state: snapshot.state.clone(),
            network: snapshot.network.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TargetUpdatedPayload {
    pub snapshot_id: String,
    pub timestamp: DateTime<Utc>,
    pub target: ReachabilityTargetSnapshot,
}

impl TargetUpdatedPayload {
    pub(crate) fn from_snapshot(
        snapshot: &NetWatcherSnapshot,
        target: &ReachabilityTargetSnapshot,
    ) -> Self {
        Self {
            snapshot_id: snapshot.meta.snapshot_id.clone(),
            timestamp: snapshot.meta.timestamp,
            target: target.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
    pub snapshot_id: String,
    pub timestamp: DateTime<Utc>,
    pub platform: String,
    pub plugin_version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotState {
    pub overall: OverallState,
    pub network: NetworkLayerState,
    pub internet: InternetStatus,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSnapshot {
    pub primary_interface_id: Option<String>,
    pub interfaces: Vec<NetworkInterface>,
    pub internet: InternetSnapshot,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InternetSnapshot {
    pub status: InternetStatus,
    pub verified: bool,
    pub system_hint: InternetSystemHint,
    pub active_probe: Option<InternetProbeResult>,
    pub captive_portal: bool,
    pub checked_at: Option<DateTime<Utc>>,
    pub consecutive_failures: usize,
    pub reason: String,
}

impl Default for InternetSnapshot {
    fn default() -> Self {
        Self {
            status: InternetStatus::Unknown,
            verified: false,
            system_hint: InternetSystemHint::default(),
            active_probe: None,
            captive_portal: false,
            checked_at: None,
            consecutive_failures: 0,
            reason: "internet_state_unknown".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InternetStatus {
    Unknown,
    Available,
    Degraded,
    Unavailable,
    CaptivePortal,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InternetSystemHint {
    pub source: InternetHintSource,
    pub level: InternetHintLevel,
}

impl Default for InternetSystemHint {
    fn default() -> Self {
        Self {
            source: InternetHintSource::Unavailable,
            level: InternetHintLevel::Unknown,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InternetHintSource {
    WindowsNcsi,
    MacosReachability,
    Unavailable,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InternetHintLevel {
    Unknown,
    None,
    LocalAccess,
    ConstrainedInternetAccess,
    InternetAccess,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InternetProbeResult {
    pub status: InternetProbeStatus,
    pub duration_ms: u64,
    pub http_status: Option<u16>,
    pub error: Option<ProbeError>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InternetProbeStatus {
    Success,
    Failed,
    UnexpectedResponse,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInterface {
    pub id: String,
    pub name: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub interface_type: InterfaceType,
    pub status: InterfaceStatus,
    pub is_primary: bool,
    pub addresses: InterfaceAddresses,
    pub gateway: Option<String>,
    pub dns_servers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InterfaceType {
    Wifi,
    Ethernet,
    Vpn,
    Loopback,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InterfaceStatus {
    Up,
    Down,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceAddresses {
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub mac: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReachabilitySnapshot {
    pub config: ReachabilityConfigSnapshot,
    pub targets: Vec<ReachabilityTargetSnapshot>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReachabilityTargetSnapshot {
    pub id: String,
    pub state: ReachabilityTargetState,
    pub target: ProbeTarget,
    pub current_probe: Option<ProbeResult>,
    pub summary: QualitySummary,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReachabilityTargetState {
    pub reachability: ReachabilityStatus,
    pub quality: TargetQualityState,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReachabilityStatus {
    Unknown,
    Reachable,
    Unreachable,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReachabilityConfigSnapshot {
    pub interval_ms: u64,
    pub window_size: usize,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProbeTarget {
    #[serde(rename = "type")]
    pub target_type: ProbeTargetType,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProbeTargetType {
    Http,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub id: String,
    pub status: ProbeStatus,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub phases: ProbePhases,
    pub http: Option<HttpProbeResult>,
    pub error: Option<ProbeError>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProbeStatus {
    Success,
    Failed,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProbePhases {
    pub dns_ms: Option<u64>,
    pub tcp_ms: Option<u64>,
    pub tls_ms: Option<u64>,
    pub http_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HttpProbeResult {
    pub status_code: u16,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProbeError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QualitySummary {
    pub sample_count: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub failure_rate: f64,
    pub latency_ms: LatencySummary,
    pub jitter_ms: u64,
    pub consecutive_failures: usize,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_failure_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LatencySummary {
    pub avg: u64,
    pub min: u64,
    pub max: u64,
    pub p95: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotChanges {
    pub has_changes: bool,
    pub previous_overall: Option<OverallState>,
    pub current_overall: OverallState,
    pub changed_fields: Vec<String>,
    pub changed_target_ids: Vec<String>,
}

impl NetWatcherSnapshot {
    pub fn initial(plugin_version: &str) -> Self {
        Self::initial_with_config(plugin_version, &NetWatcherConfig::default())
    }

    pub fn initial_with_config(plugin_version: &str, config: &NetWatcherConfig) -> Self {
        let overall = OverallState::Unknown;

        Self {
            meta: SnapshotMeta {
                snapshot_id: format!("nw_{}", Uuid::new_v4()),
                timestamp: Utc::now(),
                platform: std::env::consts::OS.to_string(),
                plugin_version: plugin_version.to_string(),
            },
            state: SnapshotState {
                overall: overall.clone(),
                network: NetworkLayerState::Unknown,
                internet: InternetStatus::Unknown,
                reason: "network_state_unknown".to_string(),
            },
            network: NetworkSnapshot::default(),
            reachability: ReachabilitySnapshot {
                config: ReachabilityConfigSnapshot {
                    interval_ms: config.interval_ms,
                    window_size: config.window_size(),
                    timeout_ms: config.timeout_ms,
                },
                targets: config
                    .targets
                    .clone()
                    .into_iter()
                    .map(|target| ReachabilityTargetSnapshot {
                        id: target.id,
                        state: ReachabilityTargetState {
                            reachability: ReachabilityStatus::Unknown,
                            quality: TargetQualityState::Unknown,
                            reason: "insufficient_data".to_string(),
                        },
                        target: ProbeTarget {
                            target_type: ProbeTargetType::Http,
                            url: target.url,
                        },
                        current_probe: None,
                        summary: QualitySummary::default(),
                    })
                    .collect(),
            },
            changes: SnapshotChanges {
                has_changes: false,
                previous_overall: None,
                current_overall: overall,
                changed_fields: Vec::new(),
                changed_target_ids: Vec::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_snapshot_serializes_with_expected_state() {
        let snapshot = NetWatcherSnapshot::initial("0.1.0");
        let value = serde_json::to_value(snapshot).unwrap();

        assert_eq!(value["state"]["overall"], "unknown");
        assert_eq!(value["state"]["network"], "unknown");
        assert_eq!(value["state"]["internet"], "unknown");
        assert_eq!(value["network"]["internet"]["status"], "unknown");
        assert!(value["state"].get("quality").is_none());
        assert!(value["state"].get("score").is_none());
        assert_eq!(value["reachability"]["targets"], serde_json::json!([]));
        assert_eq!(value["reachability"]["config"]["intervalMs"], 10_000);
        assert!(value.get("quality").is_none());
    }

    #[test]
    fn initial_snapshot_reflects_config() {
        let config = crate::NetWatcherConfig {
            targets: vec![crate::ReachabilityTargetConfig {
                id: "api".to_string(),
                url: "https://example.com/health".to_string(),
            }],
            interval_ms: 5_000,
            timeout_ms: 1_500,
            ..Default::default()
        };

        let snapshot = NetWatcherSnapshot::initial_with_config("0.1.0", &config);

        assert_eq!(snapshot.reachability.targets[0].id, "api");
        assert_eq!(
            snapshot.reachability.targets[0].target.url,
            "https://example.com/health"
        );
        assert_eq!(snapshot.reachability.config.interval_ms, 5_000);
        assert_eq!(snapshot.reachability.config.timeout_ms, 1_500);
        assert_eq!(snapshot.reachability.config.window_size, 20);
    }

    #[test]
    fn initial_snapshot_reflects_multiple_reachability_targets() {
        let config = crate::NetWatcherConfig {
            targets: vec![
                crate::ReachabilityTargetConfig {
                    id: "api".to_string(),
                    url: "https://api.example.com/health".to_string(),
                },
                crate::ReachabilityTargetConfig {
                    id: "cdn".to_string(),
                    url: "https://cdn.example.com/ping".to_string(),
                },
            ],
            ..Default::default()
        };

        let snapshot = NetWatcherSnapshot::initial_with_config("0.1.0", &config);

        assert_eq!(snapshot.reachability.targets.len(), 2);
        assert_eq!(snapshot.reachability.targets[0].id, "api");
        assert_eq!(
            snapshot.reachability.targets[0].target.url,
            "https://api.example.com/health"
        );
        assert_eq!(snapshot.reachability.targets[1].id, "cdn");
        assert_eq!(
            snapshot.reachability.targets[1].target.url,
            "https://cdn.example.com/ping"
        );
        assert_eq!(snapshot.reachability.targets[0].summary.sample_count, 0);
        assert_eq!(snapshot.reachability.targets[1].summary.sample_count, 0);
    }

    #[test]
    fn network_update_payload_contains_only_network_panel_data() {
        let snapshot = NetWatcherSnapshot::initial("0.1.0");
        let value = serde_json::to_value(NetworkUpdatedPayload::from_snapshot(&snapshot)).unwrap();

        assert!(value.get("snapshotId").is_some());
        assert!(value.get("timestamp").is_some());
        assert!(value.get("platform").is_some());
        assert!(value.get("state").is_some());
        assert!(value.get("network").is_some());
        assert!(value.get("meta").is_none());
        assert!(value.get("reachability").is_none());
        assert!(value.get("changes").is_none());
    }

    #[test]
    fn target_update_payload_contains_one_target_only() {
        let config = crate::NetWatcherConfig {
            targets: vec![
                crate::ReachabilityTargetConfig {
                    id: "api".to_string(),
                    url: "https://api.example.com/health".to_string(),
                },
                crate::ReachabilityTargetConfig {
                    id: "cdn".to_string(),
                    url: "https://cdn.example.com/ping".to_string(),
                },
            ],
            ..Default::default()
        };
        let snapshot = NetWatcherSnapshot::initial_with_config("0.1.0", &config);
        let value = serde_json::to_value(TargetUpdatedPayload::from_snapshot(
            &snapshot,
            &snapshot.reachability.targets[0],
        ))
        .unwrap();

        assert_eq!(value["target"]["id"], "api");
        assert!(value.get("network").is_none());
        assert!(value.get("state").is_none());
        assert!(value.get("reachability").is_none());
        assert!(value["target"].get("targets").is_none());
    }
}
