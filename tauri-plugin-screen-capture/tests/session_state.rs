use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use tauri_plugin_screen_capture::{
    capture::{CaptureBackend, DummyCaptureBackend, FrameConsumer, RunningCapture},
    pipeline::frame::VideoFrame,
    publisher::{CapturePublisher, CapturePublisherFactory, PublisherBundle},
    CaptureSourceKind, CaptureStats, CaptureStatus, ListSourcesOptions, PermissionStatus,
    PixelFormat, Result, ScreenCaptureState, StartCaptureOptions,
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

#[derive(Debug)]
struct OneFrameBackend;

#[async_trait]
impl CaptureBackend for OneFrameBackend {
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
        consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>> {
        consumer
            .push_frame(VideoFrame {
                width: 1,
                height: 1,
                pixel_format: PixelFormat::Bgra,
                timestamp_ns: 1,
                data: Arc::from([0, 0, 0, 255]),
            })
            .await?;
        Ok(Box::new(ObservableRunningCapture {
            counters: Arc::new(BackendLifecycleCounters::default()),
        }))
    }
}

#[derive(Debug, Default)]
struct RecordingPublisher {
    frames: Mutex<Vec<(u32, u32, u64)>>,
    stats: Mutex<CaptureStats>,
}

#[async_trait]
impl CapturePublisher for RecordingPublisher {
    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        self.stats.lock().unwrap().started = true;
        Ok(())
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.frames
            .lock()
            .unwrap()
            .push((frame.width, frame.height, frame.timestamp_ns));
        self.stats.lock().unwrap().frames_published += 1;
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        self.stats.lock().unwrap().started = false;
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        self.stats.lock().unwrap().started = true;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.stats.lock().unwrap().started = false;
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(self.stats.lock().unwrap().clone())
    }
}

#[derive(Debug)]
struct RecordingPublisherFactory {
    publisher: Arc<RecordingPublisher>,
}

#[async_trait]
impl CapturePublisherFactory for RecordingPublisherFactory {
    async fn create(&self, _options: &StartCaptureOptions) -> Result<PublisherBundle> {
        Ok(PublisherBundle::without_webrtc(self.publisher.clone()))
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

#[tokio::test]
async fn state_rejects_agora_publisher_until_sdk_adapter_exists() {
    let state = ScreenCaptureState::with_backend(Arc::new(DummyCaptureBackend));
    let mut options = start_options();
    options.publisher = Some(tauri_plugin_screen_capture::PublisherOptions {
        kind: tauri_plugin_screen_capture::PublisherKind::Agora,
        agora: Some(tauri_plugin_screen_capture::AgoraPublisherOptions {
            app_id: "app-id".to_string(),
            channel: "demo-channel".to_string(),
            uid: Some(42),
            token: Some("token".to_string()),
        }),
    });

    let error = state
        .start_capture(options)
        .await
        .expect_err("expected agora rejection");
    assert_eq!(
        error.payload().code,
        tauri_plugin_screen_capture::CaptureErrorCode::PublisherUnsupported
    );
}

#[tokio::test]
async fn state_routes_frames_to_custom_publisher_factory() {
    let publisher = Arc::new(RecordingPublisher::default());
    let state = ScreenCaptureState::with_backend_and_publisher_factory(
        Arc::new(OneFrameBackend),
        Arc::new(RecordingPublisherFactory {
            publisher: Arc::clone(&publisher),
        }),
    );

    let session = state.start_capture(start_options()).await.expect("start");

    assert_eq!(publisher.frames.lock().unwrap().as_slice(), &[(1, 1, 1)]);
    let stats = state
        .get_capture_stats(&session.session_id)
        .await
        .expect("stats");
    assert_eq!(stats.frames_published, 1);
    assert!(stats.started);
}
