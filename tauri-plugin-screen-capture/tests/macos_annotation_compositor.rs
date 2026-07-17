#![cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]

use tauri_plugin_screen_capture::annotation::{
    AnnotationOperation, AnnotationRgba, NormalizedPoint, Size, Stroke,
};
use tauri_plugin_screen_capture::platform::macos::media::tessellator::document_operations;
use tauri_plugin_screen_capture::platform::macos::media::tessellator::tessellate;
use tauri_plugin_screen_capture::platform::macos::media::{
    CaptureFrameGeometry, MacCaptureFrame, MacMetalCompositor,
};
use tauri_plugin_screen_capture::{
    AnnotationColor, AnnotationDocument, AnnotationElement, AnnotationElementKind, AnnotationPoint,
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
fn windows_annotation_document_tools_map_to_metal_strokes() {
    let element = |kind, points| AnnotationElement {
        id: format!("{kind:?}"),
        kind,
        points,
        color: AnnotationColor {
            red: 255,
            green: 0,
            blue: 0,
            alpha: 255,
        },
        width: 0.006,
    };
    let endpoints = || {
        vec![
            AnnotationPoint { x: 0.1, y: 0.2 },
            AnnotationPoint { x: 0.8, y: 0.7 },
        ]
    };
    let document = AnnotationDocument {
        visible: true,
        elements: vec![
            element(AnnotationElementKind::Pen, endpoints()),
            element(AnnotationElementKind::Line, endpoints()),
            element(AnnotationElementKind::Rectangle, endpoints()),
            element(AnnotationElementKind::Ellipse, endpoints()),
            element(AnnotationElementKind::Arrow, endpoints()),
        ],
    };

    let operations = document_operations(&document, Size::new(1920.0, 1080.0)).unwrap();
    assert_eq!(operations.len(), 7, "arrow expands to three strokes");
    assert_eq!(operations[2].stroke().points().len(), 5);
    assert_eq!(operations[3].stroke().points().len(), 65);
    assert!(operations
        .iter()
        .flat_map(|operation| operation.stroke().points())
        .all(|point| point.x.is_finite() && point.y.is_finite()));
}

#[test]
#[ignore = "requires a logged-in macOS Metal session"]
fn compositor_produces_an_iosurface_backed_output() {
    let io_surface = aligned_bgra_surface(64, 64);
    let pixel_buffer = apple_cf::cv::CVPixelBuffer::create_with_io_surface(&io_surface)
        .expect("source pixel buffer");
    {
        let mut pixels = pixel_buffer.lock_read_write().expect("lock source pixels");
        for pixel in pixels
            .as_slice_mut()
            .expect("source pixels")
            .chunks_exact_mut(4)
        {
            pixel.copy_from_slice(&[0, 0, 255, 255]);
        }
    }
    let geometry = CaptureFrameGeometry::from_attachments(64, 64, None, None, None).unwrap();
    let source = MacCaptureFrame::new(pixel_buffer, 1, geometry, 1);
    let annotations = tauri_plugin_screen_capture::annotation::AnnotationSession::default();
    let compositor = MacMetalCompositor::new(64, 64).expect("Metal compositor");
    let output = compositor
        .compose(&source, &annotations.snapshot())
        .expect("compose frame");
    assert_eq!(output.size(), (64, 64));
    assert!(output.pixel_buffer().io_surface().is_some());
    let pixels = output
        .pixel_buffer()
        .lock_read_only()
        .expect("lock output pixels");
    assert_eq!(&pixels.as_slice()[..4], &[0, 0, 255, 255]);
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
