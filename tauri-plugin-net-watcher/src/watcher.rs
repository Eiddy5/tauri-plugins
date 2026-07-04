use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tauri::{
    async_runtime::{self, JoinHandle, RwLock},
    AppHandle, Emitter, Runtime,
};
use tokio::sync::mpsc;

use crate::{
    network::{has_available_interface, read_network_snapshot},
    quality::{evaluate_state, HttpProber, RollingWindow, StateConfig},
    snapshot::{build_snapshot, SnapshotBuildInput},
    Error, NetWatcherConfig, NetWatcherSnapshot, NetworkSnapshot, ProbeStatus, ProbeTarget,
    ProbeTargetType, QualityConfigSnapshot, ReachabilityStatus, ReachabilityTargetSnapshot, Result,
    StartWatchingOptions,
};

const SNAPSHOT_EVENT: &str = "net-watcher://snapshot-updated";
const SYSTEM_EVENT_DEBOUNCE: Duration = Duration::from_millis(500);

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
    let target_configs = config.effective_targets();
    let mut aggregate_window = RollingWindow::new(
        config
            .window_size()
            .saturating_mul(target_configs.len())
            .max(1),
    );
    let mut targets = target_configs
        .into_iter()
        .map(|target| TargetProbeState {
            id: target.id,
            target: ProbeTarget {
                target_type: ProbeTargetType::Http,
                url: target.url,
            },
            window: RollingWindow::new(config.window_size()),
        })
        .collect::<Vec<_>>();
    let prober = HttpProber::new(config.timeout_ms);
    let state_config = StateConfig {
        degraded_failure_rate: config.degraded_failure_rate(),
        degraded_p95_latency_ms: config.degraded_p95_latency_ms(),
        offline_consecutive_failures: config.offline_consecutive_failures(),
    };
    let quality_config = QualityConfigSnapshot {
        interval_ms: config.interval_ms,
        window_size: config.window_size(),
        timeout_ms: config.timeout_ms,
    };

    loop {
        let (network, network_error) = match read_network_snapshot(config.include_mac_address()) {
            Ok(network) => (network, None),
            Err(error) => (NetworkSnapshot::default(), Some(error)),
        };

        for target in &mut targets {
            let probe = prober.probe(&target.target).await;
            target.window.push(probe.clone());
            aggregate_window.push(probe);
        }

        let summary = aggregate_window.summary();
        let previous = snapshot.read().await.clone();
        let mut state = evaluate_state(has_available_interface(&network), &summary, &state_config);
        if let Some(error) = network_error {
            state.reason = format!("network_snapshot_failed:{}", error.code());
        }

        let next = build_snapshot(
            &previous,
            SnapshotBuildInput {
                network,
                state,
                quality_config: quality_config.clone(),
                reachability_targets: targets.iter().map(reachability_target_snapshot).collect(),
                summary,
            },
        );

        let should_emit = next.changes.has_changes;
        *snapshot.write().await = next.clone();
        if should_emit {
            let _ = app.emit(SNAPSHOT_EVENT, next);
        }

        match wait_for_next_wake(Duration::from_millis(config.interval_ms), &mut signal_rx).await {
            WatcherWake::Interval | WatcherWake::SystemNetworkChanged => {}
            WatcherWake::Stop => break,
        }
    }
}

fn reachability_target_snapshot(target: &TargetProbeState) -> ReachabilityTargetSnapshot {
    let current_probe = target.window.latest();

    ReachabilityTargetSnapshot {
        id: target.id.clone(),
        status: reachability_status(current_probe.as_ref()),
        target: target.target.clone(),
        current_probe,
        summary: target.window.summary(),
    }
}

fn reachability_status(probe: Option<&crate::ProbeResult>) -> ReachabilityStatus {
    match probe.map(|probe| &probe.status) {
        Some(ProbeStatus::Success) => ReachabilityStatus::Reachable,
        Some(ProbeStatus::Failed) => ReachabilityStatus::Unreachable,
        None => ReachabilityStatus::Unknown,
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
