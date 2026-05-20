use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    capture::{CaptureBackend, RunningCapture},
    error::Error,
    models::{
        Capabilities, CaptureErrorCode, CapturePublisherKind, CaptureSession, CaptureSource,
        CaptureStats, CaptureStatus, ListSourcesOptions, PermissionStatus, StartCaptureOptions,
        WebRtcAnswer, WebRtcIceCandidate, WebRtcOffer,
    },
    pipeline::CapturePipeline,
    publisher::{AgoraPublisher, CapturePublisher, NullPublisher, WebRtcLoopbackPublisher},
    webrtc::signaling::WebRtcSignalingState,
    Result,
};

struct SessionRecord {
    session: CaptureSession,
    publisher: Arc<dyn CapturePublisher>,
    pipeline: Arc<CapturePipeline>,
    running_capture: Arc<dyn RunningCapture>,
    webrtc_signaling: Option<Arc<WebRtcSignalingState>>,
}

pub struct ScreenCaptureState {
    backend: Arc<dyn CaptureBackend>,
    sessions: Mutex<HashMap<String, SessionRecord>>,
}

impl Default for ScreenCaptureState {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        let backend: Arc<dyn CaptureBackend> = Arc::new(crate::capture::MacOsCaptureBackend);
        #[cfg(not(target_os = "macos"))]
        let backend: Arc<dyn CaptureBackend> = Arc::new(crate::capture::DummyCaptureBackend);

        Self::with_backend(backend)
    }
}

impl ScreenCaptureState {
    pub fn with_backend(backend: Arc<dyn CaptureBackend>) -> Self {
        Self {
            backend,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            platform: std::env::consts::OS.to_string(),
            supports_display_capture: cfg!(target_os = "macos"),
            supports_window_capture: cfg!(target_os = "macos"),
            supports_thumbnails: cfg!(target_os = "macos"),
            supports_cursor_capture: cfg!(target_os = "macos"),
            supports_webrtc_loopback: true,
            supports_agora: false,
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
        let publisher_kind = options.effective_publisher();
        let (publisher, webrtc_signaling): (Arc<dyn CapturePublisher>, _) = match publisher_kind {
            CapturePublisherKind::Null => (Arc::new(NullPublisher::default()), None),
            CapturePublisherKind::WebRtcLoopback => {
                let signaling = WebRtcSignalingState::new().await?;
                let publisher = WebRtcLoopbackPublisher::new(signaling);
                let signaling = publisher.signaling();
                (Arc::new(publisher), Some(signaling))
            }
            CapturePublisherKind::Agora => (Arc::new(AgoraPublisher), None),
        };

        publisher.start(options.clone()).await?;
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

        let session = CaptureSession {
            session_id: Uuid::new_v4().to_string(),
            source_id: options.source_id,
            source_kind: options.source_kind,
            status: CaptureStatus::Publishing,
            publisher: publisher_kind,
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
                webrtc_signaling,
            },
        );

        Ok(session)
    }

    pub async fn pause_capture(&self, session_id: &str) -> Result<()> {
        let (publisher, running_capture) = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| {
                    (
                        Arc::clone(&record.publisher),
                        Arc::clone(&record.running_capture),
                    )
                })
                .ok_or_else(invalid_session_error)?
        };

        running_capture.pause().await?;
        publisher.pause().await?;

        let mut sessions = self.sessions.lock().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or_else(invalid_session_error)?;
        record.session.status = CaptureStatus::Paused;
        Ok(())
    }

    pub async fn resume_capture(&self, session_id: &str) -> Result<()> {
        let (publisher, running_capture) = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| {
                    (
                        Arc::clone(&record.publisher),
                        Arc::clone(&record.running_capture),
                    )
                })
                .ok_or_else(invalid_session_error)?
        };

        running_capture.resume().await?;
        publisher.resume().await?;

        let mut sessions = self.sessions.lock().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or_else(invalid_session_error)?;
        record.session.status = CaptureStatus::Publishing;
        Ok(())
    }

    pub async fn stop_capture(&self, session_id: &str) -> Result<()> {
        let (publisher, running_capture) = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| {
                    (
                        Arc::clone(&record.publisher),
                        Arc::clone(&record.running_capture),
                    )
                })
                .ok_or_else(invalid_session_error)?
        };

        running_capture.stop().await?;
        publisher.stop().await?;
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
        signaling.accept_answer(answer).await
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
        if record.session.publisher != CapturePublisherKind::WebRtcLoopback {
            return Err(Error::new(
                CaptureErrorCode::PublisherUnsupported,
                "capture session is not using WebRTC loopback publisher",
                false,
            ));
        }
        record.webrtc_signaling.as_ref().cloned().ok_or_else(|| {
            Error::new(
                CaptureErrorCode::PublisherUnsupported,
                "capture session does not expose WebRTC signaling",
                false,
            )
        })
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
