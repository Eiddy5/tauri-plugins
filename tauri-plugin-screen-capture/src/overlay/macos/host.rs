use std::{cell::RefCell, collections::HashMap};

use objc2_app_kit::NSStatusWindowLevel;

use crate::{
    models::{CaptureErrorCode, CaptureSourceKind},
    overlay::OverlayTarget,
    Error, Result,
};

use super::{
    corner_panel_frames, decide_window_overlay,
    events::EventObservers,
    needs_native_update,
    panel::CornerPanel,
    verify_relative_order,
    window_info::{display_frame, ordered_windows, window_geometry},
    OverlayDecision,
};

thread_local! {
    static HOSTS: RefCell<HashMap<u64, OverlayHost>> = RefCell::new(HashMap::new());
}

pub(crate) fn start(session_id: u64, target: OverlayTarget) -> Result<()> {
    HOSTS.with_borrow_mut(|hosts| {
        if hosts.contains_key(&session_id) {
            return Err(overlay_error("macOS 浮层会话已经启动"));
        }
        let mut host = OverlayHost::new(session_id, target)?;
        host.refresh()?;
        hosts.insert(session_id, host);
        Ok(())
    })
}

pub(crate) fn show(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        host.requested_visible = true;
        host.observers.set_correction_paused(false);
        host.refresh()
    })
}

pub(crate) fn hide(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        host.requested_visible = false;
        host.observers.set_correction_paused(true);
        host.hide_panels();
        Ok(())
    })
}

pub(crate) fn refresh(session_id: u64) -> Result<()> {
    with_host(session_id, OverlayHost::refresh)
}

pub(crate) fn hide_transient(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        host.hide_panels();
        Ok(())
    })
}

pub(crate) fn stop(session_id: u64) -> Result<()> {
    HOSTS.with_borrow_mut(|hosts| {
        hosts.remove(&session_id);
        Ok(())
    })
}

fn with_host(
    session_id: u64,
    operation: impl FnOnce(&mut OverlayHost) -> Result<()>,
) -> Result<()> {
    HOSTS.with_borrow_mut(|hosts| match hosts.get_mut(&session_id) {
        Some(host) => operation(host),
        None => Ok(()),
    })
}

struct OverlayHost {
    target: OverlayTarget,
    panels: [CornerPanel; 4],
    requested_visible: bool,
    observers: EventObservers,
    last_frame: Option<super::MacRect>,
    last_level: Option<i32>,
    panels_visible: bool,
}

impl OverlayHost {
    fn new(session_id: u64, target: OverlayTarget) -> Result<Self> {
        Ok(Self {
            target,
            panels: CornerPanel::create_set(session_id)?,
            requested_visible: true,
            observers: EventObservers::new(session_id)?,
            last_frame: None,
            last_level: None,
            panels_visible: false,
        })
    }

    fn refresh(&mut self) -> Result<()> {
        if !self.requested_visible {
            self.hide_panels();
            return Ok(());
        }

        match self.target.source_kind {
            CaptureSourceKind::Display => self.refresh_display(),
            CaptureSourceKind::Window => self.refresh_window(),
        }
    }

    fn refresh_display(&mut self) -> Result<()> {
        let display_id = parse_source_id(&self.target.source_id, "display")?;
        let frame = display_frame(display_id);
        if !needs_native_update(self.panels_visible, self.last_frame != Some(frame), true) {
            return Ok(());
        }
        self.apply_frames(frame);
        for panel in &self.panels {
            panel.set_level(NSStatusWindowLevel as i32);
            panel.order_front();
        }
        self.last_frame = Some(frame);
        self.last_level = Some(NSStatusWindowLevel as i32);
        self.panels_visible = true;
        Ok(())
    }

    fn refresh_window(&mut self) -> Result<()> {
        let target_id = parse_source_id(&self.target.source_id, "window")?;
        let Some(geometry) = window_geometry(target_id)? else {
            self.hide_panels();
            return Err(Error::new(
                CaptureErrorCode::SourceUnavailable,
                "共享窗口已经关闭或不在当前用户空间",
                true,
            ));
        };
        let OverlayDecision::Show { level, .. } = decide_window_overlay(&geometry.snapshot, true)
        else {
            self.hide_panels();
            return Ok(());
        };

        let panel_ids = self.panel_ids();
        let current_order = ordered_windows(target_id, &panel_ids)?;
        let order_valid = verify_relative_order(&current_order, target_id, &panel_ids);
        let bounds_changed =
            self.last_frame != Some(geometry.frame) || self.last_level != Some(level);
        if !needs_native_update(self.panels_visible, bounds_changed, order_valid) {
            return Ok(());
        }

        self.apply_frames(geometry.frame);
        for panel in &self.panels {
            panel.set_level(level);
            panel.order_above(target_id);
        }

        let windows = ordered_windows(target_id, &panel_ids)?;
        if !verify_relative_order(&windows, target_id, &panel_ids) {
            self.hide_panels();
            return Err(overlay_error("无法维持目标窗口的相对层级，已隐藏浮层"));
        }
        self.last_frame = Some(geometry.frame);
        self.last_level = Some(level);
        self.panels_visible = true;
        Ok(())
    }

    fn apply_frames(&self, target: super::MacRect) {
        for (panel, frame) in self.panels.iter().zip(corner_panel_frames(target, 32.0)) {
            panel.update_frame(frame);
        }
    }

    fn hide_panels(&mut self) {
        for panel in &self.panels {
            panel.hide();
        }
        self.panels_visible = false;
    }

    fn panel_ids(&self) -> [u32; 4] {
        self.panels.each_ref().map(CornerPanel::window_id)
    }
}

fn parse_source_id(source_id: &str, expected_kind: &str) -> Result<u32> {
    source_id
        .strip_prefix(expected_kind)
        .and_then(|value| value.strip_prefix(':'))
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| {
            Error::new(
                CaptureErrorCode::SourceNotFound,
                format!("无法识别共享源: {source_id}"),
                true,
            )
        })
}

fn overlay_error(message: impl Into<String>) -> Error {
    Error::new(CaptureErrorCode::Internal, message, true)
}
