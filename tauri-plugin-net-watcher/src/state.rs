use crate::models::{
    NetworkLayerState, OverallState, QualityLayerState, QualitySummary, SnapshotState,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StateConfig {
    pub degraded_failure_rate: f64,
    pub degraded_p95_latency_ms: u64,
    pub offline_consecutive_failures: usize,
}

pub fn evaluate_state(
    has_available_interface: bool,
    summary: &QualitySummary,
    config: &StateConfig,
) -> SnapshotState {
    let config = normalize_config(config);

    if !has_available_interface {
        return SnapshotState {
            overall: OverallState::Offline,
            network: NetworkLayerState::Disconnected,
            quality: QualityLayerState::Unknown,
            score: 0,
            reason: "no_available_interface".to_string(),
        };
    }

    if summary.sample_count == 0 {
        return SnapshotState {
            overall: OverallState::Unknown,
            network: NetworkLayerState::Connected,
            quality: QualityLayerState::Unknown,
            score: 0,
            reason: "insufficient_data".to_string(),
        };
    }

    if summary.consecutive_failures >= config.offline_consecutive_failures {
        return SnapshotState {
            overall: OverallState::LocalOnly,
            network: NetworkLayerState::Connected,
            quality: QualityLayerState::Unreachable,
            score: 10,
            reason: "target_unreachable".to_string(),
        };
    }

    let score = calculate_score(summary);
    let is_degraded = summary.failure_rate >= config.degraded_failure_rate
        || summary.latency_ms.p95 >= config.degraded_p95_latency_ms
        || summary.jitter_ms >= 300;

    if is_degraded {
        SnapshotState {
            overall: OverallState::Degraded,
            network: NetworkLayerState::Connected,
            quality: QualityLayerState::Unstable,
            score,
            reason: "high_latency_or_recent_failures".to_string(),
        }
    } else {
        SnapshotState {
            overall: OverallState::Online,
            network: NetworkLayerState::Connected,
            quality: QualityLayerState::Stable,
            score,
            reason: "network_stable".to_string(),
        }
    }
}

fn normalize_config(config: &StateConfig) -> StateConfig {
    StateConfig {
        degraded_failure_rate: if config.degraded_failure_rate <= 0.0 {
            0.15
        } else {
            config.degraded_failure_rate.min(1.0)
        },
        degraded_p95_latency_ms: if config.degraded_p95_latency_ms == 0 {
            800
        } else {
            config.degraded_p95_latency_ms
        },
        offline_consecutive_failures: config.offline_consecutive_failures.max(1),
    }
}

fn calculate_score(summary: &QualitySummary) -> u8 {
    let failure_penalty = (summary.failure_rate.clamp(0.0, 1.0) * 60.0).round() as i32;
    let latency_penalty = (summary.latency_ms.p95 / 100).min(30) as i32;
    let jitter_penalty = (summary.jitter_ms / 25).min(20) as i32;
    let consecutive_penalty = summary.consecutive_failures.saturating_mul(10).min(30) as i32;

    (100 - failure_penalty - latency_penalty - jitter_penalty - consecutive_penalty).clamp(0, 100)
        as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    fn config() -> StateConfig {
        StateConfig {
            degraded_failure_rate: 0.15,
            degraded_p95_latency_ms: 800,
            offline_consecutive_failures: 3,
        }
    }

    #[test]
    fn no_interface_is_offline() {
        let state = evaluate_state(false, &QualitySummary::default(), &config());

        assert_eq!(state.overall, OverallState::Offline);
        assert_eq!(state.network, NetworkLayerState::Disconnected);
        assert_eq!(state.reason, "no_available_interface");
    }

    #[test]
    fn consecutive_failures_are_local_only() {
        let summary = QualitySummary {
            sample_count: 3,
            failure_count: 3,
            failure_rate: 1.0,
            consecutive_failures: 3,
            ..Default::default()
        };

        let state = evaluate_state(true, &summary, &config());

        assert_eq!(state.overall, OverallState::LocalOnly);
        assert_eq!(state.quality, QualityLayerState::Unreachable);
    }

    #[test]
    fn high_latency_is_degraded() {
        let summary = QualitySummary {
            sample_count: 20,
            success_count: 20,
            latency_ms: LatencySummary {
                avg: 400,
                min: 100,
                max: 1200,
                p95: 900,
            },
            ..Default::default()
        };

        let state = evaluate_state(true, &summary, &config());

        assert_eq!(state.overall, OverallState::Degraded);
        assert_eq!(state.quality, QualityLayerState::Unstable);
    }

    #[test]
    fn stable_summary_is_online() {
        let summary = QualitySummary {
            sample_count: 20,
            success_count: 20,
            latency_ms: LatencySummary {
                avg: 100,
                min: 80,
                max: 160,
                p95: 150,
            },
            jitter_ms: 20,
            ..Default::default()
        };

        let state = evaluate_state(true, &summary, &config());

        assert_eq!(state.overall, OverallState::Online);
        assert_eq!(state.quality, QualityLayerState::Stable);
        assert!(state.score > 80);
    }

    #[test]
    fn extreme_consecutive_failures_do_not_overflow_score() {
        let summary = QualitySummary {
            sample_count: 20,
            failure_count: 20,
            failure_rate: 1.0,
            latency_ms: LatencySummary {
                avg: u64::MAX,
                min: u64::MAX,
                max: u64::MAX,
                p95: u64::MAX,
            },
            jitter_ms: u64::MAX,
            consecutive_failures: usize::MAX,
            ..Default::default()
        };

        let score = calculate_score(&summary);

        assert_eq!(score, 0);
    }

    #[test]
    fn zero_thresholds_do_not_degrade_healthy_summary() {
        let summary = QualitySummary {
            sample_count: 20,
            success_count: 20,
            latency_ms: LatencySummary {
                avg: 100,
                min: 80,
                max: 160,
                p95: 150,
            },
            jitter_ms: 20,
            ..Default::default()
        };
        let config = StateConfig {
            degraded_failure_rate: 0.0,
            degraded_p95_latency_ms: 0,
            offline_consecutive_failures: 0,
        };

        let state = evaluate_state(true, &summary, &config);

        assert_eq!(state.overall, OverallState::Online);
        assert_eq!(state.quality, QualityLayerState::Stable);
    }
}
