use tauri_plugin_screen_capture::*;

#[test]
fn list_sources_options_defaults_are_safe_for_user_ui() {
    let options = ListSourcesOptions::default();
    assert_eq!(
        options.kinds,
        vec![CaptureSourceKind::Display, CaptureSourceKind::Window]
    );
    assert!(!options.include_thumbnails);
    assert!(!options.include_current_app);
    assert!(!options.include_system_ui);
    assert!(!options.debug_raw_sources);
}

#[test]
fn list_sources_options_deserializes_missing_fields_from_frontend() {
    let options: ListSourcesOptions = serde_json::from_value(serde_json::json!({}))
        .expect("deserialize empty list sources options");

    assert_eq!(
        options.kinds,
        vec![CaptureSourceKind::Display, CaptureSourceKind::Window]
    );
    assert!(!options.include_thumbnails);
    assert!(!options.include_current_app);
    assert!(!options.include_system_ui);
    assert!(!options.debug_raw_sources);
}

#[test]
fn list_sources_options_deserializes_partial_frontend_options() {
    let options: ListSourcesOptions = serde_json::from_value(serde_json::json!({
        "includeCurrentApp": true
    }))
    .expect("deserialize partial list sources options");

    assert_eq!(
        options.kinds,
        vec![CaptureSourceKind::Display, CaptureSourceKind::Window]
    );
    assert!(!options.include_thumbnails);
    assert!(options.include_current_app);
    assert!(!options.include_system_ui);
    assert!(!options.debug_raw_sources);
}

#[test]
fn start_capture_options_use_video_defaults() {
    let options = StartCaptureOptions {
        source_id: "display:1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: None,
        width: None,
        height: None,
        capture_cursor: None,
        publisher: None,
    };

    assert_eq!(options.effective_fps(), 30);
    assert!(options.effective_capture_cursor());
    assert_eq!(options.effective_video_size(), (1280, 720));
}

#[test]
fn start_capture_options_normalize_video_size_for_h264() {
    let options = StartCaptureOptions {
        source_id: "window:abc".to_string(),
        source_kind: CaptureSourceKind::Window,
        fps: Some(60),
        width: Some(853),
        height: Some(481),
        capture_cursor: Some(true),
        publisher: None,
    };

    assert_eq!(options.effective_video_size(), (852, 480));
}

#[test]
fn capture_error_serializes_stable_code() {
    let error = CaptureErrorPayload {
        code: CaptureErrorCode::PermissionDenied,
        message: "Screen Recording permission is denied".to_string(),
        recoverable: true,
        details: None,
    };

    let json = serde_json::to_value(error).expect("serialize error payload");
    assert_eq!(json["code"], "permissionDenied");
    assert_eq!(json["recoverable"], true);
}

#[test]
fn webrtc_error_codes_use_documented_wire_names() {
    let negotiation = serde_json::to_value(CaptureErrorCode::WebRtcNegotiationFailed)
        .expect("serialize negotiation error code");
    assert_eq!(negotiation, "webrtcNegotiationFailed");

    let track =
        serde_json::to_value(CaptureErrorCode::WebRtcTrackFailed).expect("serialize track error");
    assert_eq!(track, "webrtcTrackFailed");

    let parsed: CaptureErrorCode =
        serde_json::from_str("\"webrtcTrackFailed\"").expect("deserialize track error");
    assert_eq!(parsed, CaptureErrorCode::WebRtcTrackFailed);
}

#[test]
fn capture_stats_deserializes_legacy_payload_with_zeroed_stage_metrics() {
    let stats: CaptureStats = serde_json::from_value(serde_json::json!({
        "framesCaptured": 120,
        "framesPublished": 110,
        "framesDropped": 10,
        "fps": 55.0,
        "bitrateKbps": 6000,
        "started": true
    }))
    .expect("deserialize legacy capture stats");

    assert_eq!(stats.frames_capture_dropped, 0);
    assert_eq!(stats.frames_pipeline_dropped, 0);
    assert_eq!(stats.frames_encoder_dropped, 0);
    assert_eq!(stats.frames_cpu_readback, 0);
    assert_eq!(stats.capture_fps, 0.0);
    assert_eq!(stats.publish_fps, 0.0);
    assert_eq!(stats.encoder_backend, None);
}
