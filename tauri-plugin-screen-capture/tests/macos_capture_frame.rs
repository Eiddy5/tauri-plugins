#![cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]

use tauri_plugin_screen_capture::platform::macos::media::{CaptureFrameGeometry, FrameRect};

#[test]
fn frame_geometry_uses_screen_capture_attachments() {
    let geometry = CaptureFrameGeometry::from_attachments(
        1920,
        1080,
        Some(FrameRect::new(240.0, 0.0, 1440.0, 1080.0)),
        Some(2.0),
        Some(1.0),
    )
    .unwrap();
    assert_eq!(geometry.output_content_rect().x, 240.0);
    assert_eq!(geometry.scale_factor(), 1.0);
    assert_eq!(geometry.content_scale(), 2.0);
}

#[test]
fn frame_geometry_rejects_invalid_attachment_values() {
    assert!(CaptureFrameGeometry::from_attachments(
        1920,
        1080,
        Some(FrameRect::new(0.0, 0.0, f64::NAN, 1080.0)),
        Some(1.0),
        Some(1.0),
    )
    .is_err());
}
