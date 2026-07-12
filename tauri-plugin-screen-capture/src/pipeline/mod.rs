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
    latest_frame: Mutex<Option<VideoFrame>>,
    pending_frame: Arc<Mutex<Option<VideoFrame>>>,
    notify: Arc<Notify>,
    stopped: Arc<AtomicBool>,
    worker: JoinHandle<()>,
    started_at: Mutex<Option<Instant>>,
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

        self.publisher.push_frame(frame).await
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
    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
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
    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.as_ref().push_frame(frame).await
    }
}

fn spawn_latest_frame_worker(
    publisher: Arc<dyn CapturePublisher>,
    pending_frame: Arc<Mutex<Option<VideoFrame>>>,
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
                if let Err(error) = publisher.push_frame(frame).await {
                    eprintln!("[screen-capture] pipeline publish failed: {error:?}");
                }
            }

            if stopped.load(Ordering::Acquire) {
                return;
            }
        }
    })
}
