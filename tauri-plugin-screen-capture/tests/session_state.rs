use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use async_trait::async_trait;
use tauri_plugin_screen_capture::{
    capture::{CaptureBackend, DummyCaptureBackend, FrameConsumer, RunningCapture},
    overlay::ShareOverlayFactory,
    CaptureErrorCode, CaptureErrorPayload, CaptureSourceKind, CaptureStatus, ListSourcesOptions,
    PermissionStatus, Result, ScreenCaptureState, StartCaptureOptions,
};
use tokio::sync::watch;

#[derive(Debug, Default)]
struct BackendLifecycleCounters {
    starts: AtomicUsize,
    pauses: AtomicUsize,
    resumes: AtomicUsize,
    stops: AtomicUsize,
}

#[derive(Debug, Default)]
struct OverlayLifecycleCounters {
    starts: AtomicUsize,
    hides: AtomicUsize,
    shows: AtomicUsize,
    stops: AtomicUsize,
}

#[derive(Debug)]
struct OverlayProbe {
    counters: Arc<OverlayLifecycleCounters>,
    fail_lifecycle_calls: bool,
}

#[async_trait]
impl tauri_plugin_screen_capture::overlay::ShareOverlay for OverlayProbe {
    async fn start(
        &self,
        _target: tauri_plugin_screen_capture::overlay::OverlayTarget,
    ) -> Result<()> {
        self.counters.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn hide(&self) -> Result<()> {
        self.counters.hides.fetch_add(1, Ordering::SeqCst);
        if self.fail_lifecycle_calls {
            return Err(tauri_plugin_screen_capture::Error::new(
                tauri_plugin_screen_capture::CaptureErrorCode::Internal,
                "overlay hide failed",
                true,
            ));
        }
        Ok(())
    }

    async fn show(&self) -> Result<()> {
        self.counters.shows.fetch_add(1, Ordering::SeqCst);
        if self.fail_lifecycle_calls {
            return Err(tauri_plugin_screen_capture::Error::new(
                tauri_plugin_screen_capture::CaptureErrorCode::Internal,
                "overlay show failed",
                true,
            ));
        }
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.counters.stops.fetch_add(1, Ordering::SeqCst);
        if self.fail_lifecycle_calls {
            return Err(tauri_plugin_screen_capture::Error::new(
                tauri_plugin_screen_capture::CaptureErrorCode::Internal,
                "overlay stop failed",
                true,
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct OverlayProbeFactory {
    creates: AtomicUsize,
    overlays: Mutex<Vec<Arc<OverlayLifecycleCounters>>>,
    fail_lifecycle_calls: bool,
}

impl OverlayProbeFactory {
    fn overlay_counters(&self) -> Vec<Arc<OverlayLifecycleCounters>> {
        self.overlays.lock().expect("overlays lock").clone()
    }
}

impl ShareOverlayFactory for OverlayProbeFactory {
    fn create_overlay(&self) -> Arc<dyn tauri_plugin_screen_capture::overlay::ShareOverlay> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        let counters = Arc::new(OverlayLifecycleCounters::default());
        self.overlays
            .lock()
            .expect("overlays lock")
            .push(Arc::clone(&counters));
        Arc::new(OverlayProbe {
            counters,
            fail_lifecycle_calls: self.fail_lifecycle_calls,
        })
    }
}

#[derive(Debug)]
struct ObservableBackend {
    counters: Arc<BackendLifecycleCounters>,
    last_options: Arc<Mutex<Option<StartCaptureOptions>>>,
}

#[async_trait]
impl CaptureBackend for ObservableBackend {
    async fn check_permission(&self) -> Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    async fn request_permission(&self) -> Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    async fn list_sources(
        &self,
        options: ListSourcesOptions,
    ) -> Result<Vec<tauri_plugin_screen_capture::CaptureSource>> {
        DummyCaptureBackend.list_sources(options).await
    }

    async fn start_capture(
        &self,
        options: StartCaptureOptions,
        _consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>> {
        self.counters.starts.fetch_add(1, Ordering::SeqCst);
        *self.last_options.lock().expect("last options lock") = Some(options);
        Ok(Box::new(ObservableRunningCapture {
            counters: Arc::clone(&self.counters),
        }))
    }
}

#[derive(Debug)]
struct ObservableRunningCapture {
    counters: Arc<BackendLifecycleCounters>,
}

#[async_trait]
impl RunningCapture for ObservableRunningCapture {
    async fn pause(&self) -> Result<()> {
        self.counters.pauses.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        self.counters.resumes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.counters.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Debug)]
struct FinishingBackend {
    counters: Arc<BackendLifecycleCounters>,
    finish_sender: watch::Sender<Option<CaptureErrorPayload>>,
}

#[async_trait]
impl CaptureBackend for FinishingBackend {
    async fn check_permission(&self) -> Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    async fn request_permission(&self) -> Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    async fn list_sources(
        &self,
        options: ListSourcesOptions,
    ) -> Result<Vec<tauri_plugin_screen_capture::CaptureSource>> {
        DummyCaptureBackend.list_sources(options).await
    }

    async fn start_capture(
        &self,
        _options: StartCaptureOptions,
        _consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>> {
        self.counters.starts.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FinishingRunningCapture {
            counters: Arc::clone(&self.counters),
            finish_receiver: self.finish_sender.subscribe(),
        }))
    }
}

#[derive(Debug)]
struct FinishingRunningCapture {
    counters: Arc<BackendLifecycleCounters>,
    finish_receiver: watch::Receiver<Option<CaptureErrorPayload>>,
}

#[async_trait]
impl RunningCapture for FinishingRunningCapture {
    async fn pause(&self) -> Result<()> {
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.counters.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn finish_receiver(&self) -> Option<watch::Receiver<Option<CaptureErrorPayload>>> {
        Some(self.finish_receiver.clone())
    }
}

fn start_options() -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "dummy-display-1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(1280),
        height: Some(720),
        capture_cursor: Some(true),
        publisher: None,
    }
}

#[tokio::test]
async fn dummy_backend_lists_display_and_window() {
    let backend = DummyCaptureBackend;

    let sources = backend
        .list_sources(ListSourcesOptions::default())
        .await
        .expect("dummy sources");

    assert!(sources
        .iter()
        .any(|source| source.kind == CaptureSourceKind::Display));
    assert!(sources
        .iter()
        .any(|source| source.kind == CaptureSourceKind::Window));
}

#[tokio::test]
async fn dummy_backend_permission_is_granted() {
    let backend = DummyCaptureBackend;
    assert_eq!(
        backend.check_permission().await.expect("permission"),
        PermissionStatus::Granted
    );
}

#[tokio::test]
async fn dummy_backend_defaults_do_not_return_current_app_window() {
    let backend = DummyCaptureBackend;

    let sources = backend
        .list_sources(ListSourcesOptions::default())
        .await
        .expect("dummy sources");

    assert!(sources.iter().all(|source| {
        source.kind != CaptureSourceKind::Window || source.pid != Some(std::process::id())
    }));
}

#[tokio::test]
async fn state_tracks_capture_lifecycle() {
    let state = ScreenCaptureState::with_backend(Arc::new(DummyCaptureBackend));
    let session = state.start_capture(start_options()).await.expect("start");
    assert_eq!(session.status, CaptureStatus::Publishing);

    state
        .pause_capture(&session.session_id)
        .await
        .expect("pause");
    assert_eq!(
        state
            .get_capture_session(&session.session_id)
            .await
            .unwrap()
            .status,
        CaptureStatus::Paused
    );

    state
        .resume_capture(&session.session_id)
        .await
        .expect("resume");
    assert_eq!(
        state
            .get_capture_session(&session.session_id)
            .await
            .unwrap()
            .status,
        CaptureStatus::Publishing
    );

    state.stop_capture(&session.session_id).await.expect("stop");
    assert!(state
        .get_capture_session(&session.session_id)
        .await
        .is_err());
}

#[tokio::test]
async fn state_drives_backend_capture_lifecycle() {
    let counters = Arc::new(BackendLifecycleCounters::default());
    let last_options = Arc::new(Mutex::new(None));
    let backend = Arc::new(ObservableBackend {
        counters: Arc::clone(&counters),
        last_options,
    });
    let state = ScreenCaptureState::with_backend(backend);

    let session = state.start_capture(start_options()).await.expect("start");
    assert_eq!(counters.starts.load(Ordering::SeqCst), 1);

    state
        .pause_capture(&session.session_id)
        .await
        .expect("pause");
    assert_eq!(counters.pauses.load(Ordering::SeqCst), 1);

    state
        .resume_capture(&session.session_id)
        .await
        .expect("resume");
    assert_eq!(counters.resumes.load(Ordering::SeqCst), 1);

    state.stop_capture(&session.session_id).await.expect("stop");
    assert_eq!(counters.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn state_drives_share_overlay_lifecycle() {
    let backend_counters = Arc::new(BackendLifecycleCounters::default());
    let last_options = Arc::new(Mutex::new(None));
    let backend = Arc::new(ObservableBackend {
        counters: backend_counters,
        last_options,
    });
    let overlay_counters = Arc::new(OverlayLifecycleCounters::default());
    let overlay = Arc::new(OverlayProbe {
        counters: Arc::clone(&overlay_counters),
        fail_lifecycle_calls: false,
    });
    let state = ScreenCaptureState::with_backend_and_overlay(backend, overlay);

    let session = state.start_capture(start_options()).await.expect("start");
    assert_eq!(overlay_counters.starts.load(Ordering::SeqCst), 1);

    state
        .pause_capture(&session.session_id)
        .await
        .expect("pause");
    assert_eq!(overlay_counters.hides.load(Ordering::SeqCst), 1);

    state
        .resume_capture(&session.session_id)
        .await
        .expect("resume");
    assert_eq!(overlay_counters.shows.load(Ordering::SeqCst), 1);

    state.stop_capture(&session.session_id).await.expect("stop");
    assert_eq!(overlay_counters.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn state_creates_independent_share_overlay_per_session() {
    let backend = Arc::new(ObservableBackend {
        counters: Arc::new(BackendLifecycleCounters::default()),
        last_options: Arc::new(Mutex::new(None)),
    });
    let overlay_factory = Arc::new(OverlayProbeFactory::default());
    let state = ScreenCaptureState::with_backend_and_overlay_factory(
        backend,
        Arc::clone(&overlay_factory) as Arc<dyn ShareOverlayFactory>,
    );

    let first_session = state
        .start_capture(start_options())
        .await
        .expect("first start");
    let second_session = state
        .start_capture(StartCaptureOptions {
            source_id: "dummy-display-2".to_string(),
            ..start_options()
        })
        .await
        .expect("second start");

    assert_ne!(first_session.session_id, second_session.session_id);
    assert_eq!(overlay_factory.creates.load(Ordering::SeqCst), 2);

    let overlays = overlay_factory.overlay_counters();
    assert_eq!(overlays.len(), 2);
    assert!(!Arc::ptr_eq(&overlays[0], &overlays[1]));
    assert_eq!(overlays[0].starts.load(Ordering::SeqCst), 1);
    assert_eq!(overlays[1].starts.load(Ordering::SeqCst), 1);

    state
        .pause_capture(&first_session.session_id)
        .await
        .expect("pause first session");
    assert_eq!(overlays[0].hides.load(Ordering::SeqCst), 1);
    assert_eq!(overlays[1].hides.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn state_keeps_session_consistent_when_overlay_lifecycle_calls_fail() {
    let backend = Arc::new(ObservableBackend {
        counters: Arc::new(BackendLifecycleCounters::default()),
        last_options: Arc::new(Mutex::new(None)),
    });
    let overlay_factory = Arc::new(OverlayProbeFactory {
        fail_lifecycle_calls: true,
        ..OverlayProbeFactory::default()
    });
    let state = ScreenCaptureState::with_backend_and_overlay_factory(
        backend,
        Arc::clone(&overlay_factory) as Arc<dyn ShareOverlayFactory>,
    );

    let session = state.start_capture(start_options()).await.expect("start");

    state
        .pause_capture(&session.session_id)
        .await
        .expect("pause ignores overlay hide failure");
    assert_eq!(
        state
            .get_capture_session(&session.session_id)
            .await
            .expect("session after pause")
            .status,
        CaptureStatus::Paused
    );

    state
        .resume_capture(&session.session_id)
        .await
        .expect("resume ignores overlay show failure");
    assert_eq!(
        state
            .get_capture_session(&session.session_id)
            .await
            .expect("session after resume")
            .status,
        CaptureStatus::Publishing
    );

    state
        .stop_capture(&session.session_id)
        .await
        .expect("stop ignores overlay stop failure");
    assert!(state
        .get_capture_session(&session.session_id)
        .await
        .is_err());

    let overlays = overlay_factory.overlay_counters();
    assert_eq!(overlays.len(), 1);
    assert_eq!(overlays[0].hides.load(Ordering::SeqCst), 1);
    assert_eq!(overlays[0].shows.load(Ordering::SeqCst), 1);
    assert_eq!(overlays[0].stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn state_removes_session_when_running_capture_finishes_after_source_closes() {
    let counters = Arc::new(BackendLifecycleCounters::default());
    let (finish_sender, _) = watch::channel(None);
    let backend = Arc::new(FinishingBackend {
        counters: Arc::clone(&counters),
        finish_sender: finish_sender.clone(),
    });
    let overlay_factory = Arc::new(OverlayProbeFactory::default());
    let state = ScreenCaptureState::with_backend_and_overlay_factory(
        backend,
        Arc::clone(&overlay_factory) as Arc<dyn ShareOverlayFactory>,
    );

    let session = state
        .start_capture(StartCaptureOptions {
            source_id: "dummy-window-1".to_string(),
            source_kind: CaptureSourceKind::Window,
            ..start_options()
        })
        .await
        .expect("start");

    finish_sender
        .send(Some(CaptureErrorPayload {
            code: CaptureErrorCode::SourceUnavailable,
            message: "window closed".to_string(),
            recoverable: true,
            details: None,
        }))
        .expect("send finish");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if state
                .get_capture_session(&session.session_id)
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("session removed after capture finish");

    assert_eq!(counters.stops.load(Ordering::SeqCst), 1);
    let overlays = overlay_factory.overlay_counters();
    assert_eq!(overlays.len(), 1);
    assert_eq!(overlays[0].stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn state_normalizes_capture_size_before_starting_backend() {
    let counters = Arc::new(BackendLifecycleCounters::default());
    let last_options = Arc::new(Mutex::new(None));
    let backend = Arc::new(ObservableBackend {
        counters,
        last_options: Arc::clone(&last_options),
    });
    let state = ScreenCaptureState::with_backend(backend);

    state
        .start_capture(StartCaptureOptions {
            source_id: "dummy-window-1".to_string(),
            source_kind: CaptureSourceKind::Window,
            fps: Some(30),
            width: Some(853),
            height: Some(481),
            capture_cursor: Some(true),
            publisher: None,
        })
        .await
        .expect("start");

    let options = last_options
        .lock()
        .expect("last options lock")
        .clone()
        .expect("backend options");
    assert_eq!(options.width, Some(852));
    assert_eq!(options.height, Some(480));
}
