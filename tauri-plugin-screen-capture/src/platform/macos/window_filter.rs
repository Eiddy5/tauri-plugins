pub fn is_macos_system_ui(app_name: &str, title: &str) -> bool {
    let app = app_name.to_ascii_lowercase();
    let title = title.to_ascii_lowercase();
    matches!(app.as_str(), "dock" | "windowserver" | "systemuiserver")
        || matches!(title.as_str(), "dock" | "menu bar")
}

pub fn macos_window_filtered_reason(
    is_on_screen: bool,
    has_owning_application: bool,
    has_title: bool,
) -> Option<&'static str> {
    if !is_on_screen {
        Some("offScreen")
    } else if !has_owning_application {
        Some("missingOwner")
    } else if !has_title {
        Some("missingTitle")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_non_user_macos_windows() {
        assert_eq!(
            macos_window_filtered_reason(true, false, true),
            Some("missingOwner")
        );
        assert_eq!(
            macos_window_filtered_reason(false, true, true),
            Some("offScreen")
        );
        assert_eq!(
            macos_window_filtered_reason(true, true, false),
            Some("missingTitle")
        );
        assert_eq!(macos_window_filtered_reason(true, true, true), None);
    }
}
