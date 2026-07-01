const COMMANDS: &[&str] = &[
    "get_snapshot",
    "start_watching",
    "stop_watching",
    "get_config",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
