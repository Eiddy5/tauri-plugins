use tauri_plugin_screen_capture::overlay::{
    corner_segments, OverlayRect, OverlaySegment, OverlayStyle,
};

#[cfg(windows)]
use tauri_plugin_screen_capture::overlay::windows_target_handle_from_source_id;

#[test]
fn corner_segments_frame_the_target_rect_with_eight_segments() {
    let style = OverlayStyle {
        color: 0x22C55E,
        thickness: 4,
        corner_length: 32,
    };

    let segments = corner_segments(
        OverlayRect {
            left: 100,
            top: 200,
            right: 500,
            bottom: 600,
        },
        style,
    );

    assert_eq!(
        segments,
        vec![
            OverlaySegment::new(100, 200, 32, 4),
            OverlaySegment::new(100, 200, 4, 32),
            OverlaySegment::new(468, 200, 32, 4),
            OverlaySegment::new(496, 200, 4, 32),
            OverlaySegment::new(100, 596, 32, 4),
            OverlaySegment::new(100, 568, 4, 32),
            OverlaySegment::new(468, 596, 32, 4),
            OverlaySegment::new(496, 568, 4, 32),
        ]
    );
}

#[test]
fn corner_segments_clamp_length_for_small_targets() {
    let segments = corner_segments(
        OverlayRect {
            left: 10,
            top: 20,
            right: 50,
            bottom: 52,
        },
        OverlayStyle {
            color: 0x22C55E,
            thickness: 4,
            corner_length: 32,
        },
    );

    assert_eq!(
        segments,
        vec![
            OverlaySegment::new(10, 20, 20, 4),
            OverlaySegment::new(10, 20, 4, 16),
            OverlaySegment::new(30, 20, 20, 4),
            OverlaySegment::new(46, 20, 4, 16),
            OverlaySegment::new(10, 48, 20, 4),
            OverlaySegment::new(10, 36, 4, 16),
            OverlaySegment::new(30, 48, 20, 4),
            OverlaySegment::new(46, 36, 4, 16),
        ]
    );
}

#[test]
fn default_style_matches_the_share_border_spec() {
    assert_eq!(
        OverlayStyle::default(),
        OverlayStyle {
            color: 0x22C55E,
            thickness: 4,
            corner_length: 32,
        }
    );
}

#[test]
fn corner_segments_keep_positive_dimensions_for_invalid_public_inputs() {
    let segments = corner_segments(
        OverlayRect {
            left: 50,
            top: 60,
            right: 10,
            bottom: 20,
        },
        OverlayStyle {
            color: 0x22C55E,
            thickness: 0,
            corner_length: -8,
        },
    );

    assert_eq!(segments.len(), 8);
    assert!(segments
        .iter()
        .all(|segment| segment.width >= 1 && segment.height >= 1));
}

#[cfg(windows)]
#[test]
fn windows_target_handle_from_source_id_parses_only_window_hex_ids() {
    assert_eq!(
        windows_target_handle_from_source_id("window:2a"),
        Some(0x2a)
    );
    assert_eq!(windows_target_handle_from_source_id("display:1"), None);
}
