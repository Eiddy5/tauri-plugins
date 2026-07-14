#![cfg(target_os = "macos")]

use tauri_plugin_screen_capture::overlay::macos::{
    decide_window_overlay, OverlayDecision, WindowSnapshot,
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
