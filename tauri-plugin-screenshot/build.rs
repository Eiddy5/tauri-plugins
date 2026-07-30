const COMMANDS: &[&str] = &["capture_area", "take_capture"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
