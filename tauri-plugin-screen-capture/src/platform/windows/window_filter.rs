pub fn is_windows_system_ui(app_name: &str, title: &str) -> bool {
    let app = app_name.to_ascii_lowercase();
    let title = title.to_ascii_lowercase();
    matches!(
        app.as_str(),
        "shellexperiencehost" | "startmenuexperiencehost" | "explorer"
    ) || matches!(title.as_str(), "taskbar" | "start" | "shell_traywnd")
}
