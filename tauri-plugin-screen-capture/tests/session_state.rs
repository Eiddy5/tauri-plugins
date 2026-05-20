use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
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
        _options: StartCaptureOptions,
        _consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>> {
        self.counters.starts.fetch_add(1, Ordering::SeqCst);
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
    let backend = Arc::new(ObservableBackend {
        counters: Arc::clone(&counters),
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
