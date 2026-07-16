pub mod frame;
pub mod stats;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

use async_trait::async_trait;
use tokio::{
    sync::{oneshot, Mutex, Notify},
    task::JoinHandle,
};

use crate::{
    annotation::AnnotationLayer,
    capture::FrameConsumer,
    models::{AnnotationDocument, CaptureErrorPayload, CaptureStats},
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    Result,
};

// OpenH264 uses millisecond timestamps, while the native encoders use finer timebases.
// Advancing by one millisecond keeps republished static frames distinct for every backend.
const MIN_PUBLISH_TIMESTAMP_STEP_NS: u64 = 1_000_000;

pub struct CapturePipeline {
    publisher: Arc<dyn CapturePublisher>,
    annotations: Option<Arc<AnnotationLayer>>,
    stats: Mutex<CaptureStats>,
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
}

impl PipelineFrame {
    fn timestamp_ns(&self) -> u64 {
        match self {
            Self::Cpu(frame) => frame.timestamp_ns,
            #[cfg(target_os = "windows")]
            Self::Gpu(surface) => surface.timestamp_ns(),
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
        let notify = Arc::new(Notify::new());
        let stopped = Arc::new(AtomicBool::new(false));
        let worker = spawn_latest_frame_worker(
            Arc::clone(&publisher),
            Arc::clone(&pending_frame),
            Arc::clone(&notify),
            Arc::clone(&stopped),
            annotations.clone(),
        );

        Self {
            publisher,
            annotations,
            stats: Mutex::new(CaptureStats::default()),
            latest_frame: Mutex::new(None),
            pending_frame,
            notify,
            stopped,
            worker,
            started_at: Mutex::new(None),
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
) -> JoinHandle<()> {
    tokio::spawn(async move {
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
                let frame = pending.frame.with_timestamp(timestamp_ns);
                let result = match composite_annotations(&annotations, frame) {
                    Ok(frame) => publish_pipeline_frame(&publisher, frame).await,
                    Err(error) => Err(error),
                };
                let completion_result = match result {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        eprintln!("[screen-capture] pipeline publish failed: {error:?}");
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
) -> Result<PipelineFrame> {
    if let (Some(annotations), PipelineFrame::Cpu(cpu_frame)) = (annotations, &mut frame) {
        *cpu_frame = annotations.composite(cpu_frame.clone())?;
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
    }
}
