use tauri_plugin_screen_capture::annotation::{
    CaptureGeometry, NormalizedPoint, Point, Rect, Size,
};

#[test]
fn normalized_points_map_into_letterboxed_content_rect() {
    let geometry =
        CaptureGeometry::aspect_fit(Size::new(1600.0, 1200.0), Size::new(1920.0, 1080.0));
    assert_eq!(
        geometry.output_content_rect(),
        Rect::new(240.0, 0.0, 1440.0, 1080.0)
    );
    assert_eq!(
        geometry.to_output(NormalizedPoint::new(0.0, 0.0)),
        Point::new(240.0, 0.0)
    );
    assert_eq!(
        geometry.to_output(NormalizedPoint::new(1.0, 1.0)),
        Point::new(1680.0, 1080.0)
    );
}

#[test]
fn appkit_overlay_point_maps_to_top_left_normalized_content() {
    let geometry = CaptureGeometry::from_overlay(Rect::new(-1440.0, 0.0, 1440.0, 900.0));
    assert_eq!(
        geometry.overlay_to_normalized(Point::new(-1440.0, 900.0)),
        NormalizedPoint::new(0.0, 0.0)
    );
    assert_eq!(
        geometry.overlay_to_normalized(Point::new(0.0, 0.0)),
        NormalizedPoint::new(1.0, 1.0)
    );
}

#[test]
fn invalid_geometry_is_rejected_without_native_types() {
    let error = CaptureGeometry::try_from_overlay(Rect::new(0.0, 0.0, 0.0, 900.0))
        .expect_err("zero-width overlay must fail");
    assert_eq!(
        error.payload().code,
        tauri_plugin_screen_capture::CaptureErrorCode::AnnotationInvalidOptions
    );
}
