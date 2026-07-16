use tauri_plugin_screen_capture::{
    AnnotationColor, AnnotationDocument, AnnotationElement, AnnotationElementKind,
    AnnotationOptions, AnnotationPoint,
};

#[test]
fn annotation_document_uses_a_platform_neutral_wire_contract() {
    let document = AnnotationDocument {
        visible: true,
        elements: vec![AnnotationElement {
            id: "stroke-1".to_string(),
            kind: AnnotationElementKind::Pen,
            points: vec![
                AnnotationPoint { x: 0.25, y: 0.5 },
                AnnotationPoint { x: 0.75, y: 0.5 },
            ],
            color: AnnotationColor {
                red: 255,
                green: 32,
                blue: 16,
                alpha: 192,
            },
            width: 0.01,
        }],
    };

    let json = serde_json::to_value(document).expect("serialize annotation document");

    assert_eq!(json["visible"], true);
    assert_eq!(json["elements"][0]["kind"], "pen");
    assert_eq!(json["elements"][0]["points"][0]["x"], 0.25);
    assert_eq!(json["elements"][0]["color"]["alpha"], 192);
    assert_eq!(json["elements"][0]["width"], 0.01);
}

#[test]
fn annotation_options_default_to_disabled_for_existing_capture_callers() {
    let options: AnnotationOptions =
        serde_json::from_value(serde_json::json!({})).expect("deserialize annotation options");

    assert!(!options.enabled);
}

#[test]
fn annotation_document_supports_the_dingtalk_style_shape_baseline() {
    let kinds = [
        AnnotationElementKind::Pen,
        AnnotationElementKind::Line,
        AnnotationElementKind::Rectangle,
        AnnotationElementKind::Ellipse,
        AnnotationElementKind::Arrow,
    ];

    let json = serde_json::to_value(kinds).expect("serialize annotation kinds");
    assert_eq!(
        json,
        serde_json::json!(["pen", "line", "rectangle", "ellipse", "arrow"])
    );
}
