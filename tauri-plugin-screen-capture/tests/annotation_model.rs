use tauri_plugin_screen_capture::{
    annotation::{simplify_points, AnnotationSession, NormalizedPoint},
    AnnotationTool, CaptureErrorCode,
};

#[test]
fn completed_stroke_is_one_undoable_revision() {
    let session = AnnotationSession::default();
    session.set_interaction_enabled(true).unwrap();
    session
        .begin_stroke(NormalizedPoint::new(0.1, 0.2), 0.01)
        .unwrap();
    session
        .append_point(NormalizedPoint::new(0.4, 0.5))
        .unwrap();
    let state = session.end_stroke().unwrap();

    assert_eq!(state.operation_count, 1);
    assert!(state.can_undo);
    assert_eq!(state.revision, 2);
    assert_eq!(session.snapshot().operations().len(), 1);

    let undone = session.undo().unwrap();
    assert_eq!(undone.operation_count, 0);
    assert_eq!(undone.revision, 3);
}

#[test]
fn disabled_interaction_rejects_native_input_without_mutating_scene() {
    let session = AnnotationSession::default();
    let error = session
        .begin_stroke(NormalizedPoint::new(0.5, 0.5), 0.01)
        .unwrap_err();
    assert_eq!(
        error.payload().code,
        CaptureErrorCode::AnnotationUnavailable
    );
    assert_eq!(session.state().revision, 0);
}

#[test]
fn eraser_is_kept_as_an_ordered_operation() {
    let session = AnnotationSession::default();
    session.set_interaction_enabled(true).unwrap();
    session
        .set_tool(AnnotationTool::Eraser { width: 24.0 })
        .unwrap();
    session
        .begin_stroke(NormalizedPoint::new(0.2, 0.2), 0.04)
        .unwrap();
    session.end_stroke().unwrap();
    assert!(session.snapshot().operations()[0].is_eraser());
}

#[test]
fn invalid_tool_does_not_replace_the_previous_tool() {
    let session = AnnotationSession::default();
    let before = session.state();
    let error = session
        .set_tool(AnnotationTool::Pen {
            color: "red".into(),
            width: 0.0,
        })
        .unwrap_err();

    assert_eq!(
        error.payload().code,
        CaptureErrorCode::AnnotationInvalidOptions
    );
    assert_eq!(session.state(), before);
}

#[test]
fn simplification_keeps_endpoints_and_visible_corner() {
    let input = vec![
        NormalizedPoint::new(0.0, 0.0),
        NormalizedPoint::new(0.1, 0.001),
        NormalizedPoint::new(0.5, 0.5),
        NormalizedPoint::new(0.9, 0.501),
        NormalizedPoint::new(1.0, 1.0),
    ];
    let output = simplify_points(&input, 0.002);
    assert_eq!(output.first(), input.first());
    assert_eq!(output.last(), input.last());
    assert!(output.contains(&NormalizedPoint::new(0.5, 0.5)));
}

#[test]
fn near_duplicate_samples_are_dropped_before_commit() {
    let session = AnnotationSession::default();
    session.set_interaction_enabled(true).unwrap();
    session
        .begin_stroke(NormalizedPoint::new(0.5, 0.5), 0.08)
        .unwrap();
    session
        .append_point(NormalizedPoint::new(0.5001, 0.5001))
        .unwrap();
    session
        .append_point(NormalizedPoint::new(0.8, 0.8))
        .unwrap();
    session.end_stroke().unwrap();

    assert_eq!(
        session.snapshot().operations()[0].stroke().points().len(),
        2
    );
}
