const COMMANDS: &[&str] = &[
    "runtime_connect",
    "runtime_ready",
    "runtime_invoke",
    "runtime_cancel",
    "runtime_resolve",
    "runtime_disconnect",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
