use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tauri_plugin_screen_capture::{
    pipeline::frame::VideoFrame,
    publisher::{
        AgoraExternalVideoFrame, AgoraExternalVideoPixelFormat, AgoraPublisher, AgoraVideoSink,
        CapturePublisher,
    },
    AgoraPublisherOptions, CaptureStats, PixelFormat, Result, StartCaptureOptions,
};

fn frame() -> VideoFrame {
    VideoFrame {
        width: 2,
        height: 1,
        pixel_format: PixelFormat::Bgra,
        timestamp_ns: 42_000,
        data: Arc::from([1, 2, 3, 4, 5, 6, 7, 8]),
    }
}

fn options() -> AgoraPublisherOptions {
    AgoraPublisherOptions {
        app_id: "app-id".to_string(),
        channel: "demo-channel".to_string(),
        uid: Some(42),
        token: Some("token".to_string()),
    }
}

fn start_options() -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "display:1".to_string(),
        source_kind: tauri_plugin_screen_capture::CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(2),
        height: Some(1),
        capture_cursor: Some(true),
        publisher: None,
    }
}

#[test]
fn agora_external_frame_maps_tight_bgra_video_frame() {
    let input = frame();
    let expected_data = Arc::clone(&input.data);

    let external = AgoraExternalVideoFrame::try_from(input).expect("external frame");

    assert_eq!(external.width, 2);
    assert_eq!(external.height, 1);
    assert_eq!(external.stride, 8);
    assert_eq!(external.pixel_format, AgoraExternalVideoPixelFormat::Bgra);
    assert_eq!(external.timestamp_us, 42);
    assert!(Arc::ptr_eq(&external.data, &expected_data));
}

#[tokio::test]
async fn agora_publisher_pushes_external_frames_through_injected_sink() {
    let sink = Arc::new(RecordingSink::default());
    let publisher = AgoraPublisher::with_sink(options(), sink.clone());

    publisher.start(start_options()).await.expect("start");
    publisher.push_frame(frame()).await.expect("push");

    assert_eq!(sink.pushed.lock().unwrap().as_slice(), &[(2, 1, 8, 42)]);

    let stats = publisher.stats().await.expect("stats");
    assert_eq!(stats.frames_published, 1);
    assert_eq!(stats.frames_dropped, 0);
    assert!(stats.started);
}

#[derive(Debug, Default)]
struct RecordingSink {
    started: Mutex<bool>,
    pushed: Mutex<Vec<(u32, u32, u32, u64)>>,
}

#[async_trait]
impl AgoraVideoSink for RecordingSink {
    async fn start(
        &self,
        _agora: &AgoraPublisherOptions,
        _capture: &StartCaptureOptions,
    ) -> Result<()> {
        *self.started.lock().unwrap() = true;
        Ok(())
    }

    async fn push_external_video_frame(&self, frame: AgoraExternalVideoFrame) -> Result<()> {
        assert!(*self.started.lock().unwrap());
        self.pushed.lock().unwrap().push((
            frame.width,
            frame.height,
            frame.stride,
            frame.timestamp_us,
        ));
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        *self.started.lock().unwrap() = false;
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(CaptureStats::default())
    }
}
