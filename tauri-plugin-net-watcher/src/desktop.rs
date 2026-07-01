use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::de::DeserializeOwned;
use tauri::{
    async_runtime::{self, JoinHandle, RwLock},
    plugin::PluginApi,
    AppHandle, Emitter, Runtime,
};

use crate::{
    network::{has_available_interface, read_network_snapshot},
    probe::HttpProber,
    state::{evaluate_state, StateConfig},
    stats::RollingWindow,
    Error, NetWatcherConfig, NetWatcherSnapshot, NetworkSnapshot, ProbeTarget, ProbeTargetType,
    QualityConfigSnapshot, QualitySnapshot, Result, SnapshotChanges, SnapshotMeta,
    StartWatchingOptions,
};

const SNAPSHOT_EVENT: &str = "net-watcher://snapshot-updated";

pub fn init<R, C>(
    app: &AppHandle<R>,
    _api: PluginApi<R, C>,
    config: NetWatcherConfig,
) -> Result<NetWatcher<R>>
where
    R: Runtime,
    C: DeserializeOwned,
{
    let watcher = NetWatcher::new(app.clone(), config.clone());

    if config.auto_start {
        watcher.start_background(None)?;
    }

    Ok(watcher)
}

/// Access to the net-watcher APIs.
pub struct NetWatcher<R: Runtime> {
    app: AppHandle<R>,
    config: Arc<NetWatcherConfig>,
    snapshot: Arc<RwLock<NetWatcherSnapshot>>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl<R> NetWatcher<R>
where
    R: Runtime,
{
    fn new(app: AppHandle<R>, config: NetWatcherConfig) -> Self {
        Self {
            app,
            snapshot: Arc::new(RwLock::new(NetWatcherSnapshot::initial_with_config(
                env!("CARGO_PKG_VERSION"),
                &config,
            ))),
            config: Arc::new(config),
            task: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) async fn get_snapshot(&self) -> Result<NetWatcherSnapshot> {
        Ok(self.snapshot.read().await.clone())
    }

    pub(crate) async fn get_config(&self) -> Result<NetWatcherConfig> {
        Ok((*self.config).clone())
    }

    pub(crate) async fn start_watching(&self, options: Option<StartWatchingOptions>) -> Result<()> {
        self.start_background(options)
    }

    pub(crate) async fn stop_watching(&self) -> Result<()> {
        let mut task = self
            .task
            .lock()
            .map_err(|_| Error::internal("net watcher task lock is poisoned"))?;

        clear_finished_task(&mut task);
        if task.is_none() {
            return Err(Error::not_watching());
        }

        if let Some(handle) = task.take() {
            handle.abort();
            Ok(())
        } else {
            Err(Error::not_watching())
        }
    }

    fn start_background(&self, options: Option<StartWatchingOptions>) -> Result<()> {
        let mut task = self
            .task
            .lock()
            .map_err(|_| Error::internal("net watcher task lock is poisoned"))?;
        clear_finished_task(&mut task);

        if task.is_some() {
            return Err(Error::already_watching());
        }

        let config = merge_runtime_config(&self.config, options);
        config.validate()?;

        let app = self.app.clone();
        let snapshot = self.snapshot.clone();
        let handle = async_runtime::spawn(run_loop(app, snapshot, config));
        *task = Some(handle);

        Ok(())
    }
}

fn clear_finished_task(task: &mut Option<JoinHandle<()>>) {
    if task
        .as_ref()
        .is_some_and(|handle| handle.inner().is_finished())
    {
        *task = None;
    }
}

fn merge_runtime_config(
    config: &NetWatcherConfig,
    options: Option<StartWatchingOptions>,
) -> NetWatcherConfig {
    match options {
        Some(options) => config.clone().with_runtime_options(options),
        None => config.clone(),
    }
}

async fn run_loop<R>(
    app: AppHandle<R>,
    snapshot: Arc<RwLock<NetWatcherSnapshot>>,
    config: NetWatcherConfig,
) where
    R: Runtime,
{
    let mut window = RollingWindow::new(config.window_size);
    let prober = HttpProber::new(config.timeout_ms);
    let target = ProbeTarget {
        target_type: ProbeTargetType::Http,
        url: config.target.clone(),
    };
    let state_config = StateConfig {
        degraded_failure_rate: config.degraded_failure_rate,
        degraded_p95_latency_ms: config.degraded_p95_latency_ms,
        offline_consecutive_failures: config.offline_consecutive_failures,
    };
    let quality_config = QualityConfigSnapshot {
        interval_ms: config.interval_ms,
        window_size: config.window_size,
        timeout_ms: config.timeout_ms,
    };

    loop {
        let (network, network_error) = match read_network_snapshot(config.include_mac_address) {
            Ok(network) => (network, None),
            Err(error) => (NetworkSnapshot::default(), Some(error)),
        };

        let probe = prober.probe(&target).await;
        window.push(probe);
        let summary = window.summary();
        let previous = snapshot.read().await.clone();
        let mut state = evaluate_state(has_available_interface(&network), &summary, &state_config);
        if let Some(error) = network_error {
            state.reason = format!("network_snapshot_failed:{}", error.code());
        }
        let current_overall = state.overall.clone();
        let quality = QualitySnapshot {
            config: quality_config.clone(),
            target: target.clone(),
            current_probe: window.latest(),
            summary,
        };

        let next = NetWatcherSnapshot {
            meta: SnapshotMeta {
                snapshot_id: format!("nw_{}", uuid::Uuid::new_v4()),
                timestamp: chrono::Utc::now(),
                platform: std::env::consts::OS.to_string(),
                plugin_version: env!("CARGO_PKG_VERSION").to_string(),
            },
            state,
            network: network.clone(),
            quality,
            changes: SnapshotChanges {
                has_changes: false,
                previous_overall: Some(previous.state.overall.clone()),
                current_overall,
                changed_fields: Vec::new(),
            },
        };
        let changed_fields = changed_fields(&previous, &next);
        let next = NetWatcherSnapshot {
            changes: SnapshotChanges {
                has_changes: !changed_fields.is_empty(),
                changed_fields,
                ..next.changes
            },
            ..next
        };

        *snapshot.write().await = next.clone();
        let _ = app.emit(SNAPSHOT_EVENT, next);

        tokio::time::sleep(Duration::from_millis(config.interval_ms)).await;
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

    if previous.quality.target != next.quality.target {
        fields.push("quality.target".to_string());
    }

    if previous.quality.current_probe != next.quality.current_probe {
        fields.push("quality.currentProbe".to_string());
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

fn changed_summary_fields(
    previous: &crate::QualitySummary,
    next: &crate::QualitySummary,
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

    if previous.last_success_at != next.last_success_at {
        fields.push("quality.summary.lastSuccessAt".to_string());
    }

    if previous.last_failure_at != next.last_failure_at {
        fields.push("quality.summary.lastFailureAt".to_string());
    }

    if previous.last_failure_reason != next.last_failure_reason {
        fields.push("quality.summary.lastFailureReason".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        LatencySummary, OverallState, ProbePhases, ProbeResult, ProbeStatus, QualitySummary,
    };
    use chrono::Utc;

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
    fn changed_fields_reports_quality_config_target_and_probe() {
        let mut next = snapshot();
        next.quality.config.interval_ms += 1;
        next.quality.target.url = "https://example.com/health".to_string();
        next.quality.current_probe = Some(probe("probe_1"));

        let fields = fields_for(&next);

        assert!(fields.contains(&"quality.config.intervalMs".to_string()));
        assert!(fields.contains(&"quality.target".to_string()));
        assert!(fields.contains(&"quality.currentProbe".to_string()));
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
            "quality.summary.lastSuccessAt",
            "quality.summary.lastFailureAt",
            "quality.summary.lastFailureReason",
        ] {
            assert!(fields.contains(&expected.to_string()), "missing {expected}");
        }

        assert!(!fields.contains(&"quality.summary".to_string()));
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
