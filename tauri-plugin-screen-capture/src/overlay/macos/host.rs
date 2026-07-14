use std::{cell::RefCell, collections::HashMap};

use objc2_app_kit::NSStatusWindowLevel;

use crate::{
    models::{CaptureErrorCode, CaptureSourceKind},
    overlay::OverlayTarget,
    Error, Result,
};

use super::{
    corner_panel_frames, decide_window_overlay,
    events::{schedule_order_verification, EventObservers},
    needs_native_update,
    panel::CornerPanel,
    verify_relative_order,
    window_info::{display_frame, ordered_windows, window_geometry},
    OrderVerificationState, OverlayDecision,
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
        host.observers.set_correction_paused(host.dragging);
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

pub(crate) fn verify_pending_order(session_id: u64) {
    let result = with_host(session_id, OverlayHost::verify_pending_order);
    if let Err(error) = result {
        tracing::debug!(%error, session_id, "macOS 浮层延迟层级校验失败");
    }
}

pub(crate) fn begin_drag(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        if host.target.source_kind == CaptureSourceKind::Window {
            host.dragging = true;
            host.observers.set_correction_paused(true);
            host.hide_panels();
        }
        Ok(())
    })
}

pub(crate) fn end_drag(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        if host.target.source_kind == CaptureSourceKind::Window {
            host.dragging = false;
            host.observers
                .set_correction_paused(!host.requested_visible);
            return host.refresh();
        }
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
    session_id: u64,
    target: OverlayTarget,
    panels: [CornerPanel; 4],
    requested_visible: bool,
    observers: EventObservers,
    last_frame: Option<super::MacRect>,
    last_level: Option<i32>,
    panels_visible: bool,
    dragging: bool,
    order_verification: OrderVerificationState,
}

impl OverlayHost {
    fn new(session_id: u64, target: OverlayTarget) -> Result<Self> {
        Ok(Self {
            session_id,
            target,
            panels: CornerPanel::create_set(session_id)?,
            requested_visible: true,
            observers: EventObservers::new(session_id)?,
            last_frame: None,
            last_level: None,
            panels_visible: false,
            dragging: false,
            order_verification: OrderVerificationState::default(),
        })
    }

    fn refresh(&mut self) -> Result<()> {
        if !self.requested_visible || self.dragging {
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
        let Some(frame) = display_frame(display_id) else {
            self.hide_panels();
            return Err(Error::new(
                CaptureErrorCode::SourceUnavailable,
                "共享显示器已经断开或尺寸无效",
                true,
            ));
        };
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
        let geometry = match window_geometry(target_id) {
            Ok(Some(geometry)) => geometry,
            Ok(None) => {
                self.hide_panels();
                return Err(Error::new(
                    CaptureErrorCode::SourceUnavailable,
                    "共享窗口已经关闭或不在当前用户空间",
                    true,
                ));
            }
            Err(error) => {
                self.hide_panels();
                return Err(error);
            }
        };
        let OverlayDecision::Show { level, .. } = decide_window_overlay(&geometry.snapshot, true)
        else {
            self.hide_panels();
            return Ok(());
        };

        let panel_ids = self.panel_ids();
        let current_order = match ordered_windows(target_id, &panel_ids) {
            Ok(windows) => windows,
            Err(error) => {
                self.hide_panels();
                return Err(error);
            }
        };
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

        self.last_frame = Some(geometry.frame);
        self.last_level = Some(level);
        self.panels_visible = true;
        if self.order_verification.request() {
            schedule_order_verification(self.session_id);
        }
        Ok(())
    }

    fn verify_pending_order(&mut self) -> Result<()> {
        let Some(generation) = self.order_verification.take_pending() else {
            return Ok(());
        };
        if !self.requested_visible || self.dragging || !self.panels_visible {
            return Ok(());
        }

        let target_id = match parse_source_id(&self.target.source_id, "window") {
            Ok(target_id) => target_id,
            Err(error) => {
                self.hide_panels();
                return Err(error);
            }
        };
        let panel_ids = self.panel_ids();
        let windows = match ordered_windows(target_id, &panel_ids) {
            Ok(windows) => windows,
            Err(error) => {
                self.hide_panels();
                return Err(error);
            }
        };
        if verify_relative_order(&windows, target_id, &panel_ids) {
            return Ok(());
        }

        self.hide_panels();
        Err(overlay_error(format!(
            "无法维持目标窗口 {target_id} 的相对层级，已隐藏浮层（校验代次 {generation}）"
        )))
    }

    fn apply_frames(&self, target: super::MacRect) {
        for (panel, frame) in self.panels.iter().zip(corner_panel_frames(target, 32.0)) {
            panel.update_frame(frame);
        }
    }

    fn hide_panels(&mut self) {
        self.order_verification.cancel();
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
