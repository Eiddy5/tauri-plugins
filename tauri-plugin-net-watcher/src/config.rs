use serde::{Deserialize, Serialize};

const DEFAULT_TARGET: &str = "https://www.apple.com/library/test/success.html";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetWatcherConfig {
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_window_size")]
    pub window_size: usize,
    #[serde(default = "default_degraded_failure_rate")]
    pub degraded_failure_rate: f64,
    #[serde(default = "default_degraded_p95_latency_ms")]
    pub degraded_p95_latency_ms: u64,
    #[serde(default = "default_offline_consecutive_failures")]
    pub offline_consecutive_failures: usize,
    #[serde(default)]
    pub include_mac_address: bool,
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
            window_size: default_window_size(),
            degraded_failure_rate: default_degraded_failure_rate(),
            degraded_p95_latency_ms: default_degraded_p95_latency_ms(),
            offline_consecutive_failures: default_offline_consecutive_failures(),
            include_mac_address: false,
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

        if self.window_size == 0 {
            return Err(crate::Error::invalid_config(
                "net watcher window_size must be greater than zero",
            ));
        }

        if !self.degraded_failure_rate.is_finite()
            || self.degraded_failure_rate <= 0.0
            || self.degraded_failure_rate > 1.0
        {
            return Err(crate::Error::invalid_config(
                "net watcher degraded_failure_rate must be finite, greater than zero, and at most one",
            ));
        }

        if self.degraded_p95_latency_ms == 0 {
            return Err(crate::Error::invalid_config(
                "net watcher degraded_p95_latency_ms must be greater than zero",
            ));
        }

        if self.offline_consecutive_failures == 0 {
            return Err(crate::Error::invalid_config(
                "net watcher offline_consecutive_failures must be greater than zero",
            ));
        }

        Ok(())
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
        assert_eq!(config.window_size, 20);
        assert_eq!(config.degraded_failure_rate, 0.15);
        assert_eq!(config.degraded_p95_latency_ms, 800);
        assert_eq!(config.offline_consecutive_failures, 3);
        assert!(!config.include_mac_address);
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
        assert_eq!(merged.window_size, 20);
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
    fn invalid_state_thresholds_are_rejected() {
        let invalid_configs = [
            NetWatcherConfig {
                window_size: 0,
                ..Default::default()
            },
            NetWatcherConfig {
                degraded_failure_rate: 0.0,
                ..Default::default()
            },
            NetWatcherConfig {
                degraded_failure_rate: 1.1,
                ..Default::default()
            },
            NetWatcherConfig {
                degraded_failure_rate: f64::NAN,
                ..Default::default()
            },
            NetWatcherConfig {
                degraded_failure_rate: f64::INFINITY,
                ..Default::default()
            },
            NetWatcherConfig {
                degraded_p95_latency_ms: 0,
                ..Default::default()
            },
            NetWatcherConfig {
                offline_consecutive_failures: 0,
                ..Default::default()
            },
        ];

        for config in invalid_configs {
            let error = config.validate().unwrap_err();

            assert_eq!(error.code(), "invalid_config");
        }
    }
}
