use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use tauri_plugin_screen_capture::{
    capture::{CaptureBackend, DummyCaptureBackend, FrameConsumer, RunningCapture},
    CaptureSourceKind, CaptureStatus, ListSourcesOptions, PermissionStatus, Result,
    ScreenCaptureState, StartCaptureOptions,
};

#[derive(Debug, Default)]
struct BackendLifecycleCounters {
    starts: AtomicUsize,
    pauses: AtomicUsize,
    resumes: AtomicUsize,
    stops: AtomicUsize,
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

fn start_options() -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "dummy-display-1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(1280),
        height: Some(720),
        capture_cursor: Some(true),
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
