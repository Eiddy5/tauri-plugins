use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    capture::{CaptureBackend, RunningCapture},
    error::Error,
    models::{
        Capabilities, CaptureErrorCode, CaptureSession, CaptureSource, CaptureStats, CaptureStatus,
        ListSourcesOptions, PermissionStatus, StartCaptureOptions, WebRtcAnswer,
        WebRtcIceCandidate, WebRtcOffer,
    },
    overlay::{DefaultShareOverlayFactory, OverlayTarget, ShareOverlay, ShareOverlayFactory},
    pipeline::CapturePipeline,
    publisher::{CapturePublisher, WebRtcPublisher},
    webrtc::signaling::WebRtcSignalingState,
    Result,
};

struct SessionRecord {
    session: CaptureSession,
    publisher: Arc<dyn CapturePublisher>,
    pipeline: Arc<CapturePipeline>,
    running_capture: Arc<dyn RunningCapture>,
    overlay: Arc<dyn ShareOverlay>,
    webrtc_signaling: Arc<WebRtcSignalingState>,
}

pub struct ScreenCaptureState {
    backend: Arc<dyn CaptureBackend>,
    overlay_factory: Arc<dyn ShareOverlayFactory>,
    sessions: Mutex<HashMap<String, SessionRecord>>,
}

struct SharedShareOverlayFactory {
    overlay: Arc<dyn ShareOverlay>,
}

impl ShareOverlayFactory for SharedShareOverlayFactory {
    fn create_overlay(&self) -> Arc<dyn ShareOverlay> {
        Arc::clone(&self.overlay)
    }
}

impl Default for ScreenCaptureState {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        let backend: Arc<dyn CaptureBackend> = Arc::new(crate::capture::MacOsCaptureBackend);
        #[cfg(target_os = "windows")]
        let backend: Arc<dyn CaptureBackend> = Arc::new(crate::capture::WindowsCaptureBackend);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let backend: Arc<dyn CaptureBackend> = Arc::new(crate::capture::DummyCaptureBackend);

        Self::with_backend(backend)
    }
}

impl ScreenCaptureState {
    pub fn with_backend(backend: Arc<dyn CaptureBackend>) -> Self {
        Self::with_backend_and_overlay_factory(backend, Arc::new(DefaultShareOverlayFactory))
    }

    pub fn with_backend_and_overlay(
        backend: Arc<dyn CaptureBackend>,
        overlay: Arc<dyn ShareOverlay>,
    ) -> Self {
        Self::with_backend_and_overlay_factory(
            backend,
            Arc::new(SharedShareOverlayFactory { overlay }),
        )
    }

    pub fn with_backend_and_overlay_factory(
        backend: Arc<dyn CaptureBackend>,
        overlay_factory: Arc<dyn ShareOverlayFactory>,
    ) -> Self {
        Self {
            backend,
            overlay_factory,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            platform: std::env::consts::OS.to_string(),
            supports_display_capture: cfg!(any(target_os = "macos", target_os = "windows")),
            supports_window_capture: cfg!(any(target_os = "macos", target_os = "windows")),
            supports_thumbnails: cfg!(any(target_os = "macos", target_os = "windows")),
            supports_cursor_capture: cfg!(any(target_os = "macos", target_os = "windows")),
            supports_webrtc: cfg!(any(target_os = "macos", target_os = "windows")),
        }
    }

    pub async fn check_permission(&self) -> Result<PermissionStatus> {
        self.backend.check_permission().await
    }

    pub async fn request_permission(&self) -> Result<PermissionStatus> {
        self.backend.request_permission().await
    }

    pub async fn list_sources(&self, options: ListSourcesOptions) -> Result<Vec<CaptureSource>> {
        self.backend.list_sources(options).await
    }

    pub async fn start_capture(&self, options: StartCaptureOptions) -> Result<CaptureSession> {
        let options = options.with_effective_video_size();
        let signaling = WebRtcSignalingState::new().await?;
        let publisher = WebRtcPublisher::new(signaling);
        let webrtc_signaling = publisher.signaling();
        let publisher: Arc<dyn CapturePublisher> = Arc::new(publisher);
        let pipeline = Arc::new(CapturePipeline::new(Arc::clone(&publisher)));

        let running_capture = match self
            .backend
            .start_capture(options.clone(), Box::new(Arc::clone(&pipeline)))
            .await
        {
            Ok(running_capture) => running_capture,
            Err(error) => {
                let _ = publisher.stop().await;
                return Err(error);
            }
        };
        let running_capture: Arc<dyn RunningCapture> = running_capture.into();

        if let Err(error) = publisher.start(options.clone()).await {
            let _ = running_capture.stop().await;
            let _ = publisher.stop().await;
            return Err(error);
        }

        let overlay = self.overlay_factory.create_overlay();
        if let Err(error) = overlay
            .start(OverlayTarget {
                source_id: options.source_id.clone(),
                source_kind: options.source_kind,
            })
            .await
        {
            let _ = running_capture.stop().await;
            let _ = publisher.stop().await;
            return Err(error);
        }

        let session = CaptureSession {
            session_id: Uuid::new_v4().to_string(),
            source_id: options.source_id,
            source_kind: options.source_kind,
            status: CaptureStatus::Publishing,
            started_at_ms: now_ms(),
            last_error: None,
        };

        self.sessions.lock().await.insert(
            session.session_id.clone(),
            SessionRecord {
                session: session.clone(),
                publisher,
                pipeline,
                running_capture,
                overlay,
                webrtc_signaling,
            },
        );

        Ok(session)
    }

    pub async fn pause_capture(&self, session_id: &str) -> Result<()> {
        let (publisher, running_capture, overlay) = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| {
                    (
                        Arc::clone(&record.publisher),
                        Arc::clone(&record.running_capture),
                        Arc::clone(&record.overlay),
                    )
                })
                .ok_or_else(invalid_session_error)?
        };

        running_capture.pause().await?;
        publisher.pause().await?;
        if let Err(error) = overlay.hide().await {
            eprintln!("[screen-capture] failed to hide share overlay: {error}");
        }

        let mut sessions = self.sessions.lock().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or_else(invalid_session_error)?;
        record.session.status = CaptureStatus::Paused;
        Ok(())
    }

    pub async fn resume_capture(&self, session_id: &str) -> Result<()> {
        let (publisher, running_capture, overlay) = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| {
                    (
                        Arc::clone(&record.publisher),
                        Arc::clone(&record.running_capture),
                        Arc::clone(&record.overlay),
                    )
                })
                .ok_or_else(invalid_session_error)?
        };

        running_capture.resume().await?;
        publisher.resume().await?;
        if let Err(error) = overlay.show().await {
            eprintln!("[screen-capture] failed to show share overlay: {error}");
        }

        let mut sessions = self.sessions.lock().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or_else(invalid_session_error)?;
        record.session.status = CaptureStatus::Publishing;
        Ok(())
    }

    pub async fn stop_capture(&self, session_id: &str) -> Result<()> {
        let (publisher, running_capture, overlay) = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| {
                    (
                        Arc::clone(&record.publisher),
                        Arc::clone(&record.running_capture),
                        Arc::clone(&record.overlay),
                    )
                })
                .ok_or_else(invalid_session_error)?
        };

        running_capture.stop().await?;
        publisher.stop().await?;
        if let Err(error) = overlay.stop().await {
            eprintln!("[screen-capture] failed to stop share overlay: {error}");
        }
        self.sessions
            .lock()
            .await
            .remove(session_id)
            .ok_or_else(invalid_session_error)?;
        Ok(())
    }

    pub async fn get_capture_session(&self, session_id: &str) -> Result<CaptureSession> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .map(|record| record.session.clone())
            .ok_or_else(invalid_session_error)
    }

    pub async fn get_capture_stats(&self, session_id: &str) -> Result<CaptureStats> {
        let (publisher, pipeline) = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| (Arc::clone(&record.publisher), Arc::clone(&record.pipeline)))
                .ok_or_else(invalid_session_error)?
        };

        let mut stats = pipeline.stats().await;
        let publisher_stats = publisher.stats().await?;
        stats.frames_published = stats.frames_published.max(publisher_stats.frames_published);
        stats.frames_dropped = stats.frames_dropped.max(publisher_stats.frames_dropped);
        stats.started = stats.started || publisher_stats.started;
        Ok(stats)
    }

    pub async fn create_webrtc_offer(&self, session_id: &str) -> Result<WebRtcOffer> {
        let signaling = self.webrtc_signaling(session_id).await?;
        signaling.create_offer().await
    }

    pub async fn accept_webrtc_answer(&self, session_id: &str, answer: WebRtcAnswer) -> Result<()> {
        let signaling = self.webrtc_signaling(session_id).await?;
        signaling.accept_answer(answer).await?;
        let connected = signaling.wait_connected(Duration::from_secs(1)).await;
        if !connected {
            eprintln!(
                "[screen-capture] WebRTC did not reach connected state before keyframe replay"
            );
        }

        let (publisher, pipeline) = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| (Arc::clone(&record.publisher), Arc::clone(&record.pipeline)))
                .ok_or_else(invalid_session_error)?
        };
        publisher.request_keyframe().await?;
        pipeline.replay_latest_frame().await
    }

    pub async fn add_webrtc_ice_candidate(
        &self,
        session_id: &str,
        candidate: WebRtcIceCandidate,
    ) -> Result<()> {
        let signaling = self.webrtc_signaling(session_id).await?;
        signaling.add_ice_candidate(candidate).await
    }

    async fn webrtc_signaling(&self, session_id: &str) -> Result<Arc<WebRtcSignalingState>> {
        let sessions = self.sessions.lock().await;
        let record = sessions.get(session_id).ok_or_else(invalid_session_error)?;
        Ok(Arc::clone(&record.webrtc_signaling))
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn invalid_session_error() -> Error {
    Error::new(
        CaptureErrorCode::InvalidSession,
        "capture session was not found",
        true,
    )
}
