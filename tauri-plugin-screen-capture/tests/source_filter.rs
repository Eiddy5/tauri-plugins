use tauri_plugin_screen_capture::{
    sources::{filter_sources, SourceFilterOptions},
    CaptureSource, CaptureSourceKind,
};

fn window(id: &str, title: &str, app: &str, width: u32, height: u32, pid: u32) -> CaptureSource {
    CaptureSource {
        id: id.to_string(),
        kind: CaptureSourceKind::Window,
        name: title.to_string(),
        title: Some(title.to_string()),
        app_name: Some(app.to_string()),
        pid: Some(pid),
        width,
        height,
        scale_factor: 1.0,
        is_primary: false,
        thumbnail_base64: None,
        filtered_reason: None,
    }
}

#[test]
fn excludes_current_app_by_default() {
    let sources = vec![
        window("window:app", "Capture Preview", "Example", 1200, 800, 42),
        window("window:editor", "main.rs", "Code", 1200, 800, 84),
    ];

    let filtered = filter_sources(
        sources,
        SourceFilterOptions {
            current_pid: Some(42),
            include_current_app: false,
            include_system_ui: false,
            debug_raw_sources: false,
        },
    );

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "window:editor");
}

#[test]
fn excludes_known_system_ui_by_default() {
    let sources = vec![
        window("window:dock", "Dock", "Dock", 1728, 80, 10),
        window(
            "window:taskbar",
            "Taskbar",
            "ShellExperienceHost",
            1920,
            48,
            11,
        ),
        window("window:browser", "Planning", "Browser", 1280, 900, 12),
    ];

    let filtered = filter_sources(
        sources,
        SourceFilterOptions {
            current_pid: None,
            include_current_app: false,
            include_system_ui: false,
            debug_raw_sources: false,
        },
    );

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "window:browser");
}

#[test]
fn debug_mode_keeps_filtered_sources_with_reasons() {
    let sources = vec![window("window:dock", "Dock", "Dock", 1728, 80, 10)];

    let filtered = filter_sources(
        sources,
        SourceFilterOptions {
            current_pid: None,
            include_current_app: false,
            include_system_ui: false,
            debug_raw_sources: true,
        },
    );

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].filtered_reason.as_deref(), Some("systemUi"));
}

#[test]
fn include_system_ui_keeps_known_system_ui_sources() {
    let sources = vec![window("window:dock", "Dock", "Dock", 1728, 80, 10)];

    let filtered = filter_sources(
        sources,
        SourceFilterOptions {
            current_pid: None,
            include_current_app: false,
            include_system_ui: true,
            debug_raw_sources: false,
        },
    );

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "window:dock");
    assert_eq!(filtered[0].filtered_reason, None);
}

#[test]
fn keeps_normal_file_explorer_windows() {
    let sources = vec![window(
        "window:explorer-docs",
        "Documents",
        "Explorer",
        1280,
        900,
        20,
    )];

    let filtered = filter_sources(
        sources,
        SourceFilterOptions {
            current_pid: None,
            include_current_app: false,
            include_system_ui: false,
            debug_raw_sources: false,
        },
    );

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "window:explorer-docs");
}

#[test]
fn system_ui_reason_takes_precedence_over_size_filter() {
    let sources = vec![window(
        "window:taskbar",
        "Taskbar",
        "Explorer",
        1920,
        48,
        21,
    )];

    let debug_filtered = filter_sources(
        sources.clone(),
        SourceFilterOptions {
            current_pid: None,
            include_current_app: false,
            include_system_ui: false,
            debug_raw_sources: true,
        },
    );

    assert_eq!(
        debug_filtered[0].filtered_reason.as_deref(),
        Some("systemUi")
    );

    let included = filter_sources(
        sources,
        SourceFilterOptions {
            current_pid: None,
            include_current_app: false,
            include_system_ui: true,
            debug_raw_sources: false,
        },
    );

    assert_eq!(included.len(), 1);
    assert_eq!(included[0].id, "window:taskbar");
}
