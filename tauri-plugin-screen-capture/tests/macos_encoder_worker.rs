#![cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]

use std::time::Duration;

use tauri_plugin_screen_capture::webrtc::{
    h264_encoder_macos::{EncoderControlState, OrderedSampleHandoff},
    track::EncodedVideoSample,
};

fn sample(timestamp_ns: u64) -> EncodedVideoSample {
    EncodedVideoSample {
        data: vec![0, 0, 0, 1, 0x65],
        duration: Duration::from_millis(16),
        timestamp_ns,
    }
}

#[test]
fn encoded_handoff_preserves_submission_order() {
    let handoff = OrderedSampleHandoff::new(1);
    handoff.push(sample(1)).unwrap();
    assert_eq!(handoff.pop().unwrap().timestamp_ns, 1);
    handoff.push(sample(2)).unwrap();
    assert_eq!(handoff.pop().unwrap().timestamp_ns, 2);
}

#[test]
fn keyframe_request_is_consumed_by_exactly_one_submission() {
    let state = EncoderControlState::default();
    state.request_keyframe();
    assert!(state.take_force_keyframe());
    assert!(!state.take_force_keyframe());
}
