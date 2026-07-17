#![cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]

use tauri_plugin_screen_capture::annotation::{
    AnnotationOperation, AnnotationRgba, NormalizedPoint, Size, Stroke,
};
use tauri_plugin_screen_capture::platform::macos::media::tessellator::tessellate;
use tauri_plugin_screen_capture::platform::macos::media::{
    CaptureFrameGeometry, MacCaptureFrame, MacMetalCompositor,
};

#[test]
fn horizontal_stroke_mesh_covers_the_requested_width() {
    let stroke = Stroke::new(
        vec![
            NormalizedPoint::new(0.1, 0.5),
            NormalizedPoint::new(0.9, 0.5),
        ],
        0.1,
        Some(AnnotationRgba {
            red: 255,
            green: 0,
            blue: 0,
            alpha: 255,
        }),
    )
    .unwrap();
    let mesh = tessellate(&stroke, Size::new(100.0, 100.0), false);
    let bounds = mesh.bounds();
    assert!((bounds.min_y - 45.0).abs() < 0.001);
    assert!((bounds.max_y - 55.0).abs() < 0.001);
    assert!(mesh
        .vertices()
        .iter()
        .all(|vertex| vertex.position.iter().all(|value| value.is_finite())));
}

#[test]
fn eraser_mesh_is_tagged_and_finite() {
    let stroke = Stroke::new(
        vec![
            NormalizedPoint::new(0.25, 0.25),
            NormalizedPoint::new(0.75, 0.75),
        ],
        0.05,
        None,
    )
    .unwrap();
    let operation = AnnotationOperation::Eraser(stroke);
    let mesh =
        tauri_plugin_screen_capture::platform::macos::media::tessellator::tessellate_operation(
            &operation,
            Size::new(320.0, 180.0),
        );
    assert!(mesh.is_eraser());
    assert!(mesh
        .vertices()
        .iter()
        .all(|vertex| vertex.position.iter().all(|value| value.is_finite())));
}

#[test]
#[ignore = "requires a logged-in macOS Metal session"]
fn compositor_produces_an_iosurface_backed_output() {
    let io_surface = aligned_bgra_surface(64, 64);
    let pixel_buffer = apple_cf::cv::CVPixelBuffer::create_with_io_surface(&io_surface)
        .expect("source pixel buffer");
    let geometry = CaptureFrameGeometry::from_attachments(64, 64, None, None, None).unwrap();
    let source = MacCaptureFrame::new(pixel_buffer, 1, geometry, 1);
    let annotations = tauri_plugin_screen_capture::annotation::AnnotationSession::default();
    let compositor = MacMetalCompositor::new(64, 64).expect("Metal compositor");
    let output = compositor
        .compose(&source, &annotations.snapshot())
        .expect("compose frame");
    assert_eq!(output.size(), (64, 64));
    assert!(output.pixel_buffer().io_surface().is_some());
}

#[test]
#[ignore = "requires a logged-in macOS Metal session"]
fn compositor_accepts_a_width_with_an_unaligned_tight_stride() {
    let width = 1026;
    let height = 64;
    let io_surface = aligned_bgra_surface(width, height);
    let pixel_buffer = apple_cf::cv::CVPixelBuffer::create_with_io_surface(&io_surface)
        .expect("source pixel buffer");
    let geometry = CaptureFrameGeometry::from_attachments(width, height, None, None, None).unwrap();
    let source = MacCaptureFrame::new(pixel_buffer, 1, geometry, 1);
    let annotations = tauri_plugin_screen_capture::annotation::AnnotationSession::default();
    let compositor = MacMetalCompositor::new(width, height).expect("Metal compositor");
    let output = compositor
        .compose(&source, &annotations.snapshot())
        .expect("compose odd-width frame");
    assert_eq!(output.size(), (width, height));
    assert_eq!(output.pixel_buffer().bytes_per_row() % 16, 0);
}

fn aligned_bgra_surface(width: u32, height: u32) -> apple_cf::iosurface::IOSurface {
    let bytes_per_row = ((width as usize * 4) + 15) & !15;
    apple_cf::iosurface::IOSurface::create_with_properties(
        width as usize,
        height as usize,
        u32::from_be_bytes(*b"BGRA"),
        4,
        bytes_per_row,
        bytes_per_row * height as usize,
        None,
    )
    .expect("source IOSurface")
}
