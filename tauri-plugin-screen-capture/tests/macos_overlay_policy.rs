#![cfg(target_os = "macos")]

use tauri_plugin_screen_capture::overlay::macos::{
    decide_window_overlay, event_action, lightweight_order_span, needs_native_update,
    verify_lightweight_order, verify_overlay_panel_placement, visible_corner_layers, MacRect,
    OrderVerificationState, OrderedWindow, OverlayDecision, OverlayEvent, OverlayPanelLayout,
    RefreshAction, WindowFrameAction, WindowFrameTracker, WindowSnapshot,
    WINDOW_POSITION_POLL_INTERVAL,
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

fn rect(x: f64, y: f64, width: f64, height: f64) -> MacRect {
    MacRect {
        x,
        y,
        width,
        height,
    }
}

fn detailed_target(id: u32, layer: i32, owner_pid: u32, frame: MacRect) -> OrderedWindow {
    OrderedWindow::target_at_layer(id, layer)
        .with_owner_pid(owner_pid)
        .with_frame(frame)
}

fn detailed_panel(id: u32, layer: i32, owner_pid: u32, frame: MacRect) -> OrderedWindow {
    OrderedWindow::panel_at_layer(id, layer)
        .with_owner_pid(owner_pid)
        .with_frame(frame)
}

fn detailed_other(id: u32, layer: i32, owner_pid: u32, frame: MacRect) -> OrderedWindow {
    OrderedWindow::other_at_layer(id, layer)
        .with_owner_pid(owner_pid)
        .with_frame(frame)
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
fn single_overlay_panel_uses_target_frame_and_four_corner_layers() {
    let target = MacRect {
        x: 10.0,
        y: 20.0,
        width: 800.0,
        height: 600.0,
    };
    let OverlayPanelLayout {
        panel_frame,
        corner_frames,
    } = OverlayPanelLayout::new(target, 32.0);

    assert_eq!(panel_frame, target);
    assert_eq!(corner_frames.len(), 4);
    assert_eq!(
        corner_frames[0],
        MacRect {
            x: 10.0,
            y: 588.0,
            width: 32.0,
            height: 32.0,
        }
    );
    assert_eq!(
        corner_frames[3],
        MacRect {
            x: 778.0,
            y: 20.0,
            width: 32.0,
            height: 32.0,
        }
    );
}

#[test]
fn overlay_rejects_empty_or_non_finite_bounds() {
    assert!(!MacRect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 100.0,
    }
    .is_valid());
    assert!(!MacRect {
        x: f64::NAN,
        y: 0.0,
        width: 100.0,
        height: 100.0,
    }
    .is_valid());
    assert!(MacRect {
        x: -1920.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    }
    .is_valid());
}

#[test]
fn verification_accepts_single_panel_immediately_above_target() {
    let target = rect(0.0, 0.0, 100.0, 100.0);
    let layout = OverlayPanelLayout::new(target, 32.0);
    let windows = vec![
        OrderedWindow::other(90),
        detailed_panel(11, 0, 9000, target),
        detailed_target(42, 0, 9, target),
        OrderedWindow::other(7),
    ];

    assert!(verify_overlay_panel_placement(
        &windows, 42, 11, &layout, [true; 4],
    ));
}

#[test]
fn verification_rejects_panel_promoted_above_a_foreign_cover_window() {
    let target = rect(0.0, 0.0, 100.0, 100.0);
    let layout = OverlayPanelLayout::new(target, 32.0);
    let windows = vec![
        detailed_panel(11, 0, 9000, target),
        detailed_other(90, 0, 99, target),
        detailed_target(42, 0, 9, target),
    ];

    assert!(!verify_overlay_panel_placement(
        &windows, 42, 11, &layout, [true; 4],
    ));
}

#[test]
fn verification_rejects_panel_on_a_different_window_server_layer() {
    let target = rect(0.0, 0.0, 100.0, 100.0);
    let layout = OverlayPanelLayout::new(target, 32.0);
    let windows = vec![
        detailed_panel(11, 1, 9000, target),
        detailed_target(42, 0, 9, target),
    ];

    assert!(!verify_overlay_panel_placement(
        &windows, 42, 11, &layout, [true; 4],
    ));
}

#[test]
fn lightweight_order_accepts_single_panel_immediately_above_target() {
    assert!(verify_lightweight_order(&[90, 11, 42, 7], 42, &[11, 42]));
}

#[test]
fn lightweight_order_rejects_target_reordered_above_panels() {
    assert!(!verify_lightweight_order(&[90, 42, 11, 7], 42, &[11, 42]));
}

#[test]
fn lightweight_order_accepts_a_cached_safe_sibling_between_panel_and_target() {
    let windows = vec![
        OrderedWindow::panel(11),
        OrderedWindow::other(77).with_owner_pid(9),
        OrderedWindow::target(42).with_owner_pid(9),
    ];
    let span = lightweight_order_span(&windows, 42, 11).unwrap();

    assert_eq!(span, vec![11, 77, 42]);
    assert!(verify_lightweight_order(&[90, 11, 77, 42, 7], 42, &span));
}

#[test]
fn lightweight_order_rejects_an_unknown_intervening_window_or_missing_panel() {
    assert!(!verify_lightweight_order(
        &[90, 11, 77, 42, 7],
        42,
        &[11, 42]
    ));
    let windows = vec![
        OrderedWindow::other(77).with_owner_pid(9),
        OrderedWindow::target(42).with_owner_pid(9),
    ];
    assert!(lightweight_order_span(&windows, 42, 11).is_none());
}

#[test]
fn non_overlapping_same_owner_siblings_do_not_invalidate_the_panel() {
    let target = rect(0.0, 0.0, 2560.0, 1440.0);
    let layout = OverlayPanelLayout::new(target, 32.0);
    let windows = vec![
        detailed_panel(11, 0, 9000, target),
        detailed_other(16818, 0, 36957, rect(48.0, 2.0, 2464.0, 1386.0)),
        detailed_other(18571, 0, 36957, rect(49.0, 458.0, 11.0, 64.0)),
        detailed_target(16303, 0, 36957, target),
    ];

    let visibility = visible_corner_layers(&windows, 16303, &layout.corner_frames);
    assert_eq!(visibility, [true; 4]);
    assert!(verify_overlay_panel_placement(
        &windows, 16303, 11, &layout, visibility,
    ));
    let span = lightweight_order_span(&windows, 16303, 11).unwrap();
    let window_ids = windows.iter().map(|window| window.id).collect::<Vec<_>>();
    assert!(verify_lightweight_order(&window_ids, 16303, &span));
}

#[test]
fn same_owner_sibling_hides_only_the_overlapped_corner() {
    let layout = OverlayPanelLayout::new(rect(0.0, 0.0, 100.0, 100.0), 32.0);
    let windows = vec![
        detailed_other(7, 0, 42, rect(0.0, 80.0, 10.0, 10.0)),
        detailed_target(8, 0, 42, rect(0.0, 0.0, 100.0, 100.0)),
    ];

    assert_eq!(
        visible_corner_layers(&windows, 8, &layout.corner_frames),
        [false, true, true, true]
    );
}

#[test]
fn single_panel_keeps_safe_sibling_below_it_and_hides_only_the_covered_corner_layer() {
    let target = rect(0.0, 0.0, 100.0, 100.0);
    let layout = OverlayPanelLayout::new(target, 32.0);
    let windows = vec![
        detailed_panel(11, 0, 9000, layout.panel_frame),
        detailed_other(7, 0, 42, rect(0.0, 80.0, 10.0, 10.0)),
        detailed_target(8, 0, 42, target),
    ];
    let visibility = visible_corner_layers(&windows, 8, &layout.corner_frames);

    assert_eq!(visibility, [false, true, true, true]);
    assert!(verify_overlay_panel_placement(
        &windows, 8, 11, &layout, visibility,
    ));
}

#[test]
fn same_owner_surface_inside_the_transparent_corner_area_keeps_the_corner_visible() {
    let target = rect(0.0, 0.0, 100.0, 100.0);
    let layout = OverlayPanelLayout::new(target, 32.0);
    let transparent_center = rect(10.0, 78.0, 10.0, 10.0);
    let windows = vec![
        detailed_panel(11, 0, 9000, target),
        detailed_other(7, 0, 42, transparent_center),
        detailed_target(8, 0, 42, target),
    ];

    let visibility = visible_corner_layers(&windows, 8, &layout.corner_frames);
    assert_eq!(visibility, [true; 4]);
    assert!(verify_overlay_panel_placement(
        &windows, 8, 11, &layout, visibility,
    ));
}

#[test]
fn different_owner_window_does_not_change_sibling_visibility() {
    let layout = OverlayPanelLayout::new(rect(0.0, 0.0, 100.0, 100.0), 32.0);
    let windows = vec![
        detailed_other(7, 0, 99, rect(0.0, 80.0, 10.0, 10.0)),
        detailed_target(8, 0, 42, rect(0.0, 0.0, 100.0, 100.0)),
    ];

    assert_eq!(
        visible_corner_layers(&windows, 8, &layout.corner_frames),
        [true; 4]
    );
}

#[test]
fn touching_sibling_bounds_do_not_occlude_a_corner() {
    let layout = OverlayPanelLayout::new(rect(0.0, 0.0, 100.0, 100.0), 32.0);
    let windows = vec![
        detailed_other(7, 0, 42, rect(32.0, 80.0, 10.0, 10.0)),
        detailed_target(8, 0, 42, rect(0.0, 0.0, 100.0, 100.0)),
    ];

    assert_eq!(
        visible_corner_layers(&windows, 8, &layout.corner_frames),
        [true; 4]
    );
}

#[test]
fn first_window_frame_observation_keeps_the_visible_overlay() {
    let mut tracker = WindowFrameTracker::default();
    assert_eq!(
        tracker.observe(MacRect {
            x: 10.0,
            y: 20.0,
            width: 800.0,
            height: 600.0,
        }),
        WindowFrameAction::Keep
    );
}

#[test]
fn stable_frame_escalates_to_refresh_when_lightweight_order_is_invalid() {
    assert_eq!(
        WindowFrameAction::Keep.refresh_if_order_invalid(false),
        WindowFrameAction::Refresh
    );
    assert_eq!(
        WindowFrameAction::Keep.refresh_if_order_invalid(true),
        WindowFrameAction::Keep
    );
    assert_eq!(
        WindowFrameAction::Hide.refresh_if_order_invalid(false),
        WindowFrameAction::Hide
    );
}

#[test]
fn changed_window_frame_hides_the_overlay_until_it_stabilizes() {
    let initial = MacRect {
        x: 10.0,
        y: 20.0,
        width: 800.0,
        height: 600.0,
    };
    let moved = MacRect {
        x: 30.0,
        y: 40.0,
        width: 900.0,
        height: 700.0,
    };
    let mut tracker = WindowFrameTracker::default();

    assert!(!tracker.is_moving());
    assert_eq!(tracker.observe(initial), WindowFrameAction::Keep);
    assert_eq!(tracker.observe(moved), WindowFrameAction::Hide);
    assert!(tracker.is_moving());
    assert_eq!(tracker.observe(moved), WindowFrameAction::Refresh);
    assert!(!tracker.is_moving());
    assert_eq!(tracker.observe(moved), WindowFrameAction::Keep);
}

#[test]
fn every_new_frame_keeps_the_overlay_hidden_during_continuous_motion() {
    let mut tracker = WindowFrameTracker::default();
    let frame = |x| MacRect {
        x,
        y: 20.0,
        width: 800.0,
        height: 600.0,
    };

    assert_eq!(tracker.observe(frame(10.0)), WindowFrameAction::Keep);
    assert_eq!(tracker.observe(frame(20.0)), WindowFrameAction::Hide);
    assert_eq!(tracker.observe(frame(30.0)), WindowFrameAction::Hide);
    assert_eq!(tracker.observe(frame(40.0)), WindowFrameAction::Hide);
    assert_eq!(tracker.observe(frame(40.0)), WindowFrameAction::Refresh);
}

#[test]
fn explicit_refresh_synchronizes_the_tracker_without_a_second_hide() {
    let frame = |x| MacRect {
        x,
        y: 20.0,
        width: 800.0,
        height: 600.0,
    };
    let mut tracker = WindowFrameTracker::default();

    assert_eq!(tracker.observe(frame(10.0)), WindowFrameAction::Keep);
    assert_eq!(tracker.observe(frame(20.0)), WindowFrameAction::Hide);

    tracker.synchronize(frame(20.0));

    assert_eq!(tracker.observe(frame(20.0)), WindowFrameAction::Keep);
}

#[test]
fn stable_frame_refreshes_when_the_overlay_was_hidden_for_minimize() {
    let frame = MacRect {
        x: 10.0,
        y: 20.0,
        width: 800.0,
        height: 600.0,
    };
    let mut tracker = WindowFrameTracker::default();

    tracker.synchronize(frame);

    assert_eq!(
        tracker.observe_for_visibility(frame, false),
        WindowFrameAction::Refresh
    );
}

#[test]
fn workspace_and_display_changes_trigger_refresh() {
    for event in [
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
fn application_activation_waits_for_window_server_ordering() {
    assert_eq!(
        event_action(OverlayEvent::ApplicationActivated),
        RefreshAction::RefreshAfterWindowOrdering
    );
}

#[test]
fn window_position_timer_is_handled_by_target_frame_tracking() {
    assert_eq!(
        WINDOW_POSITION_POLL_INTERVAL,
        std::time::Duration::from_millis(100)
    );
    assert_eq!(
        event_action(OverlayEvent::WindowPositionTimer),
        RefreshAction::TrackWindowFrame
    );
}

#[test]
fn correction_timer_skips_unchanged_native_updates() {
    assert!(!needs_native_update(true, false, true));
    assert!(needs_native_update(false, false, true));
    assert!(needs_native_update(true, true, true));
    assert!(needs_native_update(true, false, false));
}

#[test]
fn order_verification_only_accepts_latest_generation() {
    let mut state = OrderVerificationState::default();

    let stale_generation = state.request();
    assert_eq!(stale_generation, 1);
    assert_eq!(state.pending_generation(), Some(1));
    let current_generation = state.request();
    assert_eq!(current_generation, 2);
    assert_eq!(state.pending_generation(), Some(2));
    assert!(!state.take_if_current(stale_generation));
    assert_eq!(state.pending_generation(), Some(2));
    assert!(state.take_if_current(current_generation));
    assert_eq!(state.pending_generation(), None);
}

#[test]
fn hidden_overlay_cancels_pending_order_verification() {
    let mut state = OrderVerificationState::default();
    let canceled_generation = state.request();

    state.cancel();

    assert!(!state.take_if_current(canceled_generation));
    assert_eq!(state.pending_generation(), None);
}

#[test]
fn stale_timer_cannot_consume_verification_requested_after_cancel() {
    let mut state = OrderVerificationState::default();
    let stale_generation = state.request();
    state.cancel();
    let current_generation = state.request();

    assert!(!state.take_if_current(stale_generation));
    assert_eq!(state.pending_generation(), Some(current_generation));
    assert!(state.take_if_current(current_generation));
}
