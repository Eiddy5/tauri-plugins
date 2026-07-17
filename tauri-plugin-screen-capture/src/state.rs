use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::sync::watch;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    annotation::AnnotationSession,
    capture::{CaptureBackend, CaptureFinishReason, RunningCapture},
    error::Error,
    models::{
        AnnotationDocument, AnnotationInputTarget, AnnotationState, AnnotationStateChangedEvent,
        AnnotationTool, AnnotationToolKind, Capabilities, CaptureErrorCode, CaptureSession,
        CaptureSessionEndedEvent, CaptureSource, CaptureStats, CaptureStatus, ListSourcesOptions,
        PermissionStatus, PublisherKind, StartCaptureOptions, WebRtcAnswer, WebRtcIceCandidate,
        WebRtcOffer,
    },
    overlay::{DefaultShareOverlayFactory, OverlayTarget, ShareOverlay, ShareOverlayFactory},
    pipeline::CapturePipeline,
    publisher::{
        AgoraPublisher, CapturePublisher, CapturePublisherFactory, PublisherBundle, WebRtcPublisher,
    },
    webrtc::signaling::WebRtcSignalingState,
    Result,
};

struct SessionRecord {
    session: CaptureSession,
    publisher: Arc<dyn CapturePublisher>,
    pipeline: Arc<CapturePipeline>,
    running_capture: Arc<dyn RunningCapture>,
    overlay: Arc<dyn ShareOverlay>,
    webrtc_signaling: Option<Arc<WebRtcSignalingState>>,
    annotation_session: Option<Arc<AnnotationSession>>,
}

pub trait CaptureEventSink: Send + Sync {
    fn emit_session_ended(&self, event: CaptureSessionEndedEvent) -> Result<()>;
    fn emit_annotation_state_changed(&self, _event: AnnotationStateChangedEvent) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default)]
struct NoopCaptureEventSink;

impl CaptureEventSink for NoopCaptureEventSink {
    fn emit_session_ended(&self, _event: CaptureSessionEndedEvent) -> Result<()> {
        Ok(())
    }
}

pub struct ScreenCaptureState {
    backend: Arc<dyn CaptureBackend>,
    overlay_factory: Arc<dyn ShareOverlayFactory>,
    publisher_factory: Arc<dyn CapturePublisherFactory>,
    event_sink: Arc<dyn CaptureEventSink>,
    sessions: Arc<Mutex<HashMap<String, SessionRecord>>>,
}

struct SharedShareOverlayFactory {
    overlay: Arc<dyn ShareOverlay>,
}

impl ShareOverlayFactory for SharedShareOverlayFactory {
    fn create_overlay(&self, _annotation_session: Arc<AnnotationSession>) -> Arc<dyn ShareOverlay> {
        Arc::clone(&self.overlay)
    }
}

impl Default for ScreenCaptureState {
    fn default() -> Self {
        Self::with_backend(default_backend())
    }
}

impl ScreenCaptureState {
    pub fn with_backend(backend: Arc<dyn CaptureBackend>) -> Self {
        Self::with_backend_overlay_and_publisher_factory(
            backend,
            Arc::new(DefaultShareOverlayFactory),
            Arc::new(DefaultPublisherFactory),
            Arc::new(NoopCaptureEventSink),
        )
    }

    pub fn with_publisher_factory(publisher_factory: Arc<dyn CapturePublisherFactory>) -> Self {
        Self::with_backend_overlay_and_publisher_factory(
            default_backend(),
            Arc::new(DefaultShareOverlayFactory),
            publisher_factory,
            Arc::new(NoopCaptureEventSink),
        )
    }

    pub fn with_backend_and_overlay(
        backend: Arc<dyn CaptureBackend>,
        overlay: Arc<dyn ShareOverlay>,
    ) -> Self {
        Self::with_backend_overlay_and_publisher_factory(
            backend,
            Arc::new(SharedShareOverlayFactory { overlay }),
            Arc::new(DefaultPublisherFactory),
            Arc::new(NoopCaptureEventSink),
        )
    }

    pub fn with_backend_and_overlay_factory(
        backend: Arc<dyn CaptureBackend>,
        overlay_factory: Arc<dyn ShareOverlayFactory>,
    ) -> Self {
        Self::with_backend_overlay_and_publisher_factory(
            backend,
            overlay_factory,
            Arc::new(DefaultPublisherFactory),
            Arc::new(NoopCaptureEventSink),
        )
    }

    pub fn with_backend_overlay_factory_and_event_sink(
        backend: Arc<dyn CaptureBackend>,
        overlay_factory: Arc<dyn ShareOverlayFactory>,
        event_sink: Arc<dyn CaptureEventSink>,
    ) -> Self {
        Self::with_backend_overlay_and_publisher_factory(
            backend,
            overlay_factory,
            Arc::new(DefaultPublisherFactory),
            event_sink,
        )
    }

    pub(crate) fn with_overlay_publisher_and_event_sink(
        overlay_factory: Arc<dyn ShareOverlayFactory>,
        publisher_factory: Option<Arc<dyn CapturePublisherFactory>>,
        event_sink: Arc<dyn CaptureEventSink>,
    ) -> Self {
        Self::with_backend_overlay_and_publisher_factory(
            default_backend(),
            overlay_factory,
            publisher_factory.unwrap_or_else(|| Arc::new(DefaultPublisherFactory)),
            event_sink,
        )
    }

    fn with_backend_overlay_and_publisher_factory(
        backend: Arc<dyn CaptureBackend>,
        overlay_factory: Arc<dyn ShareOverlayFactory>,
        publisher_factory: Arc<dyn CapturePublisherFactory>,
        event_sink: Arc<dyn CaptureEventSink>,
    ) -> Self {
        Self {
            backend,
            overlay_factory,
            publisher_factory,
            event_sink,
            sessions: Arc::new(Mutex::new(HashMap::new())),
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
            supports_annotations: cfg!(any(
                target_os = "windows",
                all(target_os = "macos", feature = "macos-screencapturekit")
            )),
            annotation_tools: if cfg!(all(target_os = "macos", feature = "macos-screencapturekit"))
            {
                vec![AnnotationToolKind::Pen, AnnotationToolKind::Eraser]
            } else {
                Vec::new()
            },
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
        let bundle = self.publisher_factory.create(&options).await?;
        let publisher = bundle.publisher;
        let webrtc_signaling = bundle.webrtc_signaling;
        let annotation_session = Arc::new(AnnotationSession::default());

        #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
        let pipeline = Arc::new(if options.effective_annotations_enabled() {
            CapturePipeline::new_with_native_annotations(
                Arc::clone(&publisher),
                Arc::clone(&annotation_session),
                options.width.unwrap_or(1280),
                options.height.unwrap_or(720),
            )?
        } else {
            CapturePipeline::new(Arc::clone(&publisher))
        });
        #[cfg(not(all(target_os = "macos", feature = "macos-screencapturekit")))]
        let pipeline = Arc::new(if options.effective_annotations_enabled() {
            CapturePipeline::new_with_annotations(Arc::clone(&publisher))
        } else {
            CapturePipeline::new(Arc::clone(&publisher))
        });

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
        let finish_receiver = running_capture.finish_receiver();

        if let Err(error) = publisher.start(options.clone()).await {
            let _ = running_capture.stop().await;
            let _ = publisher.stop().await;
            return Err(error);
        }

        let overlay = self
            .overlay_factory
            .create_overlay(Arc::clone(&annotation_session));
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
                pipeline: Arc::clone(&pipeline),
                running_capture,
                overlay,
                webrtc_signaling,
                annotation_session: Some(Arc::clone(&annotation_session)),
            },
        );

        if let Some(finish_receiver) = finish_receiver {
            spawn_capture_finish_monitor(
                Arc::clone(&self.sessions),
                session.session_id.clone(),
                finish_receiver,
                Arc::clone(&self.event_sink),
            );
        }

        spawn_annotation_revision_monitor(
            Arc::downgrade(&pipeline),
            annotation_session.subscribe(),
        );

        Ok(session)
    }

    pub async fn set_annotation_interaction(
        &self,
        session_id: &str,
        enabled: bool,
    ) -> Result<AnnotationState> {
        let (annotations, overlay) = self.annotation_resources(session_id).await?;
        let previous_enabled = annotations.state().interaction_enabled;
        let state = annotations.set_interaction_enabled(enabled)?;
        if let Err(error) = overlay.set_annotation_interaction(enabled).await {
            let _ = annotations.set_interaction_enabled(previous_enabled);
            return Err(error);
        }
        self.emit_annotation_state(session_id, state.clone())?;
        Ok(state)
    }

    pub async fn set_annotation_tool(
        &self,
        session_id: &str,
        tool: AnnotationTool,
    ) -> Result<AnnotationState> {
        let (annotations, _) = self.annotation_resources(session_id).await?;
        let state = annotations.set_tool(tool)?;
        self.emit_annotation_state(session_id, state.clone())?;
        Ok(state)
    }

    pub async fn undo_annotation(&self, session_id: &str) -> Result<AnnotationState> {
        let (annotations, overlay) = self.annotation_resources(session_id).await?;
        let state = annotations.undo()?;
        overlay.refresh_annotations().await?;
        self.emit_annotation_state(session_id, state.clone())?;
        Ok(state)
    }

    pub async fn clear_annotations(&self, session_id: &str) -> Result<AnnotationState> {
        let (annotations, overlay) = self.annotation_resources(session_id).await?;
        let state = annotations.clear();
        overlay.refresh_annotations().await?;
        self.emit_annotation_state(session_id, state.clone())?;
        Ok(state)
    }

    pub async fn get_annotation_state(&self, session_id: &str) -> Result<AnnotationState> {
        let (annotations, _) = self.annotation_resources(session_id).await?;
        Ok(annotations.state())
    }

    async fn annotation_resources(
        &self,
        session_id: &str,
    ) -> Result<(Arc<AnnotationSession>, Arc<dyn ShareOverlay>)> {
        let sessions = self.sessions.lock().await;
        let record = sessions.get(session_id).ok_or_else(invalid_session_error)?;
        let annotations = record.annotation_session.clone().ok_or_else(|| {
            Error::new(
                CaptureErrorCode::AnnotationUnavailable,
                "当前会话不支持原生标注",
                true,
            )
        })?;
        Ok((annotations, Arc::clone(&record.overlay)))
    }

    fn emit_annotation_state(&self, session_id: &str, state: AnnotationState) -> Result<()> {
        self.event_sink
            .emit_annotation_state_changed(AnnotationStateChangedEvent {
                session_id: session_id.to_string(),
                state,
            })
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
        let record = self
            .sessions
            .lock()
            .await
            .remove(session_id)
            .ok_or_else(invalid_session_error)?;

        // 从会话表移除后依次清理所有资源，确保任一 stop 失败都不会跳过浮层销毁。
        let capture_result = record.running_capture.stop().await;
        let publisher_result = record.publisher.stop().await;
        if let Err(error) = record.overlay.stop().await {
            eprintln!("[screen-capture] failed to stop share overlay: {error}");
        }

        capture_result?;
        publisher_result?;
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
        let (publisher, pipeline, running_capture) = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| {
                    (
                        Arc::clone(&record.publisher),
                        Arc::clone(&record.pipeline),
                        Arc::clone(&record.running_capture),
                    )
                })
                .ok_or_else(invalid_session_error)?
        };

        let mut stats = pipeline.stats().await;
        let publisher_stats = publisher.stats().await?;
        let capture_stats = running_capture.stats().await?;
        stats.frames_published = stats.frames_published.max(publisher_stats.frames_published);
        stats.frames_capture_dropped = capture_stats.frames_capture_dropped;
        stats.frames_cpu_readback = capture_stats
            .frames_cpu_readback
            .saturating_add(publisher_stats.frames_cpu_readback);
        stats.frames_encoder_dropped = publisher_stats.frames_encoder_dropped;
        stats.frames_dropped = stats
            .frames_capture_dropped
            .saturating_add(stats.frames_pipeline_dropped)
            .saturating_add(stats.frames_encoder_dropped);
        if publisher_stats.frames_published > 0 {
            stats.fps = publisher_stats.fps;
            stats.publish_fps = publisher_stats.publish_fps.max(publisher_stats.fps);
        }
        stats.bitrate_kbps = stats.bitrate_kbps.max(publisher_stats.bitrate_kbps);
        stats.encoder_backend = publisher_stats.encoder_backend;
        stats.started = stats.started || publisher_stats.started;
        Ok(stats)
    }

    pub async fn set_annotation_document(
        &self,
        session_id: &str,
        document: AnnotationDocument,
    ) -> Result<()> {
        let pipeline = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| Arc::clone(&record.pipeline))
                .ok_or_else(invalid_session_error)?
        };
        pipeline.set_annotation_document(document).await
    }

    pub async fn get_annotation_document(&self, session_id: &str) -> Result<AnnotationDocument> {
        let pipeline = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| Arc::clone(&record.pipeline))
                .ok_or_else(invalid_session_error)?
        };
        pipeline.annotation_document()
    }

    pub async fn get_annotation_input_target(
        &self,
        session_id: &str,
    ) -> Result<Option<AnnotationInputTarget>> {
        let overlay = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|record| Arc::clone(&record.overlay))
                .ok_or_else(invalid_session_error)?
        };
        overlay.annotation_input_target().await
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
        record.webrtc_signaling.as_ref().cloned().ok_or_else(|| {
            Error::new(
                CaptureErrorCode::WebRtcNegotiationFailed,
                "capture session was not created with a WebRTC publisher",
                true,
            )
        })
    }
}

fn default_backend() -> Arc<dyn CaptureBackend> {
    #[cfg(target_os = "macos")]
    let backend: Arc<dyn CaptureBackend> = Arc::new(crate::capture::MacOsCaptureBackend);
    #[cfg(target_os = "windows")]
    let backend: Arc<dyn CaptureBackend> = Arc::new(crate::capture::WindowsCaptureBackend);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let backend: Arc<dyn CaptureBackend> = Arc::new(crate::capture::DummyCaptureBackend);
    backend
}

struct DefaultPublisherFactory;

#[async_trait::async_trait]
impl CapturePublisherFactory for DefaultPublisherFactory {
    async fn create(&self, options: &StartCaptureOptions) -> Result<PublisherBundle> {
        match options.effective_publisher_kind() {
            PublisherKind::WebRtcLoopback => {
                let signaling = WebRtcSignalingState::new().await?;
                let publisher = WebRtcPublisher::new(signaling);
                let webrtc_signaling = publisher.signaling();
                Ok(PublisherBundle::new(
                    Arc::new(publisher),
                    Some(webrtc_signaling),
                ))
            }
            PublisherKind::Agora => {
                let agora = options
                    .publisher
                    .as_ref()
                    .and_then(|publisher| publisher.agora.as_ref())
                    .ok_or_else(|| {
                        Error::new(
                            CaptureErrorCode::PublisherUnsupported,
                            "Agora publisher requires appId and channel options",
                            true,
                        )
                    })?;
                Err(AgoraPublisher::sdk_unavailable_error(agora))
            }
        }
    }
}

fn spawn_annotation_revision_monitor(
    pipeline: std::sync::Weak<CapturePipeline>,
    mut revision: watch::Receiver<u64>,
) {
    tokio::spawn(async move {
        while revision.changed().await.is_ok() {
            let Some(pipeline) = pipeline.upgrade() else {
                break;
            };
            if let Err(error) = pipeline.replay_latest_frame().await {
                tracing::debug!(%error, "failed to replay annotation revision");
            }
        }
    });
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

fn spawn_capture_finish_monitor(
    sessions: Arc<Mutex<HashMap<String, SessionRecord>>>,
    session_id: String,
    finish_receiver: watch::Receiver<Option<CaptureFinishReason>>,
    event_sink: Arc<dyn CaptureEventSink>,
) {
    tokio::spawn(async move {
        let finish = wait_for_capture_finish(finish_receiver).await;
        let record = sessions.lock().await.remove(&session_id);

        if let Some(record) = record {
            let error = finish.map(|reason| reason.into_error_payload(record.session.source_kind));
            if let Some(payload) = &error {
                eprintln!(
                    "[screen-capture] capture session {} finished: {}",
                    session_id, payload.message
                );
            }

            if let Err(error) = record.running_capture.stop().await {
                eprintln!("[screen-capture] failed to stop finished capture source: {error}");
            }
            if let Err(error) = record.publisher.stop().await {
                eprintln!(
                    "[screen-capture] failed to stop publisher for finished capture: {error}"
                );
            }
            if let Err(error) = record.overlay.stop().await {
                eprintln!("[screen-capture] failed to stop overlay for finished capture: {error}");
            }
            if let Some(error) = error {
                if let Err(emit_error) =
                    event_sink.emit_session_ended(CaptureSessionEndedEvent { session_id, error })
                {
                    eprintln!("[screen-capture] failed to emit session ended event: {emit_error}");
                }
            }
        }
    });
}

async fn wait_for_capture_finish(
    mut finish_receiver: watch::Receiver<Option<CaptureFinishReason>>,
) -> Option<CaptureFinishReason> {
    loop {
        if let Some(payload) = finish_receiver.borrow().clone() {
            return Some(payload);
        }

        if finish_receiver.changed().await.is_err() {
            return None;
        }
    }
}
