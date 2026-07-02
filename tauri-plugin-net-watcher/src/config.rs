use serde::{Deserialize, Serialize};

const DEFAULT_TARGET: &str = "https://www.apple.com/library/test/success.html";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetWatcherConfig {
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartWatchingOptions {
    pub target: Option<String>,
    pub interval_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
}

impl Default for NetWatcherConfig {
    fn default() -> Self {
        Self {
            auto_start: false,
            target: default_target(),
            interval_ms: default_interval_ms(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

impl NetWatcherConfig {
    pub fn with_runtime_options(mut self, options: StartWatchingOptions) -> Self {
        if let Some(target) = options.target {
            self.target = target;
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
        if !self.target.starts_with("http://") && !self.target.starts_with("https://") {
            return Err(crate::Error::invalid_config(
                "net watcher target must use http or https",
            ));
        }

        if self.interval_ms == 0 {
            return Err(crate::Error::invalid_config(
                "net watcher interval_ms must be greater than zero",
            ));
        }

        if self.timeout_ms == 0 {
            return Err(crate::Error::invalid_config(
                "net watcher timeout_ms must be greater than zero",
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

    pub(crate) fn offline_consecutive_failures(&self) -> usize {
        default_offline_consecutive_failures()
    }

    pub(crate) fn include_mac_address(&self) -> bool {
        false
    }
}

fn default_target() -> String {
    DEFAULT_TARGET.to_string()
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

fn default_offline_consecutive_failures() -> usize {
    3
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_config_uses_core_defaults() {
        let config = NetWatcherConfig::default();

        assert!(!config.auto_start);
        assert_eq!(
            config.target,
            "https://www.apple.com/library/test/success.html"
        );
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
                "target": "https://www.apple.com/library/test/success.html",
                "intervalMs": 10000,
                "timeoutMs": 3000
            })
        );
    }

    #[test]
    fn internal_fields_are_rejected_from_public_config() {
        let value = json!({
            "target": "https://example.com/health",
            "windowSize": 3
        });

        let error = serde_json::from_value::<NetWatcherConfig>(value).unwrap_err();

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn runtime_options_override_session_values_only() {
        let base = NetWatcherConfig::default();
        let options = StartWatchingOptions {
            target: Some("https://api.example.com/health".to_string()),
            interval_ms: Some(5_000),
            timeout_ms: Some(1_500),
        };

        let merged = base.with_runtime_options(options);

        assert_eq!(merged.target, "https://api.example.com/health");
        assert_eq!(merged.interval_ms, 5_000);
        assert_eq!(merged.timeout_ms, 1_500);
    }

    #[test]
    fn invalid_runtime_values_are_rejected() {
        let options = StartWatchingOptions {
            target: Some("file:///tmp/health".to_string()),
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
    fn internal_defaults_are_available_to_runtime_only() {
        let config = NetWatcherConfig::default();

        assert_eq!(config.window_size(), 20);
        assert_eq!(config.degraded_failure_rate(), 0.15);
        assert_eq!(config.degraded_p95_latency_ms(), 800);
        assert_eq!(config.offline_consecutive_failures(), 3);
        assert!(!config.include_mac_address());
    }
}
