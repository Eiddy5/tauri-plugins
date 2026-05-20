use tauri_plugin_screen_capture::{
    pipeline::frame::VideoFrame,
    publisher::{AgoraPublisher, CapturePublisher, NullPublisher, WebRtcLoopbackPublisher},
    CaptureErrorCode, CaptureSourceKind, CaptureStats, PixelFormat, StartCaptureOptions,
};

fn options() -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "dummy-display-1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(1280),
        height: Some(720),
        capture_cursor: Some(true),
        publisher: None,
    }
}

#[tokio::test]
async fn null_publisher_counts_published_frames() {
    let publisher = NullPublisher::default();
    publisher.start(options()).await.expect("start");
    publisher
        .push_frame(VideoFrame {
            width: 1280,
            height: 720,
            pixel_format: PixelFormat::Rgba,
            timestamp_ns: 1,
            data: vec![0; 4],
        })
        .await
        .expect("push frame");

    let stats: CaptureStats = publisher.stats().await.expect("stats");
    assert_eq!(stats.frames_published, 1);
    assert!(stats.started);
}

#[tokio::test]
async fn agora_publisher_returns_unsupported() {
    let publisher = AgoraPublisher;
    let err = publisher.start(options()).await.expect_err("unsupported");
    assert_eq!(err.payload().code, CaptureErrorCode::PublisherUnsupported);
}

#[tokio::test]
async fn null_publisher_cannot_resume_before_start() {
    let publisher = NullPublisher::default();

    let err = publisher.resume().await.expect_err("resume before start");
    assert_eq!(err.payload().code, CaptureErrorCode::CaptureRuntimeFailed);
}

#[tokio::test]
async fn null_publisher_cannot_resume_after_stop() {
    let publisher = NullPublisher::default();
    publisher.start(options()).await.expect("start");
    publisher.stop().await.expect("stop");

    let err = publisher.resume().await.expect_err("resume after stop");
    assert_eq!(err.payload().code, CaptureErrorCode::CaptureRuntimeFailed);
}

#[tokio::test]
async fn agora_lifecycle_methods_return_unsupported() {
    let publisher = AgoraPublisher;

    for result in [
        publisher.pause().await,
        publisher.resume().await,
        publisher.stop().await,
        publisher.stats().await.map(|_| ()),
    ] {
        let err = result.expect_err("unsupported lifecycle method");
        assert_eq!(err.payload().code, CaptureErrorCode::PublisherUnsupported);
    }
}

#[test]
fn webrtc_loopback_publisher_keeps_distinct_public_type() {
    assert_ne!(
        std::any::TypeId::of::<WebRtcLoopbackPublisher>(),
        std::any::TypeId::of::<NullPublisher>()
    );
}
