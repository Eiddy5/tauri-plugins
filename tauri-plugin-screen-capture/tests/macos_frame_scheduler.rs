#![cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]

use std::time::Duration;

use tauri_plugin_screen_capture::platform::macos::media::scheduler::{
    SchedulerDecision, SchedulerPolicy,
};

#[test]
fn annotation_revision_wakes_a_static_scene_immediately() {
    let mut policy = SchedulerPolicy::new(60, 5, Duration::ZERO);
    policy.observe_source(1, Duration::ZERO);
    assert_eq!(
        policy.next(Duration::ZERO),
        SchedulerDecision::Compose {
            source_generation: 1
        }
    );
    policy.mark_composed(1, 0, Duration::ZERO);
    policy.observe_annotation_revision(2);
    assert_eq!(
        policy.next(Duration::from_millis(1)),
        SchedulerDecision::Compose {
            source_generation: 1
        }
    );
}

#[test]
fn missed_deadline_does_not_burst_catch_up_frames() {
    let mut policy = SchedulerPolicy::new(60, 5, Duration::ZERO);
    policy.mark_composed(1, 0, Duration::ZERO);
    let late = Duration::from_millis(200);
    policy.mark_composed(1, 0, late);
    assert!(policy.deadline() >= late + Duration::from_millis(16));
}

#[test]
fn latest_source_replaces_old_and_backpressure_is_explicit() {
    let mut policy = SchedulerPolicy::new(30, 5, Duration::ZERO);
    policy.observe_source(1, Duration::ZERO);
    policy.observe_source(2, Duration::from_millis(1));
    policy.set_surface_available(false);
    assert_eq!(
        policy.next(Duration::from_millis(1)),
        SchedulerDecision::DropBackpressure
    );
    policy.set_surface_available(true);
    assert_eq!(
        policy.next(Duration::from_millis(1)),
        SchedulerDecision::Compose {
            source_generation: 2
        }
    );
}
