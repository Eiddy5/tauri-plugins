use tauri_plugin_screen_capture::{
    capture::{CaptureBackend, DummyCaptureBackend, FrameConsumer, RunningCapture},
    webrtc::{
        signaling::WebRtcSignalingState,
        track::{EncodedVideoSample, WebRtcH264SampleSender},
    },
    CaptureErrorCode, CaptureSource, CaptureSourceKind, ListSourcesOptions, PermissionStatus,
    ScreenCaptureState, StartCaptureOptions,
};

use async_trait::async_trait;
use std::time::Duration;

fn start_options() -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "display:1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(2),
        height: Some(1),
        capture_cursor: Some(true),
    }
}

struct EmptyBackend;

#[async_trait]
impl CaptureBackend for EmptyBackend {
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
        _consumer: Box<dyn FrameConsumer>,
    ) -> tauri_plugin_screen_capture::Result<Box<dyn RunningCapture>> {
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

#[tokio::test]
async fn signaling_creates_offer_for_h264_video_track_only() {
    let signaling = WebRtcSignalingState::new().await.expect("signaling");

    let offer = signaling.create_offer().await.expect("offer");

    assert_eq!(offer.sdp_type, "offer");
    assert!(offer.sdp.contains("m=video"));
    assert!(offer.sdp.contains("H264/90000"));
    assert!(!offer.sdp.contains("m=application"));
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
async fn state_exposes_webrtc_offer_for_capture_sessions() {
    let state = ScreenCaptureState::with_backend(std::sync::Arc::new(EmptyBackend));
    let session = state.start_capture(start_options()).await.expect("start");

    let offer = state
        .create_webrtc_offer(&session.session_id)
        .await
        .expect("offer");

    assert_eq!(offer.sdp_type, "offer");
    assert!(offer.sdp.contains("m=video"));
    assert!(offer.sdp.contains("H264/90000"));
    assert!(!offer.sdp.contains("m=application"));
}
