use super::{MacRect, OverlayPanelLayout};

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

pub fn lightweight_order_span(
    windows: &[OrderedWindow],
    target_id: u32,
    panel_id: u32,
) -> Option<Vec<u32>> {
    let target_index = windows
        .iter()
        .position(|window| window.id == target_id && window.kind == OrderedWindowKind::Target)?;
    let panel_index = windows
        .iter()
        .position(|window| window.id == panel_id && window.kind == OrderedWindowKind::Panel)?;
    let span_start = (panel_index < target_index).then_some(panel_index)?;

    Some(
        windows[span_start..=target_index]
            .iter()
            .map(|window| window.id)
            .collect(),
    )
}

pub fn verify_lightweight_order(window_ids: &[u32], target_id: u32, expected_span: &[u32]) -> bool {
    if expected_span.last() != Some(&target_id) {
        return false;
    }
    let Some(target_index) = window_ids.iter().position(|id| *id == target_id) else {
        return false;
    };
    let Some(span_start) = (target_index + 1).checked_sub(expected_span.len()) else {
        return false;
    };
    window_ids.get(span_start..=target_index) == Some(expected_span)
}

pub fn visible_corner_layers(
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

pub fn verify_overlay_panel_placement(
    windows: &[OrderedWindow],
    target_id: u32,
    panel_id: u32,
    layout: &OverlayPanelLayout,
    corner_visibility: [bool; 4],
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
    if !layout.panel_frame.is_valid()
        || layout.panel_frame != target_frame
        || windows
            .iter()
            .any(|window| window.kind == OrderedWindowKind::Panel && window.id != panel_id)
    {
        return false;
    }

    let Some(panel_index) = windows
        .iter()
        .position(|window| window.id == panel_id && window.kind == OrderedWindowKind::Panel)
    else {
        return false;
    };
    let panel = windows[panel_index];
    if panel_index >= target_index
        || panel.layer != target.layer
        || panel.frame != Some(layout.panel_frame)
    {
        return false;
    }

    let visible_segments = layout
        .corner_frames
        .iter()
        .zip(corner_visibility)
        .filter(|(_, visible)| *visible)
        .flat_map(|(corner, _)| corner_painted_segments(target_frame, *corner));
    let visible_segments = visible_segments.collect::<Vec<_>>();

    windows[(panel_index + 1)..target_index]
        .iter()
        .all(|window| {
            window.kind == OrderedWindowKind::Other
                && window.layer == target.layer
                && window.owner_pid == Some(target_owner_pid)
                && window.frame.is_some_and(|frame| {
                    visible_segments
                        .iter()
                        .all(|segment| !rects_intersect(frame, *segment))
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
    panel_visible: bool,
    bounds_changed: bool,
    relative_order_valid: bool,
) -> bool {
    !panel_visible || bounds_changed || !relative_order_valid
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
