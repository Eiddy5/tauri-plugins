use chrono::Utc;

use crate::{
    NetWatcherSnapshot, NetworkSnapshot, ProbeResult, ReachabilityConfigSnapshot,
    ReachabilitySnapshot, ReachabilityTargetSnapshot, SnapshotChanges, SnapshotMeta, SnapshotState,
};

pub(crate) struct SnapshotBuildInput {
    pub(crate) network: NetworkSnapshot,
    pub(crate) state: SnapshotState,
    pub(crate) reachability_config: ReachabilityConfigSnapshot,
    pub(crate) reachability_targets: Vec<ReachabilityTargetSnapshot>,
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
            config: input.reachability_config,
            targets: input.reachability_targets,
        },
        changes: SnapshotChanges {
            has_changes: false,
            previous_overall: Some(previous.state.overall.clone()),
            current_overall,
            changed_fields: Vec::new(),
            changed_target_ids: Vec::new(),
        },
    };
    let changed_target_ids =
        changed_target_ids(&previous.reachability.targets, &next.reachability.targets);
    let changed_fields = changed_fields(previous, &next);

    NetWatcherSnapshot {
        changes: SnapshotChanges {
            has_changes: !changed_fields.is_empty(),
            changed_fields,
            changed_target_ids,
            ..next.changes
        },
        ..next
    }
}

fn changed_fields(previous: &NetWatcherSnapshot, next: &NetWatcherSnapshot) -> Vec<String> {
    let mut fields = Vec::new();

    if previous.network.primary_interface_id != next.network.primary_interface_id
        || previous.network.interfaces != next.network.interfaces
    {
        fields.push("network".to_string());
    }

    if internet_changed(&previous.network.internet, &next.network.internet) {
        fields.push("network.internet".to_string());
    }

    if previous.reachability.config.interval_ms != next.reachability.config.interval_ms {
        fields.push("reachability.config.intervalMs".to_string());
    }

    if previous.reachability.config.window_size != next.reachability.config.window_size {
        fields.push("reachability.config.windowSize".to_string());
    }

    if previous.reachability.config.timeout_ms != next.reachability.config.timeout_ms {
        fields.push("reachability.config.timeoutMs".to_string());
    }

    if !changed_target_ids(&previous.reachability.targets, &next.reachability.targets).is_empty() {
        fields.push("reachability.targets".to_string());
    }

    if previous.state.overall != next.state.overall {
        fields.push("state.overall".to_string());
    }

    if previous.state.network != next.state.network {
        fields.push("state.network".to_string());
    }

    if previous.state.internet != next.state.internet {
        fields.push("state.internet".to_string());
    }

    if previous.state.reason != next.state.reason {
        fields.push("state.reason".to_string());
    }

    fields
}

fn internet_changed(previous: &crate::InternetSnapshot, next: &crate::InternetSnapshot) -> bool {
    previous.status != next.status
        || previous.verified != next.verified
        || previous.system_hint != next.system_hint
        || previous
            .active_probe
            .as_ref()
            .map(|probe| (&probe.status, probe.http_status, &probe.error))
            != next
                .active_probe
                .as_ref()
                .map(|probe| (&probe.status, probe.http_status, &probe.error))
        || previous.captive_portal != next.captive_portal
        || previous.consecutive_failures != next.consecutive_failures
        || previous.reason != next.reason
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

fn changed_target_ids(
    previous: &[ReachabilityTargetSnapshot],
    next: &[ReachabilityTargetSnapshot],
) -> Vec<String> {
    let mut changed = next
        .iter()
        .filter(|next_target| {
            match previous
                .iter()
                .find(|previous_target| previous_target.id == next_target.id)
            {
                Some(previous_target) => target_changed(previous_target, next_target),
                None => true,
            }
        })
        .map(|target| target.id.clone())
        .collect::<Vec<_>>();

    changed.extend(
        previous
            .iter()
            .filter(|previous_target| {
                !next
                    .iter()
                    .any(|next_target| next_target.id == previous_target.id)
            })
            .map(|target| target.id.clone()),
    );

    changed
}

fn target_changed(
    previous: &ReachabilityTargetSnapshot,
    next: &ReachabilityTargetSnapshot,
) -> bool {
    previous.id != next.id
        || previous.state != next.state
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        NetWatcherConfig, OverallState, ProbePhases, ProbeStatus, QualitySummary,
        ReachabilityStatus,
    };

    fn snapshot() -> NetWatcherSnapshot {
        let config = NetWatcherConfig {
            targets: vec![crate::ReachabilityTargetConfig {
                id: "api".to_string(),
                url: "https://api.example.com/health".to_string(),
            }],
            ..Default::default()
        };

        NetWatcherSnapshot::initial_with_config("0.1.0", &config)
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
    fn changed_fields_reports_reachability_config_and_target() {
        let mut next = snapshot();
        next.reachability.config.interval_ms += 1;
        next.reachability.targets[0].target.url = "https://example.com/health".to_string();
        next.reachability.targets[0].current_probe = Some(probe("probe_1"));
        next.reachability.targets[0].state.reachability = ReachabilityStatus::Reachable;

        let fields = fields_for(&next);

        assert!(fields.contains(&"reachability.config.intervalMs".to_string()));
        assert!(fields.contains(&"reachability.targets".to_string()));
        assert_eq!(
            changed_target_ids(&snapshot().reachability.targets, &next.reachability.targets),
            vec!["api"]
        );
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
    fn target_summary_changes_are_reported_as_target_changes() {
        let mut next = snapshot();
        next.reachability.targets[0].summary = QualitySummary {
            sample_count: 1,
            success_count: 1,
            last_success_at: Some(Utc::now()),
            ..Default::default()
        };

        let fields = fields_for(&next);

        assert_eq!(fields, vec!["reachability.targets"]);
    }

    #[test]
    fn changed_fields_ignores_summary_timestamp_churn() {
        let mut previous = snapshot();
        previous.reachability.targets[0].summary = QualitySummary {
            sample_count: 2,
            success_count: 2,
            last_success_at: Some(Utc::now()),
            ..Default::default()
        };
        let mut next = previous.clone();
        next.reachability.targets[0].summary.last_success_at = Some(Utc::now());

        let fields = changed_fields(&previous, &next);

        assert!(fields.is_empty());
    }

    #[test]
    fn changed_fields_keeps_state_field_precision() {
        let mut next = snapshot();
        next.state.overall = OverallState::Online;
        next.state.reason = "network_interface_available".to_string();

        let fields = fields_for(&next);

        assert!(fields.contains(&"state.overall".to_string()));
        assert!(fields.contains(&"state.reason".to_string()));
    }

    #[test]
    fn internet_status_change_is_reported_precisely() {
        let mut next = snapshot();
        next.network.internet.status = crate::InternetStatus::Available;
        next.network.internet.verified = true;
        next.network.internet.reason = "internet_probe_succeeded".to_string();
        next.state.internet = crate::InternetStatus::Available;

        let fields = fields_for(&next);

        assert!(fields.contains(&"network.internet".to_string()));
        assert!(fields.contains(&"state.internet".to_string()));
        assert!(!fields.contains(&"network".to_string()));
    }

    #[test]
    fn internet_probe_timing_churn_does_not_emit_change() {
        let mut previous = snapshot();
        previous.network.internet.checked_at = Some(Utc::now());
        previous.network.internet.active_probe = Some(crate::InternetProbeResult {
            status: crate::InternetProbeStatus::Success,
            duration_ms: 40,
            http_status: Some(200),
            error: None,
        });
        let mut next = previous.clone();
        next.network.internet.checked_at = Some(Utc::now());
        next.network
            .internet
            .active_probe
            .as_mut()
            .unwrap()
            .duration_ms = 80;

        let fields = changed_fields(&previous, &next);

        assert!(fields.is_empty());
    }
}
