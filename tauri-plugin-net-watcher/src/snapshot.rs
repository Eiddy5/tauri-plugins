use chrono::Utc;

use crate::{
    NetWatcherSnapshot, NetworkSnapshot, ProbeResult, QualityConfigSnapshot, QualitySnapshot,
    QualitySummary, ReachabilitySnapshot, ReachabilityTargetSnapshot, SnapshotChanges,
    SnapshotMeta, SnapshotState,
};

pub(crate) struct SnapshotBuildInput {
    pub(crate) network: NetworkSnapshot,
    pub(crate) state: SnapshotState,
    pub(crate) quality_config: QualityConfigSnapshot,
    pub(crate) reachability_targets: Vec<ReachabilityTargetSnapshot>,
    pub(crate) summary: QualitySummary,
}

pub(crate) fn build_snapshot(
    previous: &NetWatcherSnapshot,
    input: SnapshotBuildInput,
) -> NetWatcherSnapshot {
    let current_overall = input.state.overall.clone();
    let next = NetWatcherSnapshot {
        meta: SnapshotMeta {
            snapshot_id: format!("nw_{}", uuid::Uuid::new_v4()),
            timestamp: Utc::now(),
            platform: std::env::consts::OS.to_string(),
            plugin_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        state: input.state,
        network: input.network,
        reachability: ReachabilitySnapshot {
            targets: input.reachability_targets,
        },
        quality: QualitySnapshot {
            config: input.quality_config,
            summary: input.summary,
        },
        changes: SnapshotChanges {
            has_changes: false,
            previous_overall: Some(previous.state.overall.clone()),
            current_overall,
            changed_fields: Vec::new(),
        },
    };
    let changed_fields = changed_fields(previous, &next);

    NetWatcherSnapshot {
        changes: SnapshotChanges {
            has_changes: !changed_fields.is_empty(),
            changed_fields,
            ..next.changes
        },
        ..next
    }
}

fn changed_fields(previous: &NetWatcherSnapshot, next: &NetWatcherSnapshot) -> Vec<String> {
    let mut fields = Vec::new();

    if previous.network != next.network {
        fields.push("network".to_string());
    }

    if previous.quality.config.interval_ms != next.quality.config.interval_ms {
        fields.push("quality.config.intervalMs".to_string());
    }

    if previous.quality.config.window_size != next.quality.config.window_size {
        fields.push("quality.config.windowSize".to_string());
    }

    if previous.quality.config.timeout_ms != next.quality.config.timeout_ms {
        fields.push("quality.config.timeoutMs".to_string());
    }

    if reachability_targets_changed(&previous.reachability.targets, &next.reachability.targets) {
        fields.push("reachability.targets".to_string());
    }

    changed_summary_fields(
        &previous.quality.summary,
        &next.quality.summary,
        &mut fields,
    );

    if previous.state.overall != next.state.overall {
        fields.push("state.overall".to_string());
    }

    if previous.state.network != next.state.network {
        fields.push("state.network".to_string());
    }

    if previous.state.quality != next.state.quality {
        fields.push("state.quality".to_string());
    }

    if previous.state.score != next.state.score {
        fields.push("state.score".to_string());
    }

    if previous.state.reason != next.state.reason {
        fields.push("state.reason".to_string());
    }

    fields
}

fn probe_changed(previous: &Option<ProbeResult>, next: &Option<ProbeResult>) -> bool {
    match (previous, next) {
        (None, None) => false,
        (None, Some(_)) | (Some(_), None) => true,
        (Some(previous), Some(next)) => {
            previous.status != next.status
                || previous.http != next.http
                || previous.error != next.error
        }
    }
}

fn reachability_targets_changed(
    previous: &[ReachabilityTargetSnapshot],
    next: &[ReachabilityTargetSnapshot],
) -> bool {
    if previous.len() != next.len() {
        return true;
    }

    previous.iter().zip(next).any(|(previous, next)| {
        previous.id != next.id
            || previous.status != next.status
            || previous.target != next.target
            || probe_changed(&previous.current_probe, &next.current_probe)
            || previous.summary.sample_count != next.summary.sample_count
            || previous.summary.success_count != next.summary.success_count
            || previous.summary.failure_count != next.summary.failure_count
            || previous.summary.failure_rate != next.summary.failure_rate
            || previous.summary.latency_ms != next.summary.latency_ms
            || previous.summary.jitter_ms != next.summary.jitter_ms
            || previous.summary.consecutive_failures != next.summary.consecutive_failures
            || previous.summary.last_failure_reason != next.summary.last_failure_reason
    })
}

fn changed_summary_fields(
    previous: &QualitySummary,
    next: &QualitySummary,
    fields: &mut Vec<String>,
) {
    if previous.sample_count != next.sample_count {
        fields.push("quality.summary.sampleCount".to_string());
    }

    if previous.success_count != next.success_count {
        fields.push("quality.summary.successCount".to_string());
    }

    if previous.failure_count != next.failure_count {
        fields.push("quality.summary.failureCount".to_string());
    }

    if previous.failure_rate != next.failure_rate {
        fields.push("quality.summary.failureRate".to_string());
    }

    if previous.latency_ms.avg != next.latency_ms.avg {
        fields.push("quality.summary.latencyMs.avg".to_string());
    }

    if previous.latency_ms.min != next.latency_ms.min {
        fields.push("quality.summary.latencyMs.min".to_string());
    }

    if previous.latency_ms.max != next.latency_ms.max {
        fields.push("quality.summary.latencyMs.max".to_string());
    }

    if previous.latency_ms.p95 != next.latency_ms.p95 {
        fields.push("quality.summary.latencyMs.p95".to_string());
    }

    if previous.jitter_ms != next.jitter_ms {
        fields.push("quality.summary.jitterMs".to_string());
    }

    if previous.consecutive_failures != next.consecutive_failures {
        fields.push("quality.summary.consecutiveFailures".to_string());
    }

    if previous.last_failure_reason != next.last_failure_reason {
        fields.push("quality.summary.lastFailureReason".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        LatencySummary, NetWatcherConfig, OverallState, ProbePhases, ProbeStatus, QualitySummary,
        ReachabilityStatus,
    };

    fn snapshot() -> NetWatcherSnapshot {
        NetWatcherSnapshot::initial_with_config("0.1.0", &NetWatcherConfig::default())
    }

    fn probe(id: &str) -> ProbeResult {
        let now = Utc::now();

        ProbeResult {
            id: id.to_string(),
            status: ProbeStatus::Success,
            started_at: now,
            ended_at: now,
            duration_ms: 42,
            phases: ProbePhases::default(),
            http: None,
            error: None,
        }
    }

    fn fields_for(next: &NetWatcherSnapshot) -> Vec<String> {
        let previous = snapshot();

        changed_fields(&previous, next)
    }

    #[test]
    fn changed_fields_reports_quality_config_and_reachability_target() {
        let mut next = snapshot();
        next.quality.config.interval_ms += 1;
        next.reachability.targets[0].target.url = "https://example.com/health".to_string();
        next.reachability.targets[0].current_probe = Some(probe("probe_1"));
        next.reachability.targets[0].status = ReachabilityStatus::Reachable;

        let fields = fields_for(&next);

        assert!(fields.contains(&"quality.config.intervalMs".to_string()));
        assert!(fields.contains(&"reachability.targets".to_string()));
    }

    #[test]
    fn changed_fields_ignores_probe_identity_and_timestamps() {
        let mut previous = snapshot();
        previous.reachability.targets[0].current_probe = Some(probe("probe_1"));
        let mut next = previous.clone();
        next.reachability.targets[0].current_probe = Some(probe("probe_2"));

        let fields = changed_fields(&previous, &next);

        assert!(!fields.contains(&"reachability.targets".to_string()));
    }

    #[test]
    fn changed_fields_reports_probe_semantic_changes() {
        let mut previous = snapshot();
        previous.reachability.targets[0].current_probe = Some(probe("probe_1"));
        let mut next = previous.clone();
        let mut failed_probe = probe("probe_2");
        failed_probe.status = ProbeStatus::Failed;
        failed_probe.error = Some(crate::ProbeError {
            code: "http_timeout".to_string(),
            message: "timed out".to_string(),
        });
        next.reachability.targets[0].current_probe = Some(failed_probe);

        let fields = changed_fields(&previous, &next);

        assert!(fields.contains(&"reachability.targets".to_string()));
    }

    #[test]
    fn changed_fields_reports_quality_summary_subfields() {
        let mut next = snapshot();
        next.quality.summary = QualitySummary {
            sample_count: 1,
            success_count: 1,
            failure_count: 1,
            failure_rate: 0.5,
            latency_ms: LatencySummary {
                avg: 10,
                min: 5,
                max: 20,
                p95: 18,
            },
            jitter_ms: 3,
            consecutive_failures: 2,
            last_success_at: Some(Utc::now()),
            last_failure_at: Some(Utc::now()),
            last_failure_reason: Some("http_timeout".to_string()),
        };

        let fields = fields_for(&next);

        for expected in [
            "quality.summary.sampleCount",
            "quality.summary.successCount",
            "quality.summary.failureCount",
            "quality.summary.failureRate",
            "quality.summary.latencyMs.avg",
            "quality.summary.latencyMs.min",
            "quality.summary.latencyMs.max",
            "quality.summary.latencyMs.p95",
            "quality.summary.jitterMs",
            "quality.summary.consecutiveFailures",
            "quality.summary.lastFailureReason",
        ] {
            assert!(fields.contains(&expected.to_string()), "missing {expected}");
        }

        assert!(!fields.contains(&"quality.summary".to_string()));
    }

    #[test]
    fn changed_fields_ignores_summary_timestamp_churn() {
        let mut previous = snapshot();
        previous.quality.summary = QualitySummary {
            sample_count: 2,
            success_count: 2,
            last_success_at: Some(Utc::now()),
            ..Default::default()
        };
        let mut next = previous.clone();
        next.quality.summary.last_success_at = Some(Utc::now());

        let fields = changed_fields(&previous, &next);

        assert!(!fields.contains(&"quality.summary.lastSuccessAt".to_string()));
        assert!(fields.is_empty());
    }

    #[test]
    fn changed_fields_keeps_state_field_precision() {
        let mut next = snapshot();
        next.state.overall = OverallState::Online;
        next.state.score = 99;
        next.state.reason = "network_stable".to_string();

        let fields = fields_for(&next);

        assert!(fields.contains(&"state.overall".to_string()));
        assert!(fields.contains(&"state.score".to_string()));
        assert!(fields.contains(&"state.reason".to_string()));
    }
}
