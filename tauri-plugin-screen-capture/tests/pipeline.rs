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
}

impl SlowPublisher {
    fn new(delay: Duration) -> Self {
        Self {
            pushes: AtomicUsize::new(0),
            delay,
        }
    }
}

#[async_trait]
impl CapturePublisher for SlowPublisher {
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
