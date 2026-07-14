#![cfg(target_os = "macos")]

use tauri_plugin_screen_capture::overlay::macos::{
    corner_panel_frames, decide_window_overlay, event_action, needs_native_update,
    verify_relative_order, MacRect, OrderedWindow, OverlayDecision, OverlayEvent, RefreshAction,
    WindowSnapshot,
};

fn window(id: u32, layer: i32, order: usize, on_screen: bool) -> WindowSnapshot {
    WindowSnapshot {
        id,
        layer,
        order,
        on_screen,
        minimized: false,
    }
}

#[test]
fn overlay_uses_target_layer_and_immediately_above_order() {
    let decision = decide_window_overlay(&window(42, 0, 7, true), true);

    assert_eq!(
        decision,
        OverlayDecision::Show {
            target_id: 42,
            level: 0,
            target_order: 7,
        }
    );
}

#[test]
fn overlay_hides_when_target_cannot_be_safely_ordered() {
    assert_eq!(
        decide_window_overlay(&window(42, 0, 7, false), true),
        OverlayDecision::Hide
    );
    assert_eq!(
        decide_window_overlay(&window(42, 0, 7, true), false),
        OverlayDecision::Hide
    );
}

#[test]
fn overlay_hides_for_minimized_target() {
    let mut target = window(42, 0, 7, true);
    target.minimized = true;

    assert_eq!(decide_window_overlay(&target, true), OverlayDecision::Hide);
}

#[test]
fn four_corner_panels_only_allocate_corner_squares() {
    let frames = corner_panel_frames(
        MacRect {
            x: 10.0,
            y: 20.0,
            width: 800.0,
            height: 600.0,
        },
        32.0,
    );

    assert_eq!(frames.len(), 4);
    assert_eq!(
        frames[0],
        MacRect {
            x: 10.0,
            y: 588.0,
            width: 32.0,
            height: 32.0,
        }
    );
    assert_eq!(
        frames[3],
        MacRect {
            x: 778.0,
            y: 20.0,
            width: 32.0,
            height: 32.0,
        }
    );
}

#[test]
fn verification_accepts_four_panels_immediately_above_target() {
    let windows = vec![
        OrderedWindow::other(90),
        OrderedWindow::panel(14),
        OrderedWindow::panel(13),
        OrderedWindow::panel(12),
        OrderedWindow::panel(11),
        OrderedWindow::target(42),
        OrderedWindow::other(7),
    ];

    assert!(verify_relative_order(&windows, 42, &[11, 12, 13, 14]));
}

#[test]
fn verification_rejects_a_panel_promoted_above_an_existing_cover_window() {
    let windows = vec![
        OrderedWindow::panel(11),
        OrderedWindow::other(90),
        OrderedWindow::panel(14),
        OrderedWindow::panel(13),
        OrderedWindow::panel(12),
        OrderedWindow::target(42),
    ];

    assert!(!verify_relative_order(&windows, 42, &[11, 12, 13, 14]));
}

#[test]
fn drag_hides_immediately_and_mouse_up_refreshes_once() {
    assert_eq!(
        event_action(OverlayEvent::LeftMouseDragged),
        RefreshAction::Hide
    );
    assert_eq!(
        event_action(OverlayEvent::LeftMouseUp),
        RefreshAction::Refresh
    );
}

#[test]
fn workspace_and_display_changes_trigger_refresh() {
    for event in [
        OverlayEvent::ApplicationActivated,
        OverlayEvent::ApplicationHidden,
        OverlayEvent::ApplicationTerminated,
        OverlayEvent::ActiveSpaceChanged,
        OverlayEvent::ScreenParametersChanged,
        OverlayEvent::SystemWoke,
    ] {
        assert_eq!(event_action(event), RefreshAction::Refresh);
    }
}

#[test]
fn correction_timer_skips_unchanged_native_updates() {
    assert!(!needs_native_update(true, false, true));
    assert!(needs_native_update(false, false, true));
    assert!(needs_native_update(true, true, true));
    assert!(needs_native_update(true, false, false));
}
