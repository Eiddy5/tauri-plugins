#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowSnapshot {
    pub id: u32,
    pub layer: i32,
    pub order: usize,
    pub on_screen: bool,
    pub minimized: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayDecision {
    Show {
        target_id: u32,
        level: i32,
        target_order: usize,
    },
    Hide,
}

pub fn decide_window_overlay(
    target: &WindowSnapshot,
    relative_order_supported: bool,
) -> OverlayDecision {
    if !target.on_screen || target.minimized || !relative_order_supported {
        return OverlayDecision::Hide;
    }

    OverlayDecision::Show {
        target_id: target.id,
        level: target.layer,
        target_order: target.order,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrderedWindow {
    pub id: u32,
    kind: OrderedWindowKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OrderedWindowKind {
    Target,
    Panel,
    Other,
}

impl OrderedWindow {
    pub const fn target(id: u32) -> Self {
        Self {
            id,
            kind: OrderedWindowKind::Target,
        }
    }

    pub const fn panel(id: u32) -> Self {
        Self {
            id,
            kind: OrderedWindowKind::Panel,
        }
    }

    pub const fn other(id: u32) -> Self {
        Self {
            id,
            kind: OrderedWindowKind::Other,
        }
    }
}

pub fn verify_relative_order(windows: &[OrderedWindow], target_id: u32, panel_ids: &[u32]) -> bool {
    if panel_ids.is_empty() {
        return false;
    }
    let Some(target_index) = windows
        .iter()
        .position(|window| window.id == target_id && window.kind == OrderedWindowKind::Target)
    else {
        return false;
    };
    let Some(panel_start) = target_index.checked_sub(panel_ids.len()) else {
        return false;
    };
    let immediately_above = &windows[panel_start..target_index];

    immediately_above.len() == panel_ids.len()
        && immediately_above
            .iter()
            .all(|window| window.kind == OrderedWindowKind::Panel && panel_ids.contains(&window.id))
        && panel_ids.iter().all(|panel_id| {
            immediately_above
                .iter()
                .any(|window| window.id == *panel_id)
        })
        && windows[..panel_start]
            .iter()
            .all(|window| !panel_ids.contains(&window.id))
}

pub const fn needs_native_update(
    panels_visible: bool,
    bounds_changed: bool,
    relative_order_valid: bool,
) -> bool {
    !panels_visible || bounds_changed || !relative_order_valid
}
