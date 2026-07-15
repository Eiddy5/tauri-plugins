use super::MacRect;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrderedWindow {
    pub id: u32,
    pub layer: i32,
    owner_pid: Option<u32>,
    frame: Option<MacRect>,
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
            owner_pid: None,
            frame: None,
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
            owner_pid: None,
            frame: None,
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
            owner_pid: None,
            frame: None,
            kind: OrderedWindowKind::Other,
        }
    }

    pub const fn with_owner_pid(mut self, owner_pid: u32) -> Self {
        self.owner_pid = Some(owner_pid);
        self
    }

    pub const fn with_frame(mut self, frame: MacRect) -> Self {
        self.frame = Some(frame);
        self
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

pub fn verify_lightweight_order(
    window_ids: &[u32],
    target_id: u32,
    visible_panel_ids: &[u32],
) -> bool {
    if visible_panel_ids.is_empty() {
        return window_ids.contains(&target_id);
    }
    let Some(target_index) = window_ids.iter().position(|id| *id == target_id) else {
        return false;
    };
    let Some(panel_start) = target_index.checked_sub(visible_panel_ids.len()) else {
        return false;
    };
    let panel_group = &window_ids[panel_start..target_index];
    visible_panel_ids
        .iter()
        .all(|panel_id| panel_group.iter().filter(|id| *id == panel_id).count() == 1)
}

pub fn visible_corner_panels(
    windows: &[OrderedWindow],
    target_id: u32,
    panel_frames: &[MacRect; 4],
) -> [bool; 4] {
    let Some(target_index) = windows
        .iter()
        .position(|window| window.id == target_id && window.kind == OrderedWindowKind::Target)
    else {
        return [false; 4];
    };
    let target = windows[target_index];
    let Some(target_owner_pid) = target.owner_pid else {
        return [false; 4];
    };
    let Some(target_frame) = target.frame.filter(|frame| frame.is_valid()) else {
        return [false; 4];
    };

    panel_frames.map(|panel_frame| {
        let painted_segments = corner_painted_segments(target_frame, panel_frame);
        panel_frame.is_valid()
            && windows[..target_index].iter().all(|window| {
                if window.kind != OrderedWindowKind::Other
                    || window.layer != target.layer
                    || window.owner_pid != Some(target_owner_pid)
                {
                    return true;
                }
                window.frame.is_some_and(|frame| {
                    painted_segments
                        .iter()
                        .all(|segment| !rects_intersect(frame, *segment))
                })
            })
    })
}

pub fn verify_panel_placements(
    windows: &[OrderedWindow],
    target_id: u32,
    panels: &[(u32, MacRect)],
) -> bool {
    let Some(target_index) = windows
        .iter()
        .position(|window| window.id == target_id && window.kind == OrderedWindowKind::Target)
    else {
        return false;
    };
    let target = windows[target_index];
    let Some(target_owner_pid) = target.owner_pid else {
        return false;
    };
    let Some(target_frame) = target.frame.filter(|frame| frame.is_valid()) else {
        return false;
    };
    if windows.iter().any(|window| {
        window.kind == OrderedWindowKind::Panel
            && !panels.iter().any(|(panel_id, _)| *panel_id == window.id)
    }) {
        return false;
    }

    panels.iter().all(|(panel_id, panel_frame)| {
        if !panel_frame.is_valid() {
            return false;
        }
        let painted_segments = corner_painted_segments(target_frame, *panel_frame);
        let Some(panel_index) = windows
            .iter()
            .position(|window| window.id == *panel_id && window.kind == OrderedWindowKind::Panel)
        else {
            return false;
        };
        if panel_index >= target_index || windows[panel_index].layer != target.layer {
            return false;
        }

        windows[(panel_index + 1)..target_index]
            .iter()
            .all(|window| match window.kind {
                OrderedWindowKind::Panel => {
                    panels
                        .iter()
                        .any(|(expected_id, _)| *expected_id == window.id)
                        && window.layer == target.layer
                }
                OrderedWindowKind::Other => {
                    window.layer == target.layer
                        && window.owner_pid == Some(target_owner_pid)
                        && window.frame.is_some_and(|frame| {
                            painted_segments
                                .iter()
                                .all(|segment| !rects_intersect(frame, *segment))
                        })
                }
                OrderedWindowKind::Target => false,
            })
    })
}

fn corner_painted_segments(target: MacRect, panel: MacRect) -> [MacRect; 2] {
    let thickness = 4.0_f64.min(panel.width).min(panel.height);
    let on_top = panel.y > target.y;
    let on_right = panel.x > target.x;
    let horizontal_y = if on_top {
        panel.y + panel.height - thickness
    } else {
        panel.y
    };
    let vertical_x = if on_right {
        panel.x + panel.width - thickness
    } else {
        panel.x
    };

    [
        MacRect {
            x: panel.x,
            y: horizontal_y,
            width: panel.width,
            height: thickness,
        },
        MacRect {
            x: vertical_x,
            y: panel.y,
            width: thickness,
            height: panel.height,
        },
    ]
}

fn rects_intersect(first: MacRect, second: MacRect) -> bool {
    first.is_valid()
        && second.is_valid()
        && first.x < second.x + second.width
        && second.x < first.x + first.width
        && first.y < second.y + second.height
        && second.y < first.y + first.height
}

pub const fn needs_native_update(
    panels_visible: bool,
    bounds_changed: bool,
    relative_order_valid: bool,
) -> bool {
    !panels_visible || bounds_changed || !relative_order_valid
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowFrameAction {
    Keep,
    Hide,
    Refresh,
}

impl WindowFrameAction {
    pub const fn refresh_if_order_invalid(self, relative_order_valid: bool) -> Self {
        match self {
            Self::Keep if !relative_order_valid => Self::Refresh,
            action => action,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowFrameTracker {
    last_frame: Option<super::MacRect>,
    moving: bool,
}

impl WindowFrameTracker {
    pub const fn is_moving(&self) -> bool {
        self.moving
    }

    pub fn synchronize(&mut self, frame: super::MacRect) {
        self.last_frame = Some(frame);
        self.moving = false;
    }

    pub fn observe(&mut self, frame: super::MacRect) -> WindowFrameAction {
        let Some(last_frame) = self.last_frame.replace(frame) else {
            return WindowFrameAction::Keep;
        };
        if last_frame != frame {
            self.moving = true;
            return WindowFrameAction::Hide;
        }
        if self.moving {
            self.moving = false;
            return WindowFrameAction::Refresh;
        }
        WindowFrameAction::Keep
    }

    pub fn observe_for_visibility(
        &mut self,
        frame: super::MacRect,
        overlay_visible: bool,
    ) -> WindowFrameAction {
        match self.observe(frame) {
            WindowFrameAction::Keep if !overlay_visible => WindowFrameAction::Refresh,
            action => action,
        }
    }
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
