pub mod frame;
pub mod stats;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

use async_trait::async_trait;
use tokio::{
    sync::{Mutex, Notify},
    task::JoinHandle,
};

use crate::{
    capture::FrameConsumer, models::CaptureStats, pipeline::frame::VideoFrame,
    publisher::CapturePublisher, Result,
};

pub struct CapturePipeline {
    publisher: Arc<dyn CapturePublisher>,
    stats: Mutex<CaptureStats>,
    latest_frame: Mutex<Option<PipelineFrame>>,
    pending_frame: Arc<Mutex<Option<PipelineFrame>>>,
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

impl CapturePipeline {
    pub fn new(publisher: Arc<dyn CapturePublisher>) -> Self {
        let pending_frame = Arc::new(Mutex::new(None));
        let notify = Arc::new(Notify::new());
        let stopped = Arc::new(AtomicBool::new(false));
        let worker = spawn_latest_frame_worker(
            Arc::clone(&publisher),
            Arc::clone(&pending_frame),
            Arc::clone(&notify),
            Arc::clone(&stopped),
        );

        Self {
            publisher,
            stats: Mutex::new(CaptureStats::default()),
            latest_frame: Mutex::new(None),
            pending_frame,
            notify,
            stopped,
            worker,
            started_at: Mutex::new(None),
        }
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

        publish_pipeline_frame(&self.publisher, frame).await
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
        self.publisher.supports_gpu_surfaces()
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
        {
            let mut pending_frame = self.pending_frame.lock().await;
            if pending_frame.replace(frame).is_some() {
                let mut stats = self.stats.lock().await;
                stats.frames_pipeline_dropped += 1;
                stats.frames_dropped = stats.frames_pipeline_dropped;
            }
        }
        self.notify.notify_one();

        Ok(())
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
    pending_frame: Arc<Mutex<Option<PipelineFrame>>>,
    notify: Arc<Notify>,
    stopped: Arc<AtomicBool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            notify.notified().await;

            while let Some(frame) = pending_frame.lock().await.take() {
                if stopped.load(Ordering::Acquire) {
                    return;
                }
                if let Err(error) = publish_pipeline_frame(&publisher, frame).await {
                    eprintln!("[screen-capture] pipeline publish failed: {error:?}");
                }
            }

            if stopped.load(Ordering::Acquire) {
                return;
            }
        }
    })
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
