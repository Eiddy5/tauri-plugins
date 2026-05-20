use crate::models::{CaptureSource, CaptureSourceKind};

#[derive(Debug, Clone, Copy)]
pub struct SourceFilterOptions {
    pub current_pid: Option<u32>,
    pub include_current_app: bool,
    pub include_system_ui: bool,
    pub debug_raw_sources: bool,
}

pub fn filter_sources(
    sources: Vec<CaptureSource>,
    options: SourceFilterOptions,
) -> Vec<CaptureSource> {
    sources
        .into_iter()
        .filter_map(|mut source| {
            let reason = filtered_reason(&source, options);
            match reason {
                Some(reason) if options.debug_raw_sources => {
                    source.filtered_reason = Some(reason.to_string());
                    Some(source)
                }
                Some(_) => None,
                None => Some(source),
            }
        })
        .collect()
}

fn filtered_reason(source: &CaptureSource, options: SourceFilterOptions) -> Option<&'static str> {
    if source.kind == CaptureSourceKind::Window {
        if !options.include_current_app
            && options.current_pid.is_some()
            && source.pid == options.current_pid
        {
            return Some("currentApp");
        }

        if is_system_ui(source) {
            if !options.include_system_ui {
                return Some("systemUi");
            }
            return None;
        }

        if source.width < 96 || source.height < 64 {
            return Some("tooSmall");
        }
    }

    None
}

fn is_system_ui(source: &CaptureSource) -> bool {
    let title = source
        .title
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let app = source
        .app_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    matches!(
        app.as_str(),
        "dock"
            | "windowserver"
            | "systemuiserver"
            | "shellexperiencehost"
            | "startmenuexperiencehost"
    ) || matches!(
        title.as_str(),
        "dock" | "menu bar" | "taskbar" | "start" | "shell_traywnd"
    )
}
