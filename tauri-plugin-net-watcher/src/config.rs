use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use url::Url;

const MIN_INTERVAL_MS: u64 = 1_000;
const MAX_INTERVAL_MS: u64 = 3_600_000;
const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetWatcherConfig {
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub targets: Vec<ReachabilityTargetConfig>,
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachabilityTargetConfig {
    pub id: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartWatchingOptions {
    pub targets: Option<Vec<ReachabilityTargetConfig>>,
    pub interval_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
}

impl Default for NetWatcherConfig {
    fn default() -> Self {
        Self {
            auto_start: false,
            targets: Vec::new(),
            interval_ms: default_interval_ms(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

impl NetWatcherConfig {
    pub fn with_runtime_options(mut self, options: StartWatchingOptions) -> Self {
        if let Some(targets) = options.targets {
            self.targets = targets;
        }

        if let Some(interval_ms) = options.interval_ms {
            self.interval_ms = interval_ms;
        }

        if let Some(timeout_ms) = options.timeout_ms {
            self.timeout_ms = timeout_ms;
        }

        self
    }

    pub fn validate(&self) -> crate::Result<()> {
        let mut target_ids = BTreeSet::new();
        for target in &self.targets {
            if target.id.trim().is_empty() {
                return Err(crate::Error::invalid_config(
                    "net watcher target id must not be empty",
                ));
            }

            if !target_ids.insert(target.id.clone()) {
                return Err(crate::Error::invalid_config(format!(
                    "net watcher target id is duplicated: {}",
                    target.id
                )));
            }

            validate_target_url(&target.url)?;
        }

        if !(MIN_INTERVAL_MS..=MAX_INTERVAL_MS).contains(&self.interval_ms) {
            return Err(crate::Error::invalid_config(
                "net watcher interval_ms must be between 1000 and 3600000",
            ));
        }

        if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&self.timeout_ms) {
            return Err(crate::Error::invalid_config(
                "net watcher timeout_ms must be between 100 and 60000",
            ));
        }

        if self.timeout_ms > self.interval_ms {
            return Err(crate::Error::invalid_config(
                "net watcher timeout_ms must not be greater than interval_ms",
            ));
        }

        Ok(())
    }

    pub(crate) fn window_size(&self) -> usize {
        default_window_size()
    }

    pub(crate) fn degraded_failure_rate(&self) -> f64 {
        default_degraded_failure_rate()
    }

    pub(crate) fn degraded_p95_latency_ms(&self) -> u64 {
        default_degraded_p95_latency_ms()
    }

    pub(crate) fn include_mac_address(&self) -> bool {
        false
    }
}

fn validate_target_url(value: &str) -> crate::Result<()> {
    let target = Url::parse(value).map_err(|error| {
        crate::Error::invalid_config(format!("net watcher target is invalid: {error}"))
    })?;

    if target.scheme() != "http" && target.scheme() != "https" {
        return Err(crate::Error::invalid_config(
            "net watcher target must use http or https",
        ));
    }

    if target.host().is_none() {
        return Err(crate::Error::invalid_config(
            "net watcher target host is required",
        ));
    }

    Ok(())
}

fn default_interval_ms() -> u64 {
    10_000
}

fn default_timeout_ms() -> u64 {
    3_000
}

fn default_window_size() -> usize {
    20
}

fn default_degraded_failure_rate() -> f64 {
    0.15
}

fn default_degraded_p95_latency_ms() -> u64 {
    800
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_config_uses_core_defaults() {
        let config = NetWatcherConfig::default();

        assert!(!config.auto_start);
        assert!(config.targets.is_empty());
        assert_eq!(config.interval_ms, 10_000);
        assert_eq!(config.timeout_ms, 3_000);
    }

    #[test]
    fn config_serializes_only_core_public_fields() {
        let value = serde_json::to_value(NetWatcherConfig::default()).unwrap();

        assert_eq!(
            value,
            json!({
                "autoStart": false,
                "targets": [],
                "intervalMs": 10000,
                "timeoutMs": 3000
            })
        );
    }

    #[test]
    fn internal_fields_are_rejected_from_public_config() {
        let value = json!({
            "targets": [{ "id": "api", "url": "https://example.com/health" }],
            "windowSize": 3
        });

        let error = serde_json::from_value::<NetWatcherConfig>(value).unwrap_err();

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn legacy_target_field_is_rejected() {
        let config_error = serde_json::from_value::<NetWatcherConfig>(json!({
            "target": "https://example.com/health"
        }))
        .unwrap_err();
        let options_error = serde_json::from_value::<StartWatchingOptions>(json!({
            "target": "https://example.com/health"
        }))
        .unwrap_err();

        assert!(config_error.to_string().contains("unknown field"));
        assert!(options_error.to_string().contains("unknown field"));
    }

    #[test]
    fn runtime_options_override_session_values_only() {
        let base = NetWatcherConfig::default();
        let options = StartWatchingOptions {
            targets: Some(vec![ReachabilityTargetConfig {
                id: "api".to_string(),
                url: "https://api.example.com/health".to_string(),
            }]),
            interval_ms: Some(5_000),
            timeout_ms: Some(1_500),
        };

        let merged = base.with_runtime_options(options);

        assert_eq!(merged.targets.len(), 1);
        assert_eq!(merged.targets[0].id, "api");
        assert_eq!(merged.targets[0].url, "https://api.example.com/health");
        assert_eq!(merged.interval_ms, 5_000);
        assert_eq!(merged.timeout_ms, 1_500);
    }

    #[test]
    fn accepts_empty_targets() {
        let config = NetWatcherConfig {
            targets: Vec::new(),
            ..Default::default()
        };

        config.validate().unwrap();
    }

    #[test]
    fn rejects_duplicate_target_ids() {
        let config = NetWatcherConfig {
            targets: vec![
                ReachabilityTargetConfig {
                    id: "api".to_string(),
                    url: "https://api.example.com/health".to_string(),
                },
                ReachabilityTargetConfig {
                    id: "api".to_string(),
                    url: "https://api-backup.example.com/health".to_string(),
                },
            ],
            ..Default::default()
        };

        assert_eq!(config.validate().unwrap_err().code(), "invalid_config");
    }

    #[test]
    fn invalid_runtime_values_are_rejected() {
        let options = StartWatchingOptions {
            targets: Some(vec![ReachabilityTargetConfig {
                id: "api".to_string(),
                url: "file:///tmp/health".to_string(),
            }]),
            interval_ms: Some(0),
            timeout_ms: Some(0),
        };

        let error = NetWatcherConfig::default()
            .with_runtime_options(options)
            .validate()
            .unwrap_err();

        assert_eq!(error.code(), "invalid_config");
    }

    #[test]
    fn rejects_malformed_target_url() {
        let config = NetWatcherConfig {
            targets: vec![ReachabilityTargetConfig {
                id: "api".to_string(),
                url: "https://".to_string(),
            }],
            ..Default::default()
        };

        let error = config.validate().unwrap_err();

        assert_eq!(error.code(), "invalid_config");
    }

    #[test]
    fn rejects_interval_values_outside_production_bounds() {
        let too_fast = NetWatcherConfig {
            interval_ms: 999,
            ..Default::default()
        };
        let too_slow = NetWatcherConfig {
            interval_ms: 3_600_001,
            ..Default::default()
        };

        assert_eq!(too_fast.validate().unwrap_err().code(), "invalid_config");
        assert_eq!(too_slow.validate().unwrap_err().code(), "invalid_config");
    }

    #[test]
    fn rejects_timeout_values_outside_production_bounds() {
        let too_short = NetWatcherConfig {
            timeout_ms: 99,
            ..Default::default()
        };
        let too_long = NetWatcherConfig {
            timeout_ms: 60_001,
            ..Default::default()
        };
        let longer_than_interval = NetWatcherConfig {
            interval_ms: 1_000,
            timeout_ms: 1_001,
            ..Default::default()
        };

        assert_eq!(too_short.validate().unwrap_err().code(), "invalid_config");
        assert_eq!(too_long.validate().unwrap_err().code(), "invalid_config");
        assert_eq!(
            longer_than_interval.validate().unwrap_err().code(),
            "invalid_config"
        );
    }

    #[test]
    fn internal_defaults_are_available_to_runtime_only() {
        let config = NetWatcherConfig::default();

        assert_eq!(config.window_size(), 20);
        assert_eq!(config.degraded_failure_rate(), 0.15);
        assert_eq!(config.degraded_p95_latency_ms(), 800);
        assert!(!config.include_mac_address());
    }
}
