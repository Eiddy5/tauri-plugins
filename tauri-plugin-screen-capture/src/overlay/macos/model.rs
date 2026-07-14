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
