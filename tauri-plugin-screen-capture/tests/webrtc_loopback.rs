use tauri_plugin_screen_capture::{
    capture::{CaptureBackend, DummyCaptureBackend, FrameConsumer, RunningCapture},
    pipeline::frame::VideoFrame,
    publisher::{CapturePublisher, WebRtcLoopbackPublisher},
    webrtc::{
        signaling::WebRtcSignalingState,
        track::{
            EncodedVideoSample, WebRtcFramePacket, WebRtcH264SampleSender,
            MAX_DATA_CHANNEL_MESSAGE_BYTES,
        },
    },
    CaptureErrorCode, CapturePublisherKind, CaptureSource, CaptureSourceKind, ListSourcesOptions,
    PermissionStatus, PixelFormat, ScreenCaptureState, StartCaptureOptions,
};

use async_trait::async_trait;
use std::time::Duration;

fn frame() -> VideoFrame {
    VideoFrame {
        width: 2,
        height: 1,
        pixel_format: PixelFormat::Bgra,
        timestamp_ns: 42,
        data: vec![1, 2, 3, 4, 5, 6, 7, 8],
    }
}

fn start_options(publisher: CapturePublisherKind) -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "display:1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(2),
        height: Some(1),
        capture_cursor: Some(true),
        publisher: Some(publisher),
    }
}

struct FramePushingBackend;

#[async_trait]
impl CaptureBackend for FramePushingBackend {
    async fn check_permission(&self) -> tauri_plugin_screen_capture::Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    async fn request_permission(&self) -> tauri_plugin_screen_capture::Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    async fn list_sources(
        &self,
        options: ListSourcesOptions,
    ) -> tauri_plugin_screen_capture::Result<Vec<CaptureSource>> {
        DummyCaptureBackend.list_sources(options).await
    }

    async fn start_capture(
        &self,
        _options: StartCaptureOptions,
        consumer: Box<dyn FrameConsumer>,
    ) -> tauri_plugin_screen_capture::Result<Box<dyn RunningCapture>> {
        consumer.push_frame(frame()).await?;
        Ok(Box::new(DummyRunningCapture))
    }
}

struct DummyRunningCapture;

#[async_trait]
impl RunningCapture for DummyRunningCapture {
    async fn pause(&self) -> tauri_plugin_screen_capture::Result<()> {
        Ok(())
    }

    async fn resume(&self) -> tauri_plugin_screen_capture::Result<()> {
        Ok(())
    }

    async fn stop(&self) -> tauri_plugin_screen_capture::Result<()> {
        Ok(())
    }
}

#[test]
fn frame_packet_serializes_metadata_and_payload_for_browser_renderer() {
    let packet = WebRtcFramePacket::from_frame(frame()).expect("packet");
    let bytes = packet.into_bytes();

    assert_eq!(&bytes[0..4], b"SCF1");
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 1);
    assert_eq!(bytes[12], 0);
    assert_eq!(u64::from_le_bytes(bytes[13..21].try_into().unwrap()), 42);
    assert_eq!(u32::from_le_bytes(bytes[21..25].try_into().unwrap()), 8);
    assert_eq!(&bytes[25..], &[1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn frame_packet_rejects_empty_frames() {
    let mut empty = frame();
    empty.data.clear();

    let error = WebRtcFramePacket::from_frame(empty).expect_err("empty frame should fail");
    assert_eq!(error.payload().code, CaptureErrorCode::WebRtcTrackFailed);
}

#[test]
fn large_frame_packet_is_split_into_bounded_data_channel_messages() {
    let mut large = frame();
    large.width = 1920;
    large.height = 1080;
    large.data = vec![7; 1920 * 1080 * 4];

    let chunks = WebRtcFramePacket::from_frame(large)
        .expect("packet")
        .into_chunks();

    assert!(chunks.len() > 1);
    assert!(chunks
        .iter()
        .all(|chunk| chunk.len() <= MAX_DATA_CHANNEL_MESSAGE_BYTES));
}

#[test]
fn frame_chunks_fit_conservative_browser_data_channel_limit() {
    assert!(MAX_DATA_CHANNEL_MESSAGE_BYTES <= 16 * 1024);
}

#[tokio::test]
async fn signaling_creates_offer_for_h264_video_track() {
    let signaling = WebRtcSignalingState::new().await.expect("signaling");

    let offer = signaling.create_offer().await.expect("offer");

    assert_eq!(offer.sdp_type, "offer");
    assert!(offer.sdp.contains("m=video"));
    assert!(offer.sdp.contains("H264/90000"));
}

#[tokio::test]
async fn signaling_keeps_frame_data_channel_for_diagnostics() {
    let signaling = WebRtcSignalingState::new().await.expect("signaling");

    let offer = signaling.create_offer().await.expect("offer");

    assert!(offer.sdp.contains("m=application"));
    assert!(offer.sdp.contains("UDP/DTLS/SCTP"));
    assert!(offer.sdp.contains("a=candidate:"));
}

#[tokio::test]
async fn h264_sample_sender_rejects_empty_encoded_samples() {
    let signaling = WebRtcSignalingState::new().await.expect("signaling");
    let sender = WebRtcH264SampleSender::new(signaling.video_track());

    let error = sender
        .send_sample(EncodedVideoSample {
            data: Vec::new(),
            duration: Duration::from_millis(33),
            timestamp_ns: 0,
        })
        .await
        .expect_err("empty encoded sample should fail");

    assert_eq!(error.payload().code, CaptureErrorCode::WebRtcTrackFailed);
}

#[tokio::test]
async fn publisher_drops_frames_until_webrtc_data_channel_is_open() {
    let signaling = WebRtcSignalingState::new().await.expect("signaling");
    let publisher = WebRtcLoopbackPublisher::new(signaling);

    publisher
        .start(start_options(CapturePublisherKind::WebRtcLoopback))
        .await
        .expect("start");
    publisher
        .push_frame(frame())
        .await
        .expect("drop pending frame");
    let stats = publisher.stats().await.expect("stats");

    assert_eq!(stats.frames_published, 0);
    assert_eq!(stats.frames_dropped, 1);
    assert!(stats.started);
}

#[tokio::test]
async fn publisher_drops_frames_while_paused_or_stopped() {
    let signaling = WebRtcSignalingState::new().await.expect("signaling");
    let publisher = WebRtcLoopbackPublisher::new(signaling);

    publisher
        .start(start_options(CapturePublisherKind::WebRtcLoopback))
        .await
        .expect("start");
    publisher.pause().await.expect("pause");
    publisher.push_frame(frame()).await.expect("drop paused");
    publisher.stop().await.expect("stop");
    publisher.push_frame(frame()).await.expect("drop stopped");
    let stats = publisher.stats().await.expect("stats");

    assert_eq!(stats.frames_published, 0);
    assert_eq!(stats.frames_dropped, 2);
    assert!(!stats.started);
}

#[tokio::test]
async fn state_exposes_webrtc_offer_for_loopback_sessions() {
    let state = ScreenCaptureState::with_backend(std::sync::Arc::new(DummyCaptureBackend));
    let session = state
        .start_capture(start_options(CapturePublisherKind::WebRtcLoopback))
        .await
        .expect("start");

    let offer = state
        .create_webrtc_offer(&session.session_id)
        .await
        .expect("offer");

    assert_eq!(offer.sdp_type, "offer");
    assert!(offer.sdp.contains("m=video"));
    assert!(offer.sdp.contains("H264/90000"));
    assert!(offer.sdp.contains("m=application"));
    assert!(offer.sdp.contains("a=candidate:"));
}

#[tokio::test]
async fn state_stats_do_not_count_unopened_webrtc_channel_as_published() {
    let state = ScreenCaptureState::with_backend(std::sync::Arc::new(FramePushingBackend));
    let session = state
        .start_capture(start_options(CapturePublisherKind::WebRtcLoopback))
        .await
        .expect("start");

    let stats = state
        .get_capture_stats(&session.session_id)
        .await
        .expect("stats");

    assert_eq!(stats.frames_captured, 1);
    assert_eq!(stats.frames_published, 0);
    assert_eq!(stats.frames_dropped, 1);
}

#[tokio::test]
async fn state_rejects_webrtc_offer_for_non_webrtc_sessions() {
    let state = ScreenCaptureState::with_backend(std::sync::Arc::new(DummyCaptureBackend));
    let session = state
        .start_capture(start_options(CapturePublisherKind::Null))
        .await
        .expect("start");

    let error = state
        .create_webrtc_offer(&session.session_id)
        .await
        .expect_err("null publisher should not expose webrtc");

    assert_eq!(error.payload().code, CaptureErrorCode::PublisherUnsupported);
}
