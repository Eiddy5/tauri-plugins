use std::{cell::RefCell, collections::HashMap, sync::Arc};

use objc2_app_kit::NSStatusWindowLevel;

use crate::{
    annotation::AnnotationSession,
    models::{AnnotationInputTarget, CaptureErrorCode, CaptureSourceKind, CoordinateSpace},
    overlay::OverlayTarget,
    Error, Result,
};

use super::{
    decide_window_overlay,
    events::{schedule_order_verification, schedule_refresh_after_window_ordering, EventObservers},
    lightweight_order_span,
    model::mac_rects_match,
    needs_native_update,
    panel::OverlayPanel,
    verify_lightweight_order, verify_overlay_panel_placement, visible_corner_layers,
    window_info::{
        activate_window_owner, display_frame, ordered_windows, tauri_input_rect,
        visible_window_ids, window_geometry,
    },
    OrderRetryState, OrderVerificationState, OverlayDecision, OverlayPanelLayout,
    WindowFrameAction, WindowFrameTracker,
};

thread_local! {
    static HOSTS: RefCell<HashMap<u64, OverlayHost>> = RefCell::new(HashMap::new());
}

pub(crate) fn start(
    session_id: u64,
    target: OverlayTarget,
    annotation_session: Arc<AnnotationSession>,
) -> Result<()> {
    startup_activation_target(&target)?;
    let activation_target = target.clone();
    HOSTS.with_borrow_mut(|hosts| {
        if hosts.contains_key(&session_id) {
            return Err(overlay_error("macOS 浮层会话已经启动"));
        }
        hosts.insert(
            session_id,
            OverlayHost::new(session_id, target, annotation_session)?,
        );
        Ok(())
    })?;

    match activate_startup_target_with(&activation_target, activate_window_owner) {
        Ok(Some(false)) => tracing::debug!(
            session_id,
            source_id = %activation_target.source_id,
            "macOS 共享启动时未能激活目标窗口所属应用"
        ),
        Err(error) => tracing::debug!(
            %error,
            session_id,
            source_id = %activation_target.source_id,
            "macOS 共享启动时读取目标窗口所属应用失败"
        ),
        Ok(Some(true) | None) => {}
    }

    let refresh_result = if activation_target.source_kind == CaptureSourceKind::Window {
        schedule_refresh_after_window_ordering(session_id);
        Ok(())
    } else {
        refresh(session_id)
    };
    if refresh_result.is_err() {
        let _ = stop(session_id);
    }
    refresh_result
}

pub(crate) fn show(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        host.requested_visible = true;
        host.order_retry.reset();
        host.observers.set_paused(false);
        host.refresh()
    })
}

pub(crate) fn hide(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        host.requested_visible = false;
        host.observers.set_paused(true);
        host.hide_panel();
        Ok(())
    })
}

pub(crate) fn refresh(session_id: u64) -> Result<()> {
    with_host(session_id, OverlayHost::refresh)
}

pub(crate) fn refresh_after_environment_change(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        host.order_retry.reset();
        host.refresh()
    })
}

pub(crate) fn hide_transient(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        host.hide_panel();
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

pub(crate) fn set_annotation_interaction(session_id: u64, enabled: bool) -> Result<()> {
    with_host(session_id, |host| {
        if enabled {
            host.refresh()?;
        }
        host.panel.set_annotation_interaction(enabled)
    })
}

pub(crate) fn refresh_annotations(session_id: u64) -> Result<()> {
    with_host(session_id, |host| {
        host.panel.refresh_annotations();
        Ok(())
    })
}

pub(crate) fn annotation_input_target(session_id: u64) -> Result<Option<AnnotationInputTarget>> {
    HOSTS.with_borrow(|hosts| match hosts.get(&session_id) {
        Some(host) => host.annotation_input_target(),
        None => Ok(None),
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

fn startup_activation_target(target: &OverlayTarget) -> Result<Option<u32>> {
    match target.source_kind {
        CaptureSourceKind::Window => parse_source_id(&target.source_id, "window").map(Some),
        CaptureSourceKind::Display => Ok(None),
    }
}

fn activate_startup_target_with(
    target: &OverlayTarget,
    activate: impl FnOnce(u32) -> Result<bool>,
) -> Result<Option<bool>> {
    let Some(window_id) = startup_activation_target(target)? else {
        return Ok(None);
    };
    activate(window_id).map(Some)
}

struct OverlayHost {
    session_id: u64,
    target: OverlayTarget,
    panel: OverlayPanel,
    requested_visible: bool,
    observers: EventObservers,
    last_frame: Option<super::MacRect>,
    last_level: Option<i32>,
    panel_visible: bool,
    corner_visibility: [bool; 4],
    lightweight_order_span: Option<Vec<u32>>,
    window_frame_tracker: WindowFrameTracker,
    order_verification: OrderVerificationState,
    order_retry: OrderRetryState,
}

impl OverlayHost {
    fn new(
        session_id: u64,
        target: OverlayTarget,
        annotation_session: Arc<AnnotationSession>,
    ) -> Result<Self> {
        let track_window_frame = target.source_kind == CaptureSourceKind::Window;
        Ok(Self {
            session_id,
            target,
            panel: OverlayPanel::new(session_id, annotation_session)?,
            requested_visible: true,
            observers: EventObservers::new(session_id, track_window_frame)?,
            last_frame: None,
            last_level: None,
            panel_visible: false,
            corner_visibility: [false; 4],
            lightweight_order_span: None,
            window_frame_tracker: WindowFrameTracker::default(),
            order_verification: OrderVerificationState::default(),
            order_retry: OrderRetryState::default(),
        })
    }

    fn refresh(&mut self) -> Result<()> {
        if self.target.source_kind == CaptureSourceKind::Window
            && !self.panel_visible
            && self.order_retry.automatic_refresh_suppressed()
        {
            return Ok(());
        }
        if !self.requested_visible
            || (self.target.source_kind == CaptureSourceKind::Window
                && self.window_frame_tracker.is_moving())
        {
            self.hide_panel();
            return Ok(());
        }

        match self.target.source_kind {
            CaptureSourceKind::Display => self.refresh_display(),
            CaptureSourceKind::Window => self.refresh_window(),
        }
    }

    fn annotation_input_target(&self) -> Result<Option<AnnotationInputTarget>> {
        let frame = match self.target.source_kind {
            CaptureSourceKind::Display => {
                let display_id = parse_source_id(&self.target.source_id, "display")?;
                display_frame(display_id)
            }
            CaptureSourceKind::Window => {
                let window_id = parse_source_id(&self.target.source_id, "window")?;
                window_geometry(window_id)?.map(|geometry| geometry.frame)
            }
        };
        Ok(frame.map(|frame| {
            let frame = tauri_input_rect(frame);
            AnnotationInputTarget {
                x: frame.x,
                y: frame.y,
                width: frame.width,
                height: frame.height,
                coordinate_space: CoordinateSpace::Logical,
            }
        }))
    }

    fn refresh_display(&mut self) -> Result<()> {
        let display_id = parse_source_id(&self.target.source_id, "display")?;
        let Some(frame) = display_frame(display_id) else {
            self.hide_panel();
            return Err(Error::new(
                CaptureErrorCode::SourceUnavailable,
                "共享显示器已经断开或尺寸无效",
                true,
            ));
        };
        if !needs_native_update(self.panel_visible, self.last_frame != Some(frame), true) {
            return Ok(());
        }
        self.panel.update_frame(frame, [true; 4]);
        self.panel.set_level(NSStatusWindowLevel as i32);
        self.panel.order_front();
        self.last_frame = Some(frame);
        self.last_level = Some(NSStatusWindowLevel as i32);
        self.panel_visible = true;
        self.corner_visibility = [true; 4];
        Ok(())
    }

    fn refresh_window(&mut self) -> Result<()> {
        let target_id = parse_source_id(&self.target.source_id, "window")?;
        let geometry = match window_geometry(target_id) {
            Ok(Some(geometry)) => geometry,
            Ok(None) => {
                self.hide_panel();
                return Err(Error::new(
                    CaptureErrorCode::SourceUnavailable,
                    "共享窗口已经关闭或不在当前用户空间",
                    true,
                ));
            }
            Err(error) => {
                self.hide_panel();
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
            self.hide_panel();
            return Ok(());
        };

        let panel_id = self.panel.window_id();
        let current_order = match ordered_windows(target_id, &[panel_id]) {
            Ok(windows) => windows,
            Err(error) => {
                self.hide_panel();
                return Err(error);
            }
        };
        let layout = OverlayPanelLayout::new(geometry.frame, 32.0);
        let corner_visibility =
            visible_corner_layers(&current_order, target_id, &layout.corner_frames);
        let order_valid = self.corner_visibility == corner_visibility
            && verify_overlay_panel_placement(
                &current_order,
                target_id,
                panel_id,
                &layout,
                corner_visibility,
            );
        let bounds_changed = self.last_frame.map_or(true, |last_frame| {
            !mac_rects_match(last_frame, geometry.frame)
        }) || self.last_level != Some(level)
            || self.corner_visibility != corner_visibility;
        if !needs_native_update(self.panel_visible, bounds_changed, order_valid) {
            self.order_retry.verification_succeeded();
            self.lightweight_order_span =
                lightweight_order_span(&current_order, target_id, panel_id);
            self.window_frame_tracker.synchronize(geometry.frame);
            return Ok(());
        }

        self.lightweight_order_span = None;
        self.panel
            .update_frame(layout.panel_frame, corner_visibility);
        self.panel.set_level(level);
        self.panel.order_above(target_id);

        self.last_frame = Some(geometry.frame);
        self.last_level = Some(level);
        self.panel_visible = true;
        self.corner_visibility = corner_visibility;
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
                self.hide_panel();
                return Ok(());
            }
            Err(error) => {
                self.hide_panel();
                return Err(error);
            }
        };
        if decide_window_overlay(&geometry.snapshot, true) == OverlayDecision::Hide {
            self.hide_panel();
            return Ok(());
        }

        let mut action = self.window_frame_tracker.observe(geometry.frame);
        if action == WindowFrameAction::Keep
            && !self.panel_visible
            && self.order_retry.poll_allows_retry()
        {
            action = WindowFrameAction::Refresh;
        }
        if action == WindowFrameAction::Keep && self.panel_visible {
            let order_valid = visible_window_ids()
                .ok()
                .zip(self.lightweight_order_span.as_deref())
                .is_some_and(|(window_ids, expected_span)| {
                    verify_lightweight_order(&window_ids, target_id, expected_span)
                });
            action = action.refresh_if_order_invalid(order_valid);
        }

        match action {
            WindowFrameAction::Keep => Ok(()),
            WindowFrameAction::Hide => {
                self.order_retry.reset();
                self.hide_panel();
                Ok(())
            }
            WindowFrameAction::Refresh => self.refresh_window_geometry(target_id, geometry),
        }
    }

    fn verify_pending_order(&mut self, generation: u64) -> Result<()> {
        if !self.order_verification.take_if_current(generation) {
            return Ok(());
        }
        if !self.requested_visible || !self.panel_visible {
            return Ok(());
        }

        let target_id = match parse_source_id(&self.target.source_id, "window") {
            Ok(target_id) => target_id,
            Err(error) => {
                self.order_retry.verification_failed();
                self.hide_panel();
                return Err(error);
            }
        };
        let panel_id = self.panel.window_id();
        let windows = match ordered_windows(target_id, &[panel_id]) {
            Ok(windows) => windows,
            Err(error) => {
                self.order_retry.verification_failed();
                self.hide_panel();
                return Err(error);
            }
        };
        let Some(last_frame) = self.last_frame else {
            self.order_retry.verification_failed();
            self.hide_panel();
            return Err(overlay_error("浮层缺少目标窗口边界"));
        };
        let layout = OverlayPanelLayout::new(last_frame, 32.0);
        let corner_visibility = visible_corner_layers(&windows, target_id, &layout.corner_frames);
        if corner_visibility == self.corner_visibility
            && verify_overlay_panel_placement(
                &windows,
                target_id,
                panel_id,
                &layout,
                corner_visibility,
            )
        {
            self.order_retry.verification_succeeded();
            self.lightweight_order_span = lightweight_order_span(&windows, target_id, panel_id);
            return Ok(());
        }

        self.order_retry.verification_failed();
        self.hide_panel();
        Err(overlay_error("无法维持目标窗口的相对层级，已隐藏浮层"))
    }

    fn hide_panel(&mut self) {
        self.order_verification.cancel();
        self.lightweight_order_span = None;
        self.panel.hide();
        self.panel_visible = false;
        self.corner_visibility = [false; 4];
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn only_window_sharing_requests_startup_application_activation() {
        let window = OverlayTarget {
            source_id: "window:42".to_owned(),
            source_kind: CaptureSourceKind::Window,
        };
        let display = OverlayTarget {
            source_id: "display:7".to_owned(),
            source_kind: CaptureSourceKind::Display,
        };

        assert_eq!(startup_activation_target(&window).unwrap(), Some(42));
        assert_eq!(startup_activation_target(&display).unwrap(), None);
    }

    #[test]
    fn startup_activation_calls_the_window_activator_once_and_skips_displays() {
        let window = OverlayTarget {
            source_id: "window:42".to_owned(),
            source_kind: CaptureSourceKind::Window,
        };
        let display = OverlayTarget {
            source_id: "display:7".to_owned(),
            source_kind: CaptureSourceKind::Display,
        };
        let calls = Cell::new(0);
        let activate = |window_id| {
            assert_eq!(window_id, 42);
            calls.set(calls.get() + 1);
            Ok(false)
        };

        assert_eq!(
            activate_startup_target_with(&window, activate).unwrap(),
            Some(false)
        );
        assert_eq!(
            activate_startup_target_with(&display, |_| panic!("显示器共享不应激活应用")).unwrap(),
            None
        );
        assert_eq!(calls.get(), 1);
    }
}
