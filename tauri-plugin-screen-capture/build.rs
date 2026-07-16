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
    "set_annotation_document",
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
