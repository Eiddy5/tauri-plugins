pub mod frame;
pub mod stats;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
use std::time::Duration;
use std::time::Instant;

use async_trait::async_trait;
use tokio::{
    sync::{oneshot, Mutex, Notify},
    task::JoinHandle,
};

use crate::{
    annotation::{AnnotationLayer, AnnotationSession},
    capture::FrameConsumer,
    models::{AnnotationDocument, CaptureErrorPayload, CaptureStats},
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    Result,
};

// OpenH264 uses millisecond timestamps, while the native encoders use finer timebases.
// Advancing by one millisecond keeps republished static frames distinct for every backend.
const MIN_PUBLISH_TIMESTAMP_STEP_NS: u64 = 1_000_000;
#[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
const ANNOTATION_BACKPRESSURE_RETRY_DELAY: Duration = Duration::from_millis(4);
#[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
const ANNOTATION_BACKPRESSURE_MAX_ATTEMPTS: usize = 10;

pub struct CapturePipeline {
    publisher: Arc<dyn CapturePublisher>,
    annotations: Option<Arc<AnnotationLayer>>,
    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    native_annotations: Option<Arc<AnnotationSession>>,
    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    mac_compositor: Option<Arc<crate::platform::macos::media::MacMetalCompositor>>,
    stats: Arc<Mutex<CaptureStats>>,
    latest_frame: Mutex<Option<PipelineFrame>>,
    pending_frame: Arc<Mutex<Option<PendingPipelineFrame>>>,
    notify: Arc<Notify>,
    stopped: Arc<AtomicBool>,
    worker: JoinHandle<()>,
    started_at: Mutex<Option<Instant>>,
}

#[derive(Clone)]
enum PipelineFrame {
    Cpu(VideoFrame),
    #[cfg(target_os = "windows")]
    Gpu(crate::platform::windows::media::WindowsGpuSurface),
    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    MacCapture(crate::platform::macos::media::MacCaptureFrame),
    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    MacGpu(crate::platform::macos::media::MacGpuSurface),
}

impl PipelineFrame {
    fn timestamp_ns(&self) -> u64 {
        match self {
            Self::Cpu(frame) => frame.timestamp_ns,
            #[cfg(target_os = "windows")]
            Self::Gpu(surface) => surface.timestamp_ns(),
            #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
            Self::MacCapture(frame) => frame.timestamp_ns(),
            #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
            Self::MacGpu(surface) => surface.timestamp_ns(),
        }
    }

    fn with_timestamp(self, timestamp_ns: u64) -> Self {
        match self {
            Self::Cpu(mut frame) => {
                frame.timestamp_ns = timestamp_ns;
                Self::Cpu(frame)
            }
            #[cfg(target_os = "windows")]
            Self::Gpu(surface) => Self::Gpu(surface.with_timestamp(timestamp_ns)),
            #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
            Self::MacCapture(frame) => Self::MacCapture(frame.with_timestamp(timestamp_ns)),
            #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
            Self::MacGpu(surface) => Self::MacGpu(surface.with_timestamp(timestamp_ns)),
        }
    }
}

struct PendingPipelineFrame {
    frame: PipelineFrame,
    is_capture_frame: bool,
    completions: Vec<oneshot::Sender<std::result::Result<(), CaptureErrorPayload>>>,
}

impl CapturePipeline {
    pub fn new(publisher: Arc<dyn CapturePublisher>) -> Self {
        Self::build(publisher, None)
    }

    pub fn new_with_annotations(publisher: Arc<dyn CapturePublisher>) -> Self {
        Self::build(publisher, Some(Arc::new(AnnotationLayer::default())))
    }

    fn build(
        publisher: Arc<dyn CapturePublisher>,
        annotations: Option<Arc<AnnotationLayer>>,
    ) -> Self {
        let pending_frame = Arc::new(Mutex::new(None));
        let stats = Arc::new(Mutex::new(CaptureStats::default()));
        let notify = Arc::new(Notify::new());
        let stopped = Arc::new(AtomicBool::new(false));
        let worker = spawn_latest_frame_worker(
            Arc::clone(&publisher),
            Arc::clone(&pending_frame),
            Arc::clone(&notify),
            Arc::clone(&stopped),
            annotations.clone(),
            Arc::clone(&stats),
            #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
            None,
            #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
            None,
        );

        Self {
            publisher,
            annotations,
            #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
            native_annotations: None,
            #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
            mac_compositor: None,
            stats,
            latest_frame: Mutex::new(None),
            pending_frame,
            notify,
            stopped,
            worker,
            started_at: Mutex::new(None),
        }
    }

    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    pub fn new_with_native_annotations(
        publisher: Arc<dyn CapturePublisher>,
        annotations: Arc<AnnotationSession>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let compositor = Arc::new(crate::platform::macos::media::MacMetalCompositor::new(
            width, height,
        )?);
        let pending_frame = Arc::new(Mutex::new(None));
        let stats = Arc::new(Mutex::new(CaptureStats::default()));
        let notify = Arc::new(Notify::new());
        let stopped = Arc::new(AtomicBool::new(false));
        let worker = spawn_latest_frame_worker(
            Arc::clone(&publisher),
            Arc::clone(&pending_frame),
            Arc::clone(&notify),
            Arc::clone(&stopped),
            None,
            Arc::clone(&stats),
            Some(Arc::clone(&annotations)),
            Some(Arc::clone(&compositor)),
        );
        Ok(Self {
            publisher,
            annotations: Some(Arc::new(AnnotationLayer::default())),
            native_annotations: Some(annotations),
            mac_compositor: Some(compositor),
            stats,
            latest_frame: Mutex::new(None),
            pending_frame,
            notify,
            stopped,
            worker,
            started_at: Mutex::new(None),
        })
    }

    pub fn annotation_session(&self) -> Option<Arc<AnnotationSession>> {
        #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
        {
            self.native_annotations.clone()
        }
        #[cfg(not(all(target_os = "macos", feature = "macos-screencapturekit")))]
        {
            None
        }
    }

    pub async fn set_annotation_document(&self, document: AnnotationDocument) -> Result<()> {
        self.annotation_layer()?.set_document(document)?;
        self.replay_latest_frame().await
    }

    pub fn annotation_document(&self) -> Result<AnnotationDocument> {
        self.annotation_layer()?.document()
    }

    fn annotation_layer(&self) -> Result<&AnnotationLayer> {
        self.annotations.as_deref().ok_or_else(|| {
            crate::Error::new(
                crate::CaptureErrorCode::InvalidAnnotation,
                "annotations were not enabled when this capture session started",
                true,
            )
        })
    }

    pub async fn stats(&self) -> CaptureStats {
        let mut stats = self.stats.lock().await.clone();
        #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
        if let Some(annotations) = self.native_annotations.as_ref() {
            let state = annotations.state();
            stats.annotation_operation_count = state.operation_count as u64;
            stats.annotation_revision = state.revision;
        }
        if let Some(started_at) = *self.started_at.lock().await {
            let elapsed = started_at.elapsed().as_secs_f64();
            if elapsed > 0.0 {
                stats.fps = stats.frames_captured as f64 / elapsed;
                stats.capture_fps = stats.fps;
            }
        }
        stats
    }

    pub async fn replay_latest_frame(&self) -> Result<()> {
        let Some(frame) = self.latest_frame.lock().await.clone() else {
            return Ok(());
        };
        let (completion, published) = oneshot::channel();
        self.enqueue_frame(frame, Some(completion), false).await;
        match published.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(payload)) => Err(crate::Error::Structured { payload }),
            Err(error) => Err(crate::Error::new(
                crate::CaptureErrorCode::CaptureRuntimeFailed,
                format!("annotation frame publication was cancelled: {error}"),
                true,
            )),
        }
    }
}

impl Drop for CapturePipeline {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.notify.notify_waiters();
        self.worker.abort();
    }
}

#[async_trait]
impl FrameConsumer for CapturePipeline {
    fn supports_gpu_surfaces(&self) -> bool {
        self.annotations.is_none() && self.publisher.supports_gpu_surfaces()
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.push_pipeline_frame(PipelineFrame::Cpu(frame)).await
    }

    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    fn supports_mac_capture_frames(&self) -> bool {
        self.mac_compositor.is_some() && self.publisher.supports_mac_gpu_surfaces()
    }

    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    async fn push_mac_capture_frame(
        &self,
        frame: crate::platform::macos::media::MacCaptureFrame,
    ) -> Result<()> {
        self.push_pipeline_frame(PipelineFrame::MacCapture(frame))
            .await
    }

    #[cfg(target_os = "windows")]
    async fn push_gpu_surface(
        &self,
        surface: crate::platform::windows::media::WindowsGpuSurface,
    ) -> Result<()> {
        self.push_pipeline_frame(PipelineFrame::Gpu(surface)).await
    }
}

impl CapturePipeline {
    async fn push_pipeline_frame(&self, frame: PipelineFrame) -> Result<()> {
        let mut started_at = self.started_at.lock().await;
        if started_at.is_none() {
            *started_at = Some(Instant::now());
        }
        drop(started_at);
        {
            let mut stats = self.stats.lock().await;
            stats.frames_captured += 1;
            stats.started = true;
            if stats.frames_captured == 1 || stats.frames_captured % 30 == 0 {
                eprintln!(
                    "[screen-capture] pipeline captured frame count={}",
                    stats.frames_captured
                );
            }
        }
        *self.latest_frame.lock().await = Some(frame.clone());
        self.enqueue_frame(frame, None, true).await;

        Ok(())
    }

    async fn enqueue_frame(
        &self,
        frame: PipelineFrame,
        completion: Option<oneshot::Sender<std::result::Result<(), CaptureErrorPayload>>>,
        is_capture_frame: bool,
    ) {
        let replaced_capture_frame = {
            let mut pending = self.pending_frame.lock().await;
            if let Some(pending) = pending.as_mut() {
                if let Some(completion) = completion {
                    pending.completions.push(completion);
                }
                if is_capture_frame {
                    let replaced_capture_frame = pending.is_capture_frame;
                    pending.frame = frame;
                    pending.is_capture_frame = true;
                    replaced_capture_frame
                } else {
                    false
                }
            } else {
                *pending = Some(PendingPipelineFrame {
                    frame,
                    is_capture_frame,
                    completions: completion.into_iter().collect(),
                });
                false
            }
        };
        if replaced_capture_frame {
            let mut stats = self.stats.lock().await;
            stats.frames_pipeline_dropped += 1;
            stats.frames_dropped = stats.frames_pipeline_dropped;
        }
        self.notify.notify_one();
    }
}

#[async_trait]
impl FrameConsumer for Arc<CapturePipeline> {
    fn supports_gpu_surfaces(&self) -> bool {
        self.as_ref().supports_gpu_surfaces()
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.as_ref().push_frame(frame).await
    }

    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    fn supports_mac_capture_frames(&self) -> bool {
        self.as_ref().supports_mac_capture_frames()
    }

    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    async fn push_mac_capture_frame(
        &self,
        frame: crate::platform::macos::media::MacCaptureFrame,
    ) -> Result<()> {
        self.as_ref().push_mac_capture_frame(frame).await
    }

    #[cfg(target_os = "windows")]
    async fn push_gpu_surface(
        &self,
        surface: crate::platform::windows::media::WindowsGpuSurface,
    ) -> Result<()> {
        self.as_ref().push_gpu_surface(surface).await
    }
}

fn spawn_latest_frame_worker(
    publisher: Arc<dyn CapturePublisher>,
    pending_frame: Arc<Mutex<Option<PendingPipelineFrame>>>,
    notify: Arc<Notify>,
    stopped: Arc<AtomicBool>,
    annotations: Option<Arc<AnnotationLayer>>,
    stats: Arc<Mutex<CaptureStats>>,
    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))] native_annotations: Option<
        Arc<AnnotationSession>,
    >,
    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))] mac_compositor: Option<
        Arc<crate::platform::macos::media::MacMetalCompositor>,
    >,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        #[cfg(not(all(target_os = "macos", feature = "macos-screencapturekit")))]
        let _ = &stats;
        let mut last_timestamp_ns: Option<u64> = None;
        loop {
            notify.notified().await;

            while let Some(pending) = pending_frame.lock().await.take() {
                if stopped.load(Ordering::Acquire) {
                    return;
                }
                let captured_timestamp_ns = pending.frame.timestamp_ns();
                let timestamp_ns = last_timestamp_ns.map_or(captured_timestamp_ns, |last| {
                    captured_timestamp_ns.max(last.saturating_add(MIN_PUBLISH_TIMESTAMP_STEP_NS))
                });
                last_timestamp_ns = Some(timestamp_ns);
                #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
                let was_mac_capture = matches!(&pending.frame, PipelineFrame::MacCapture(_));
                let frame = pending.frame.with_timestamp(timestamp_ns);
                let _compose_started = Instant::now();
                #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
                let composed = {
                    let mut attempt = 0;
                    loop {
                        attempt += 1;
                        let result = composite_annotations(
                            &annotations,
                            frame.clone(),
                            &native_annotations,
                            &mac_compositor,
                        );
                        let should_retry = !pending.is_capture_frame
                            && attempt < ANNOTATION_BACKPRESSURE_MAX_ATTEMPTS
                            && result.as_ref().err().is_some_and(
                                crate::platform::macos::media::is_surface_backpressure,
                            );
                        if !should_retry {
                            break result;
                        }
                        tokio::time::sleep(ANNOTATION_BACKPRESSURE_RETRY_DELAY).await;
                    }
                };
                #[cfg(not(all(target_os = "macos", feature = "macos-screencapturekit")))]
                let composed = composite_annotations(&annotations, frame);
                #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
                {
                    let mut current = stats.lock().await;
                    match &composed {
                        Ok(PipelineFrame::MacGpu(_)) => {
                            current.frames_composited += 1;
                            current.gpu_compose_latency_ms =
                                _compose_started.elapsed().as_secs_f64() * 1_000.0;
                            if !pending.is_capture_frame {
                                current.frames_annotation_triggered += 1;
                                current.frames_static_repeated += 1;
                            }
                        }
                        Err(_) if was_mac_capture => {
                            current.frames_gpu_backpressure_dropped += 1;
                            current.frames_pipeline_dropped += 1;
                            current.frames_dropped = current.frames_pipeline_dropped;
                        }
                        _ => {}
                    }
                }
                let result = match composed {
                    Ok(frame) => publish_pipeline_frame(&publisher, frame).await,
                    Err(error) => Err(error),
                };
                let completion_result = match result {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
                        let expected_capture_backpressure = pending.is_capture_frame
                            && crate::platform::macos::media::is_surface_backpressure(&error);
                        #[cfg(not(all(target_os = "macos", feature = "macos-screencapturekit")))]
                        let expected_capture_backpressure = false;
                        if !expected_capture_backpressure {
                            eprintln!("[screen-capture] pipeline publish failed: {error:?}");
                        }
                        Err(error.payload())
                    }
                };
                for completion in pending.completions {
                    let _ = completion.send(completion_result.clone());
                }
            }

            if stopped.load(Ordering::Acquire) {
                return;
            }
        }
    })
}

fn composite_annotations(
    annotations: &Option<Arc<AnnotationLayer>>,
    mut frame: PipelineFrame,
    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    native_annotations: &Option<Arc<AnnotationSession>>,
    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))] mac_compositor: &Option<
        Arc<crate::platform::macos::media::MacMetalCompositor>,
    >,
) -> Result<PipelineFrame> {
    if let (Some(annotations), PipelineFrame::Cpu(cpu_frame)) = (annotations, &mut frame) {
        *cpu_frame = annotations.composite(cpu_frame.clone())?;
    }
    #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
    if let PipelineFrame::MacCapture(source) = frame {
        let native_annotations = native_annotations.as_ref().ok_or_else(|| {
            crate::Error::new(
                crate::CaptureErrorCode::AnnotationUnavailable,
                "native annotation session is unavailable",
                true,
            )
        })?;
        let compositor = mac_compositor.as_ref().ok_or_else(|| {
            crate::Error::new(
                crate::CaptureErrorCode::AnnotationUnavailable,
                "native annotation compositor is unavailable",
                true,
            )
        })?;
        let (document_revision, document) = annotations
            .as_ref()
            .map(|annotations| annotations.document_snapshot())
            .transpose()?
            .map(|(revision, document)| (revision, Some(document)))
            .unwrap_or((0, None));
        return compositor
            .compose_with_document(
                &source,
                &native_annotations.snapshot(),
                document_revision,
                document.as_deref(),
            )
            .map(PipelineFrame::MacGpu);
    }
    Ok(frame)
}

async fn publish_pipeline_frame(
    publisher: &Arc<dyn CapturePublisher>,
    frame: PipelineFrame,
) -> Result<()> {
    match frame {
        PipelineFrame::Cpu(frame) => publisher.push_frame(frame).await,
        #[cfg(target_os = "windows")]
        PipelineFrame::Gpu(surface) => publisher.push_gpu_surface(surface).await,
        #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
        PipelineFrame::MacGpu(surface) => publisher.push_mac_gpu_surface(surface).await,
        #[cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]
        PipelineFrame::MacCapture(_) => Err(crate::Error::new(
            crate::CaptureErrorCode::AnnotationUnavailable,
            "uncomposited macOS frame reached publisher",
            true,
        )),
    }
}
