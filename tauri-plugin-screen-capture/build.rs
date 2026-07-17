const COMMANDS: &[&str] = &[
    "get_capabilities",
    "check_permission",
    "request_permission",
    "list_sources",
    "start_capture",
    "pause_capture",
    "resume_capture",
    "stop_capture",
    "get_capture_session",
    "get_capture_stats",
    "get_annotation_document",
    "get_annotation_input_target",
    "set_annotation_document",
    "set_annotation_interaction",
    "set_annotation_tool",
    "undo_annotation",
    "clear_annotations",
    "get_annotation_state",
    "create_webrtc_offer",
    "accept_webrtc_answer",
    "add_webrtc_ice_candidate",
];

fn main() {
    add_swift_runtime_rpath();
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}

fn add_swift_runtime_rpath() {
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("macos") {
        return;
    }

    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
