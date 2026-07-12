use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use tauri_plugin_screen_capture::{
    capture::FrameConsumer,
    pipeline::{frame::VideoFrame, CapturePipeline},
    publisher::CapturePublisher,
    CaptureSourceKind, CaptureStats, PixelFormat, Result, StartCaptureOptions,
};

fn frame(timestamp_ns: u64) -> VideoFrame {
    VideoFrame {
        width: 2,
        height: 2,
        pixel_format: PixelFormat::Bgra,
        timestamp_ns,
        data: Arc::from([0; 16]),
    }
}

#[derive(Debug)]
struct SlowPublisher {
    pushes: AtomicUsize,
    delay: Duration,
    supports_gpu_surfaces: bool,
}

impl SlowPublisher {
    fn new(delay: Duration) -> Self {
        Self {
            pushes: AtomicUsize::new(0),
            delay,
            supports_gpu_surfaces: false,
        }
    }

    fn with_gpu_surfaces(mut self) -> Self {
        self.supports_gpu_surfaces = true;
        self
    }
}

#[async_trait]
impl CapturePublisher for SlowPublisher {
    fn supports_gpu_surfaces(&self) -> bool {
        self.supports_gpu_surfaces
    }

    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        Ok(())
    }

    async fn push_frame(&self, _frame: VideoFrame) -> Result<()> {
        tokio::time::sleep(self.delay).await;
        self.pushes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(CaptureStats::default())
    }
}

#[tokio::test]
async fn pipeline_accepts_frames_without_waiting_for_slow_publisher() {
    let publisher = Arc::new(SlowPublisher::new(Duration::from_millis(200)));
    let pipeline = CapturePipeline::new(publisher);

    tokio::time::timeout(Duration::from_millis(50), pipeline.push_frame(frame(1)))
        .await
        .expect("pipeline should not block capture on slow publishing")
        .expect("push frame");
}

#[tokio::test]
async fn pipeline_reports_replaced_frames_as_pipeline_stage_drops() {
    let publisher = Arc::new(SlowPublisher::new(Duration::from_millis(200)));
    let pipeline = CapturePipeline::new(publisher);

    pipeline.push_frame(frame(1)).await.expect("first frame");
    pipeline.push_frame(frame(2)).await.expect("second frame");
    pipeline.push_frame(frame(3)).await.expect("third frame");

    let stats = pipeline.stats().await;
    assert!(stats.frames_pipeline_dropped >= 1);
    assert_eq!(stats.frames_dropped, stats.frames_pipeline_dropped);
    assert!(stats.capture_fps > 0.0);
}

#[allow(dead_code)]
fn start_options() -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "display:1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(2),
        height: Some(2),
        capture_cursor: Some(true),
        publisher: None,
    }
}

#[tokio::test]
async fn pipeline_advertises_gpu_surfaces_only_when_publisher_supports_them() {
    let publisher = Arc::new(SlowPublisher::new(Duration::from_millis(1)).with_gpu_surfaces());
    let pipeline = CapturePipeline::new(publisher);

    assert!(pipeline.supports_gpu_surfaces());
}
