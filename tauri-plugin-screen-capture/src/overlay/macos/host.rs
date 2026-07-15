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
    verify_panel_placements, visible_corner_panels,
    window_info::{display_frame, ordered_windows, window_geometry},
    OrderVerificationState, OverlayDecision, WindowFrameAction, WindowFrameTracker,
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
        host.observers.set_paused(false);
        host.refresh()
    })
}

pub(crate) fn hide(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        host.requested_visible = false;
        host.observers.set_paused(true);
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

pub(crate) fn track_window_frame(session_id: u64) -> Result<()> {
    with_host(session_id, OverlayHost::track_window_frame)
}

pub(crate) fn verify_pending_order(session_id: u64, generation: u64) {
    let _ = with_host(session_id, |host| {
        let target_id = host.target.source_id.clone();
        let result = host.verify_pending_order(generation);
        if let Err(error) = &result {
            tracing::debug!(
                %error,
                session_id,
                %target_id,
                generation,
                "macOS 浮层延迟层级校验失败"
            );
        }
        result
    });
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
    panel_visibility: [bool; 4],
    window_frame_tracker: WindowFrameTracker,
    order_verification: OrderVerificationState,
}

impl OverlayHost {
    fn new(session_id: u64, target: OverlayTarget) -> Result<Self> {
        let track_window_frame = target.source_kind == CaptureSourceKind::Window;
        Ok(Self {
            session_id,
            target,
            panels: CornerPanel::create_set(session_id)?,
            requested_visible: true,
            observers: EventObservers::new(session_id, track_window_frame)?,
            last_frame: None,
            last_level: None,
            panels_visible: false,
            panel_visibility: [false; 4],
            window_frame_tracker: WindowFrameTracker::default(),
            order_verification: OrderVerificationState::default(),
        })
    }

    fn refresh(&mut self) -> Result<()> {
        if !self.requested_visible
            || (self.target.source_kind == CaptureSourceKind::Window
                && self.window_frame_tracker.is_moving())
        {
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
        self.panel_visibility = [true; 4];
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
        self.refresh_window_geometry(target_id, geometry)
    }

    fn refresh_window_geometry(
        &mut self,
        target_id: u32,
        geometry: super::window_info::WindowGeometry,
    ) -> Result<()> {
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
        let panel_frames = corner_panel_frames(geometry.frame, 32.0);
        let panel_visibility = visible_corner_panels(&current_order, target_id, &panel_frames);
        let expected_panels = panel_ids
            .into_iter()
            .zip(panel_frames)
            .zip(panel_visibility)
            .filter_map(|((panel_id, frame), visible)| visible.then_some((panel_id, frame)))
            .collect::<Vec<_>>();
        let order_valid = self.panel_visibility == panel_visibility
            && verify_panel_placements(&current_order, target_id, &expected_panels);
        let bounds_changed = self.last_frame != Some(geometry.frame)
            || self.last_level != Some(level)
            || self.panel_visibility != panel_visibility;
        if !needs_native_update(self.panels_visible, bounds_changed, order_valid) {
            self.window_frame_tracker.synchronize(geometry.frame);
            return Ok(());
        }

        self.apply_frames(geometry.frame);
        for (panel, visible) in self.panels.iter().zip(panel_visibility) {
            if visible {
                panel.set_level(level);
                panel.order_above(target_id);
            } else {
                panel.hide();
            }
        }

        self.last_frame = Some(geometry.frame);
        self.last_level = Some(level);
        self.panels_visible = true;
        self.panel_visibility = panel_visibility;
        self.window_frame_tracker.synchronize(geometry.frame);
        let generation = self.order_verification.request();
        schedule_order_verification(self.session_id, generation);
        Ok(())
    }

    fn track_window_frame(&mut self) -> Result<()> {
        if self.target.source_kind != CaptureSourceKind::Window || !self.requested_visible {
            return Ok(());
        }
        let target_id = parse_source_id(&self.target.source_id, "window")?;
        let geometry = match window_geometry(target_id) {
            Ok(Some(geometry)) => geometry,
            Ok(None) => {
                self.hide_panels();
                return Ok(());
            }
            Err(error) => {
                self.hide_panels();
                return Err(error);
            }
        };
        if decide_window_overlay(&geometry.snapshot, true) == OverlayDecision::Hide {
            self.hide_panels();
            return Ok(());
        }

        match self
            .window_frame_tracker
            .observe_for_visibility(geometry.frame, self.panels_visible)
        {
            WindowFrameAction::Keep => Ok(()),
            WindowFrameAction::Hide => {
                self.hide_panels();
                Ok(())
            }
            WindowFrameAction::Refresh => self.refresh_window_geometry(target_id, geometry),
        }
    }

    fn verify_pending_order(&mut self, generation: u64) -> Result<()> {
        if !self.order_verification.take_if_current(generation) {
            return Ok(());
        }
        if !self.requested_visible || !self.panels_visible {
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
        let Some(last_frame) = self.last_frame else {
            self.hide_panels();
            return Err(overlay_error("浮层缺少目标窗口边界"));
        };
        let expected_panels = panel_ids
            .into_iter()
            .zip(corner_panel_frames(last_frame, 32.0))
            .zip(self.panel_visibility)
            .filter_map(|((panel_id, frame), visible)| visible.then_some((panel_id, frame)))
            .collect::<Vec<_>>();
        if verify_panel_placements(&windows, target_id, &expected_panels) {
            return Ok(());
        }

        self.hide_panels();
        Err(overlay_error("无法维持目标窗口的相对层级，已隐藏浮层"))
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
        self.panel_visibility = [false; 4];
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
