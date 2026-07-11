use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::{stream, StreamExt};
use tauri::{
    async_runtime::{self, JoinHandle, RwLock},
    AppHandle, Emitter, Runtime,
};
use tokio::sync::mpsc;

use crate::{
    internet::InternetMonitor,
    network::{has_available_interface, read_network_snapshot},
    quality::{
        evaluate_network_state, evaluate_target_state, HttpProber, RollingWindow, StateConfig,
    },
    schedule::TargetSchedule,
    snapshot::{build_snapshot, SnapshotBuildInput},
    Error, NetWatcherConfig, NetWatcherSnapshot, NetworkSnapshot, NetworkUpdatedPayload,
    ProbeResult, ProbeTarget, ProbeTargetType, ReachabilityConfigSnapshot, ReachabilityStatus,
    ReachabilityTargetSnapshot, ReachabilityTargetState, Result, StartWatchingOptions,
    TargetQualityState, TargetUpdatedPayload,
};

const NETWORK_UPDATED_EVENT: &str = "net-watcher://network-updated";
const TARGET_UPDATED_EVENT: &str = "net-watcher://target-updated";
const SYSTEM_EVENT_DEBOUNCE: Duration = Duration::from_millis(500);
const MAX_CONCURRENT_PROBES: usize = 4;

/// Access to the net-watcher APIs.
pub struct NetWatcher<R: Runtime> {
    app: AppHandle<R>,
    config: Arc<NetWatcherConfig>,
    snapshot: Arc<RwLock<NetWatcherSnapshot>>,
    task: Arc<Mutex<Option<WatcherTask>>>,
}

struct WatcherTask {
    signal_tx: mpsc::UnboundedSender<WatcherSignal>,
    handle: JoinHandle<()>,
    _system_watcher: Option<SystemNetworkWatcher>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatcherSignal {
    Stop,
    SystemNetworkChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatcherWake {
    Interval,
    SystemNetworkChanged,
    Stop,
}

struct TargetProbeState {
    id: String,
    target: ProbeTarget,
    window: RollingWindow,
    schedule: TargetSchedule,
}

#[derive(Clone, PartialEq, Eq)]
struct NetworkFingerprint {
    primary_interface_id: Option<String>,
    interfaces: Vec<crate::NetworkInterface>,
}

impl NetworkFingerprint {
    fn from_snapshot(snapshot: &NetworkSnapshot) -> Self {
        Self {
            primary_interface_id: snapshot.primary_interface_id.clone(),
            interfaces: snapshot.interfaces.clone(),
        }
    }
}

impl<R> NetWatcher<R>
where
    R: Runtime,
{
    pub(crate) fn new(app: AppHandle<R>, config: NetWatcherConfig) -> Self {
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
        let task = {
            let mut task = self
                .task
                .lock()
                .map_err(|_| Error::internal("net watcher task lock is poisoned"))?;

            clear_finished_task(&mut task);
            if task.is_none() {
                return Err(Error::not_watching());
            }

            task.take().ok_or_else(Error::not_watching)?
        };

        let _ = task.signal_tx.send(WatcherSignal::Stop);
        let mut handle = task.handle;
        match tokio::time::timeout(Duration::from_secs(5), &mut handle).await {
            Ok(join_result) => join_result
                .map_err(|error| Error::internal(format!("net watcher task failed: {error}")))?,
            Err(_) => {
                handle.abort();
                let _ = handle.await;
            }
        }

        Ok(())
    }

    pub(crate) fn start_background(&self, options: Option<StartWatchingOptions>) -> Result<()> {
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
        let (signal_tx, signal_rx) = mpsc::unbounded_channel();
        let system_watcher =
            system_watcher_or_interval_only(start_system_network_watcher(signal_tx.clone()));
        let handle = async_runtime::spawn(run_loop(app, snapshot, config, signal_rx));
        *task = Some(WatcherTask {
            signal_tx,
            handle,
            _system_watcher: system_watcher,
        });

        Ok(())
    }
}

fn clear_finished_task(task: &mut Option<WatcherTask>) {
    if task
        .as_ref()
        .is_some_and(|task| task.handle.inner().is_finished())
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
    mut signal_rx: mpsc::UnboundedReceiver<WatcherSignal>,
) where
    R: Runtime,
{
    let started_at = Instant::now();
    let target_configs = config.targets.clone();
    let mut targets = target_configs
        .into_iter()
        .map(|target| TargetProbeState {
            id: target.id,
            target: ProbeTarget {
                target_type: ProbeTargetType::Http,
                url: target.url,
            },
            window: RollingWindow::new(config.window_size()),
            schedule: TargetSchedule::new(started_at),
        })
        .collect::<Vec<_>>();
    let prober = HttpProber::new(config.timeout_ms);
    let state_config = StateConfig {
        degraded_failure_rate: config.degraded_failure_rate(),
        degraded_p95_latency_ms: config.degraded_p95_latency_ms(),
    };
    let reachability_config = ReachabilityConfigSnapshot {
        interval_ms: config.interval_ms,
        window_size: config.window_size(),
        timeout_ms: config.timeout_ms,
    };
    let mut internet_monitor = InternetMonitor::new();
    let mut force_internet_check = true;
    let target_probe_interval = Duration::from_millis(config.interval_ms);
    let mut force_target_probe = true;
    let mut network_fingerprint = None;

    'watcher: loop {
        let (mut network, mut state, interface_available) =
            match read_network_snapshot(config.include_mac_address()) {
                Ok(network) => {
                    let interface_available = has_available_interface(&network);
                    let state = evaluate_network_state(Some(interface_available));
                    (network, state, Some(interface_available))
                }
                Err(error) => {
                    let mut state = evaluate_network_state(None);
                    state.reason = format!("network_snapshot_failed:{}", error.code());
                    (NetworkSnapshot::default(), state, None)
                }
            };

        let next_fingerprint = NetworkFingerprint::from_snapshot(&network);
        let network_path_changed = network_fingerprint.as_ref() != Some(&next_fingerprint);
        if network_path_changed {
            network_fingerprint = Some(next_fingerprint);
            internet_monitor.reset_for_network_change();
            let now = Instant::now();
            for target in &mut targets {
                target.window = RollingWindow::new(config.window_size());
                target.schedule.reset(now);
            }
            force_internet_check = true;
            force_target_probe = true;
        }

        let internet_refresh = internet_monitor.refresh(interface_available, force_internet_check);
        let internet = match run_interruptible(internet_refresh, &mut signal_rx).await {
            Ok(internet) => internet,
            Err(WatcherWake::SystemNetworkChanged) => {
                force_internet_check = true;
                force_target_probe = true;
                continue 'watcher;
            }
            Err(WatcherWake::Stop) => break,
            Err(WatcherWake::Interval) => unreachable!("signal wait cannot return interval"),
        };
        force_internet_check = false;
        state.internet = internet.status.clone();
        let targets_suspended = interface_available != Some(true)
            || internet.status == crate::InternetStatus::Unavailable;
        network.internet = internet;

        let mut suspended_target_ids = Vec::new();
        if targets_suspended {
            for target in &mut targets {
                if !target.schedule.is_suspended() {
                    target.schedule.suspend();
                    suspended_target_ids.push(target.id.clone());
                }
            }
        } else {
            let now = Instant::now();
            for target in &mut targets {
                if target.schedule.is_suspended() {
                    target.schedule.reset(now);
                    force_target_probe = true;
                }
            }
        }

        let previous = snapshot.read().await.clone();
        let network_next = build_snapshot(
            &previous,
            SnapshotBuildInput {
                network: network.clone(),
                state: state.clone(),
                reachability_config: reachability_config.clone(),
                reachability_targets: target_snapshots(&targets, &state_config),
            },
        );
        let network_changed = network_next.changes.changed_fields.iter().any(|field| {
            field == "network" || field.starts_with("network.") || field.starts_with("state.")
        });
        *snapshot.write().await = network_next.clone();
        if network_changed {
            let _ = app.emit(
                NETWORK_UPDATED_EVENT,
                NetworkUpdatedPayload::from_snapshot(&network_next),
            );
        }

        for target_id in suspended_target_ids {
            if let Some(target) = network_next
                .reachability
                .targets
                .iter()
                .find(|target| target.id == target_id)
            {
                let _ = app.emit(
                    TARGET_UPDATED_EVENT,
                    TargetUpdatedPayload::from_snapshot(&network_next, target),
                );
            }
        }

        let now = Instant::now();
        let due_target_indexes = if targets_suspended {
            Vec::new()
        } else {
            targets
                .iter()
                .enumerate()
                .filter(|(_, target)| force_target_probe || target.schedule.is_due(now))
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        };
        force_target_probe = false;

        if !due_target_indexes.is_empty() {
            let target_descriptors = due_target_indexes
                .iter()
                .map(|index| (*index, targets[*index].target.clone()))
                .collect::<Vec<_>>();
            let probe_stream = probe_target_stream(&prober, target_descriptors);
            tokio::pin!(probe_stream);

            loop {
                let next_probe = tokio::select! {
                    biased;
                    signal = signal_rx.recv() => match signal {
                        Some(WatcherSignal::SystemNetworkChanged) => {
                            match coalesce_system_network_events(
                                SYSTEM_EVENT_DEBOUNCE,
                                &mut signal_rx,
                            ).await {
                                WatcherWake::SystemNetworkChanged => {
                                    force_internet_check = true;
                                    force_target_probe = true;
                                    continue 'watcher;
                                }
                                WatcherWake::Stop => break 'watcher,
                                WatcherWake::Interval => {
                                    unreachable!("debounce cannot return interval")
                                }
                            }
                        }
                        Some(WatcherSignal::Stop) | None => break 'watcher,
                    },
                    probe = probe_stream.next() => probe,
                };

                let Some((index, probe)) = next_probe else {
                    break;
                };
                let target = &mut targets[index];
                let probe_status = probe.status.clone();
                target.window.push(probe);
                let summary = target.window.summary();
                let target_state =
                    evaluate_target_state(target.window.latest().as_ref(), &summary, &state_config);
                target.schedule.record_result(
                    &target.id,
                    probe_status,
                    target_state.quality,
                    target_probe_interval,
                    Instant::now(),
                );

                let previous = snapshot.read().await.clone();
                let target_next = build_snapshot(
                    &previous,
                    SnapshotBuildInput {
                        network: network.clone(),
                        state: state.clone(),
                        reachability_config: reachability_config.clone(),
                        reachability_targets: target_snapshots(&targets, &state_config),
                    },
                );
                *snapshot.write().await = target_next.clone();
                let target = &target_next.reachability.targets[index];
                let _ = app.emit(
                    TARGET_UPDATED_EVENT,
                    TargetUpdatedPayload::from_snapshot(&target_next, target),
                );
            }
        }

        let now = Instant::now();
        let target_check_in = targets
            .iter()
            .filter_map(|target| target.schedule.next_check_in(now))
            .min()
            .unwrap_or(Duration::MAX);
        let next_check_in =
            target_check_in.min(internet_monitor.next_check_in(interface_available));

        match wait_for_next_wake(next_check_in, &mut signal_rx).await {
            WatcherWake::Interval => {}
            WatcherWake::SystemNetworkChanged => {
                force_internet_check = true;
                force_target_probe = true;
            }
            WatcherWake::Stop => break,
        }
    }
}

fn probe_target_stream(
    prober: &HttpProber,
    targets: Vec<(usize, ProbeTarget)>,
) -> impl futures_util::Stream<Item = (usize, ProbeResult)> + '_ {
    stream::iter(targets)
        .map(move |(index, target)| async move { (index, prober.probe(&target).await) })
        .buffer_unordered(MAX_CONCURRENT_PROBES)
}

fn target_snapshots(
    targets: &[TargetProbeState],
    state_config: &StateConfig,
) -> Vec<ReachabilityTargetSnapshot> {
    targets
        .iter()
        .map(|target| reachability_target_snapshot(target, target.window.summary(), state_config))
        .collect()
}

fn reachability_target_snapshot(
    target: &TargetProbeState,
    summary: crate::QualitySummary,
    state_config: &StateConfig,
) -> ReachabilityTargetSnapshot {
    let current_probe = target.window.latest();
    let state = if target.schedule.is_suspended() {
        ReachabilityTargetState {
            reachability: ReachabilityStatus::Unknown,
            quality: TargetQualityState::Unknown,
            reason: "probe_suspended_network_unavailable".to_string(),
        }
    } else {
        evaluate_target_state(current_probe.as_ref(), &summary, state_config)
    };

    ReachabilityTargetSnapshot {
        id: target.id.clone(),
        state,
        target: target.target.clone(),
        current_probe,
        summary,
    }
}

async fn run_interruptible<F, T>(
    future: F,
    signal_rx: &mut mpsc::UnboundedReceiver<WatcherSignal>,
) -> std::result::Result<T, WatcherWake>
where
    F: Future<Output = T>,
{
    run_interruptible_with_debounce(future, signal_rx, SYSTEM_EVENT_DEBOUNCE).await
}

async fn run_interruptible_with_debounce<F, T>(
    future: F,
    signal_rx: &mut mpsc::UnboundedReceiver<WatcherSignal>,
    debounce: Duration,
) -> std::result::Result<T, WatcherWake>
where
    F: Future<Output = T>,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        signal = signal_rx.recv() => match signal {
            Some(WatcherSignal::SystemNetworkChanged) => {
                Err(coalesce_system_network_events(debounce, signal_rx).await)
            }
            Some(WatcherSignal::Stop) | None => Err(WatcherWake::Stop),
        },
        output = &mut future => Ok(output),
    }
}

async fn wait_for_next_wake(
    interval: Duration,
    signal_rx: &mut mpsc::UnboundedReceiver<WatcherSignal>,
) -> WatcherWake {
    wait_for_next_wake_with_debounce(interval, SYSTEM_EVENT_DEBOUNCE, signal_rx).await
}

async fn wait_for_next_wake_with_debounce(
    interval: Duration,
    debounce: Duration,
    signal_rx: &mut mpsc::UnboundedReceiver<WatcherSignal>,
) -> WatcherWake {
    match tokio::time::timeout(interval, signal_rx.recv()).await {
        Ok(Some(WatcherSignal::SystemNetworkChanged)) => {
            coalesce_system_network_events(debounce, signal_rx).await
        }
        Ok(Some(WatcherSignal::Stop)) | Ok(None) => WatcherWake::Stop,
        Err(_) => WatcherWake::Interval,
    }
}

async fn coalesce_system_network_events(
    debounce: Duration,
    signal_rx: &mut mpsc::UnboundedReceiver<WatcherSignal>,
) -> WatcherWake {
    loop {
        match tokio::time::timeout(debounce, signal_rx.recv()).await {
            Ok(Some(WatcherSignal::SystemNetworkChanged)) => {}
            Ok(Some(WatcherSignal::Stop)) | Ok(None) => return WatcherWake::Stop,
            Err(_) => return WatcherWake::SystemNetworkChanged,
        }
    }
}

struct SystemNetworkWatcher {
    #[cfg(target_os = "macos")]
    #[allow(dead_code)]
    inner: macos_events::MacosSystemNetworkWatcher,
    #[cfg(target_os = "windows")]
    #[allow(dead_code)]
    inner: windows_events::WindowsSystemNetworkWatcher,
}

fn start_system_network_watcher(
    tx: mpsc::UnboundedSender<WatcherSignal>,
) -> crate::Result<Option<SystemNetworkWatcher>> {
    start_platform_watcher(tx)
}

fn system_watcher_or_interval_only(
    result: crate::Result<Option<SystemNetworkWatcher>>,
) -> Option<SystemNetworkWatcher> {
    result.unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn start_platform_watcher(
    tx: mpsc::UnboundedSender<WatcherSignal>,
) -> crate::Result<Option<SystemNetworkWatcher>> {
    windows_events::WindowsSystemNetworkWatcher::start(tx)
        .map(|inner| Some(SystemNetworkWatcher { inner }))
}

#[cfg(target_os = "macos")]
fn start_platform_watcher(
    tx: mpsc::UnboundedSender<WatcherSignal>,
) -> crate::Result<Option<SystemNetworkWatcher>> {
    macos_events::MacosSystemNetworkWatcher::start(tx)
        .map(|inner| Some(SystemNetworkWatcher { inner }))
}

#[cfg(target_os = "macos")]
mod macos_events {
    use std::{
        sync::mpsc as std_mpsc,
        thread::{self, JoinHandle},
    };

    use core_foundation_sys::runloop::CFRunLoopWakeUp;
    use system_configuration::core_foundation::{
        array::CFArray,
        base::TCFType,
        runloop::{kCFRunLoopCommonModes, CFRunLoop},
        string::CFString,
    };
    use system_configuration::dynamic_store::{
        SCDynamicStore, SCDynamicStoreBuilder, SCDynamicStoreCallBackContext,
    };
    use tokio::sync::mpsc;

    use super::WatcherSignal;

    const WATCHER_NAME: &str = "tauri-plugin-net-watcher";
    const NETWORK_CHANGE_PATTERN: &str = "(State|Setup):/Network/.*";

    struct CallbackContext {
        tx: mpsc::UnboundedSender<WatcherSignal>,
    }

    pub(super) struct MacosSystemNetworkWatcher {
        run_loop: CFRunLoop,
        thread: Option<JoinHandle<()>>,
    }

    unsafe impl Send for MacosSystemNetworkWatcher {}

    impl MacosSystemNetworkWatcher {
        pub(super) fn start(tx: mpsc::UnboundedSender<WatcherSignal>) -> crate::Result<Self> {
            let (ready_tx, ready_rx) = std_mpsc::channel();
            let thread = thread::spawn(move || run_watcher_thread(tx, ready_tx));
            let run_loop = match ready_rx.recv() {
                Ok(Ok(run_loop)) => run_loop,
                Ok(Err(message)) => {
                    let _ = thread.join();
                    return Err(crate::Error::internal(message));
                }
                Err(error) => {
                    return Err(crate::Error::internal(format!(
                        "failed to start macOS network watcher thread: {error}"
                    )));
                }
            };

            Ok(Self {
                run_loop,
                thread: Some(thread),
            })
        }
    }

    impl Drop for MacosSystemNetworkWatcher {
        fn drop(&mut self) {
            self.run_loop.stop();
            unsafe {
                CFRunLoopWakeUp(self.run_loop.as_concrete_TypeRef());
            }

            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn run_watcher_thread(
        tx: mpsc::UnboundedSender<WatcherSignal>,
        ready_tx: std_mpsc::Sender<Result<CFRunLoop, String>>,
    ) {
        let callback_context = SCDynamicStoreCallBackContext {
            callout: network_changed,
            info: CallbackContext { tx },
        };

        let Some(store) = SCDynamicStoreBuilder::new(WATCHER_NAME)
            .callback_context(callback_context)
            .build()
        else {
            let _ = ready_tx.send(Err("failed to create macOS dynamic store".to_string()));
            return;
        };

        let watch_keys: CFArray<CFString> = CFArray::from_CFTypes(&[]);
        let watch_patterns = CFArray::from_CFTypes(&[CFString::from(NETWORK_CHANGE_PATTERN)]);
        if !store.set_notification_keys(&watch_keys, &watch_patterns) {
            let _ = ready_tx.send(Err(
                "failed to subscribe to macOS network changes".to_string()
            ));
            return;
        }

        let Some(run_loop_source) = store.create_run_loop_source() else {
            let _ = ready_tx.send(Err(
                "failed to create macOS network watcher run loop source".to_string(),
            ));
            return;
        };

        let run_loop = CFRunLoop::get_current();
        run_loop.add_source(&run_loop_source, unsafe { kCFRunLoopCommonModes });

        let _ = ready_tx.send(Ok(run_loop.clone()));
        CFRunLoop::run_current();
    }

    #[allow(clippy::needless_pass_by_value)]
    fn network_changed(
        _store: SCDynamicStore,
        _changed_keys: CFArray<CFString>,
        context: &mut CallbackContext,
    ) {
        send_system_network_changed(context);
    }

    fn send_system_network_changed(context: &CallbackContext) {
        let _ = context.tx.send(WatcherSignal::SystemNetworkChanged);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn callback_context_sends_system_network_changed_signal() {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let context = CallbackContext { tx };

            send_system_network_changed(&context);

            assert_eq!(rx.try_recv().unwrap(), WatcherSignal::SystemNetworkChanged);
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_events {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    use tokio::sync::mpsc;
    use windows_sys::Win32::{
        Foundation::{HANDLE, NO_ERROR},
        NetworkManagement::IpHelper::{
            CancelMibChangeNotify2, NotifyIpInterfaceChange, MIB_IPINTERFACE_ROW,
            MIB_NOTIFICATION_TYPE,
        },
        Networking::WinSock::AF_UNSPEC,
    };

    use super::WatcherSignal;

    struct CallbackContext {
        tx: mpsc::UnboundedSender<WatcherSignal>,
    }

    pub(super) struct WindowsSystemNetworkWatcher {
        handle: HANDLE,
        context: *mut CallbackContext,
    }

    unsafe impl Send for WindowsSystemNetworkWatcher {}

    impl WindowsSystemNetworkWatcher {
        pub(super) fn start(tx: mpsc::UnboundedSender<WatcherSignal>) -> crate::Result<Self> {
            let context = Box::into_raw(Box::new(CallbackContext { tx }));
            let mut handle = null_mut();

            let error = unsafe {
                NotifyIpInterfaceChange(
                    AF_UNSPEC,
                    Some(network_changed),
                    context.cast::<c_void>(),
                    false,
                    &mut handle,
                )
            };

            if error != NO_ERROR {
                unsafe {
                    drop(Box::from_raw(context));
                }
                return Err(crate::Error::internal(format!(
                    "failed to subscribe to Windows network changes: {error}"
                )));
            }

            Ok(Self { handle, context })
        }
    }

    impl Drop for WindowsSystemNetworkWatcher {
        fn drop(&mut self) {
            unsafe {
                if !self.handle.is_null() {
                    let _ = CancelMibChangeNotify2(self.handle);
                }

                if !self.context.is_null() {
                    drop(Box::from_raw(self.context));
                    self.context = null_mut();
                }
            }
        }
    }

    unsafe extern "system" fn network_changed(
        caller_context: *const c_void,
        _row: *const MIB_IPINTERFACE_ROW,
        _notification_type: MIB_NOTIFICATION_TYPE,
    ) {
        if caller_context.is_null() {
            return;
        }

        let context = unsafe { &*(caller_context as *const CallbackContext) };
        let _ = context.tx.send(WatcherSignal::SystemNetworkChanged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProbeStatus;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    async fn probes_targets_concurrently_up_to_internal_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut streams = Vec::new();
            for _ in 0..MAX_CONCURRENT_PROBES {
                streams.push(listener.accept().await.unwrap().0);
            }

            for mut stream in streams {
                let mut request = [0_u8; 512];
                let read = stream.read(&mut request).await.unwrap();
                assert!(read > 0);
                stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                    .await
                    .unwrap();
            }
        });
        let targets = (0..MAX_CONCURRENT_PROBES)
            .map(|index| ProbeTarget {
                target_type: ProbeTargetType::Http,
                url: format!("http://{address}/{index}"),
            })
            .enumerate()
            .collect::<Vec<_>>();

        let probes = probe_target_stream(&HttpProber::new(2_000), targets)
            .collect::<Vec<_>>()
            .await;
        server.await.unwrap();

        assert_eq!(probes.len(), MAX_CONCURRENT_PROBES);
        assert!(probes
            .iter()
            .all(|(_, probe)| probe.status == ProbeStatus::Success));
    }

    #[tokio::test]
    async fn network_change_interrupts_and_discards_in_flight_operation() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(WatcherSignal::SystemNetworkChanged).unwrap();

        let result = run_interruptible_with_debounce(
            std::future::pending::<()>(),
            &mut rx,
            Duration::from_millis(1),
        )
        .await;

        assert_eq!(result, Err(WatcherWake::SystemNetworkChanged));
    }

    #[test]
    fn network_fingerprint_ignores_internet_probe_evidence() {
        let mut first = NetworkSnapshot::default();
        first.internet.status = crate::InternetStatus::Available;
        let mut second = first.clone();
        second.internet.status = crate::InternetStatus::Unavailable;

        assert!(
            NetworkFingerprint::from_snapshot(&first) == NetworkFingerprint::from_snapshot(&second)
        );
    }

    #[test]
    fn suspended_target_reports_unknown_without_service_failure_sample() {
        let now = Instant::now();
        let mut schedule = TargetSchedule::new(now);
        schedule.suspend();
        let target = TargetProbeState {
            id: "api".to_string(),
            target: ProbeTarget {
                target_type: ProbeTargetType::Http,
                url: "https://api.example.com/health".to_string(),
            },
            window: RollingWindow::new(20),
            schedule,
        };
        let state_config = StateConfig {
            degraded_failure_rate: 0.15,
            degraded_p95_latency_ms: 800,
        };

        let snapshot =
            reachability_target_snapshot(&target, target.window.summary(), &state_config);

        assert_eq!(snapshot.state.reachability, ReachabilityStatus::Unknown);
        assert_eq!(snapshot.state.quality, TargetQualityState::Unknown);
        assert_eq!(snapshot.state.reason, "probe_suspended_network_unavailable");
        assert_eq!(snapshot.summary.sample_count, 0);
    }

    #[tokio::test]
    async fn wait_for_next_wake_returns_system_network_change_before_interval() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(WatcherSignal::SystemNetworkChanged).unwrap();

        let wake = wait_for_next_wake(Duration::from_secs(60), &mut rx).await;

        assert_eq!(wake, WatcherWake::SystemNetworkChanged);
    }

    #[tokio::test]
    async fn wait_for_next_wake_returns_stop_signal() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(WatcherSignal::Stop).unwrap();

        let wake = wait_for_next_wake(Duration::from_secs(60), &mut rx).await;

        assert_eq!(wake, WatcherWake::Stop);
    }

    #[tokio::test]
    async fn system_network_events_are_coalesced_during_debounce_window() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(WatcherSignal::SystemNetworkChanged).unwrap();
        tx.send(WatcherSignal::SystemNetworkChanged).unwrap();
        tx.send(WatcherSignal::SystemNetworkChanged).unwrap();

        let wake = wait_for_next_wake_with_debounce(
            Duration::from_secs(60),
            Duration::from_millis(1),
            &mut rx,
        )
        .await;

        assert_eq!(wake, WatcherWake::SystemNetworkChanged);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn stop_signal_wins_during_system_event_debounce() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(WatcherSignal::SystemNetworkChanged).unwrap();
        tx.send(WatcherSignal::Stop).unwrap();

        let wake = wait_for_next_wake_with_debounce(
            Duration::from_secs(60),
            Duration::from_millis(1),
            &mut rx,
        )
        .await;

        assert_eq!(wake, WatcherWake::Stop);
    }

    #[test]
    fn system_watcher_start_errors_fall_back_to_interval_only() {
        let watcher = system_watcher_or_interval_only(Err(Error::internal("boom")));

        assert!(watcher.is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_starts_system_network_watcher() {
        let (tx, _rx) = mpsc::unbounded_channel();

        let watcher = start_platform_watcher(tx).unwrap();

        assert!(watcher.is_some());
    }
}
