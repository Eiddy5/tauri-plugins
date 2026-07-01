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
    QualityConfigSnapshot, QualitySnapshot, Result, SnapshotChanges, SnapshotMeta, SnapshotState,
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
        let changed_fields = changed_fields(&previous, &network, &state, &quality);

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
                has_changes: !changed_fields.is_empty(),
                previous_overall: Some(previous.state.overall),
                current_overall,
                changed_fields,
            },
        };

        *snapshot.write().await = next.clone();
        let _ = app.emit(SNAPSHOT_EVENT, next);

        tokio::time::sleep(Duration::from_millis(config.interval_ms)).await;
    }
}

fn changed_fields(
    previous: &NetWatcherSnapshot,
    network: &NetworkSnapshot,
    state: &SnapshotState,
    quality: &QualitySnapshot,
) -> Vec<String> {
    let mut fields = Vec::new();

    if previous.network != *network {
        fields.push("network".to_string());
    }

    if previous.quality.current_probe != quality.current_probe {
        fields.push("quality.currentProbe".to_string());
    }

    if previous.quality.summary != quality.summary {
        fields.push("quality.summary".to_string());
    }

    if previous.state.overall != state.overall {
        fields.push("state.overall".to_string());
    }

    if previous.state.network != state.network {
        fields.push("state.network".to_string());
    }

    if previous.state.quality != state.quality {
        fields.push("state.quality".to_string());
    }

    if previous.state.score != state.score {
        fields.push("state.score".to_string());
    }

    if previous.state.reason != state.reason {
        fields.push("state.reason".to_string());
    }

    fields
}
