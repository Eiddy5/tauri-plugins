pub fn is_macos_system_ui(app_name: &str, title: &str) -> bool {
    let app = app_name.to_ascii_lowercase();
    let title = title.to_ascii_lowercase();
    matches!(app.as_str(), "dock" | "windowserver" | "systemuiserver")
        || matches!(title.as_str(), "dock" | "menu bar")
}
