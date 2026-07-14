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
    pub layer: i32,
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
        Self::target_at_layer(id, 0)
    }

    pub const fn target_at_layer(id: u32, layer: i32) -> Self {
        Self {
            id,
            layer,
            kind: OrderedWindowKind::Target,
        }
    }

    pub const fn panel(id: u32) -> Self {
        Self::panel_at_layer(id, 0)
    }

    pub const fn panel_at_layer(id: u32, layer: i32) -> Self {
        Self {
            id,
            layer,
            kind: OrderedWindowKind::Panel,
        }
    }

    pub const fn other(id: u32) -> Self {
        Self::other_at_layer(id, 0)
    }

    pub const fn other_at_layer(id: u32, layer: i32) -> Self {
        Self {
            id,
            layer,
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
    let target_layer = windows[target_index].layer;

    immediately_above.len() == panel_ids.len()
        && immediately_above.iter().all(|window| {
            window.kind == OrderedWindowKind::Panel
                && window.layer == target_layer
                && panel_ids.contains(&window.id)
        })
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OrderVerificationState {
    generation: u64,
    pending_generation: Option<u64>,
}

impl OrderVerificationState {
    pub fn request(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.pending_generation = Some(self.generation);
        self.generation
    }

    pub const fn pending_generation(&self) -> Option<u64> {
        self.pending_generation
    }

    pub fn take_if_current(&mut self, generation: u64) -> bool {
        if self.pending_generation != Some(generation) {
            return false;
        }
        self.pending_generation = None;
        true
    }

    pub fn cancel(&mut self) {
        self.pending_generation = None;
    }
}
