use std::{ptr::NonNull, time::Duration};

use block2::RcBlock;
use objc2::{
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    MainThreadMarker,
};
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification, NSWorkspaceDidActivateApplicationNotification,
    NSWorkspaceDidHideApplicationNotification, NSWorkspaceDidTerminateApplicationNotification,
    NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSDate, NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol, NSTimer,
};

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayEvent {
    WindowPositionTimer,
    ApplicationActivated,
    ApplicationHidden,
    ApplicationTerminated,
    ActiveSpaceChanged,
    ScreenParametersChanged,
    SystemWillSleep,
    SystemWoke,
    CorrectionTimer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshAction {
    Hide,
    Refresh,
    RefreshAfterWindowOrdering,
    TrackWindowFrame,
}

pub const fn event_action(event: OverlayEvent) -> RefreshAction {
    match event {
        OverlayEvent::SystemWillSleep => RefreshAction::Hide,
        OverlayEvent::WindowPositionTimer => RefreshAction::TrackWindowFrame,
        OverlayEvent::ApplicationActivated => RefreshAction::RefreshAfterWindowOrdering,
        OverlayEvent::ApplicationHidden
        | OverlayEvent::ApplicationTerminated
        | OverlayEvent::ActiveSpaceChanged
        | OverlayEvent::ScreenParametersChanged
        | OverlayEvent::SystemWoke
        | OverlayEvent::CorrectionTimer => RefreshAction::Refresh,
    }
}

pub const WINDOW_POSITION_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WINDOW_SERVER_COMMIT_DELAY: Duration = Duration::from_millis(50);
const CORRECTION_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) struct EventObservers {
    notification_tokens: Vec<NotificationToken>,
    position_timer: Option<Retained<NSTimer>>,
    correction_timer: Retained<NSTimer>,
}

struct NotificationToken {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl EventObservers {
    pub(crate) fn new(session_id: u64, track_window_frame: bool) -> Result<Self> {
        let workspace_center = NSWorkspace::sharedWorkspace().notificationCenter();
        let default_center = NSNotificationCenter::defaultCenter();
        let notification_tokens = vec![
            observe(
                &workspace_center,
                unsafe { NSWorkspaceDidActivateApplicationNotification },
                session_id,
                OverlayEvent::ApplicationActivated,
            ),
            observe(
                &workspace_center,
                unsafe { NSWorkspaceDidHideApplicationNotification },
                session_id,
                OverlayEvent::ApplicationHidden,
            ),
            observe(
                &workspace_center,
                unsafe { NSWorkspaceDidTerminateApplicationNotification },
                session_id,
                OverlayEvent::ApplicationTerminated,
            ),
            observe(
                &workspace_center,
                unsafe { NSWorkspaceActiveSpaceDidChangeNotification },
                session_id,
                OverlayEvent::ActiveSpaceChanged,
            ),
            observe(
                &workspace_center,
                unsafe { NSWorkspaceWillSleepNotification },
                session_id,
                OverlayEvent::SystemWillSleep,
            ),
            observe(
                &workspace_center,
                unsafe { NSWorkspaceDidWakeNotification },
                session_id,
                OverlayEvent::SystemWoke,
            ),
            observe(
                &default_center,
                unsafe { NSApplicationDidChangeScreenParametersNotification },
                session_id,
                OverlayEvent::ScreenParametersChanged,
            ),
        ];

        let position_timer = track_window_frame.then(|| {
            let block = RcBlock::new(move |_: NonNull<NSTimer>| {
                dispatch(session_id, OverlayEvent::WindowPositionTimer);
            });
            // SAFETY: The block captures only a numeric session ID and runs on the main run loop.
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_repeats_block(
                    WINDOW_POSITION_POLL_INTERVAL.as_secs_f64(),
                    true,
                    &block,
                )
            };
            timer.setTolerance(0.01);
            timer
        });

        let correction_block = RcBlock::new(move |_: NonNull<NSTimer>| {
            dispatch(session_id, OverlayEvent::CorrectionTimer);
        });
        // SAFETY: The block captures only a numeric session ID and runs on the main run loop.
        let correction_timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(
                CORRECTION_INTERVAL.as_secs_f64(),
                true,
                &correction_block,
            )
        };
        correction_timer.setTolerance(0.5);

        Ok(Self {
            notification_tokens,
            position_timer,
            correction_timer,
        })
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        let correction_fire = if paused {
            NSDate::distantFuture()
        } else {
            NSDate::dateWithTimeIntervalSinceNow(CORRECTION_INTERVAL.as_secs_f64())
        };
        self.correction_timer.setFireDate(&correction_fire);
        if let Some(timer) = &self.position_timer {
            let position_fire = if paused {
                NSDate::distantFuture()
            } else {
                NSDate::dateWithTimeIntervalSinceNow(WINDOW_POSITION_POLL_INTERVAL.as_secs_f64())
            };
            timer.setFireDate(&position_fire);
        }
    }
}

pub(crate) fn schedule_order_verification(session_id: u64, generation: u64) {
    let block = RcBlock::new(move |_: NonNull<NSTimer>| {
        super::host::verify_pending_order(session_id, generation);
    });
    // SAFETY: The block captures only numeric IDs. A short delay gives Window Server time to
    // commit the cross-process relative order before the one-shot performs strict validation.
    let _timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(
            WINDOW_SERVER_COMMIT_DELAY.as_secs_f64(),
            false,
            &block,
        )
    };
}

pub(crate) fn schedule_refresh_after_window_ordering(session_id: u64) {
    let block = RcBlock::new(move |_: NonNull<NSTimer>| {
        if let Err(error) = super::host::refresh_after_environment_change(session_id) {
            tracing::debug!(%error, session_id, "macOS 浮层聚焦后刷新失败");
        }
    });
    // SAFETY: The block captures only the session ID. The short delay lets AppKit finish the
    // activation transaction before this one-shot re-establishes cross-process relative order.
    let _timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(
            WINDOW_SERVER_COMMIT_DELAY.as_secs_f64(),
            false,
            &block,
        )
    };
}

impl Drop for EventObservers {
    fn drop(&mut self) {
        if let Some(timer) = &self.position_timer {
            timer.invalidate();
        }
        self.correction_timer.invalidate();
        for observer in &self.notification_tokens {
            let token: &ProtocolObject<dyn NSObjectProtocol> = &observer.token;
            let token: &AnyObject = token.as_ref();
            // SAFETY: token was created by this exact notification center.
            unsafe { observer.center.removeObserver(token) };
        }
    }
}

fn observe(
    center: &Retained<NSNotificationCenter>,
    name: &NSNotificationName,
    session_id: u64,
    event: OverlayEvent,
) -> NotificationToken {
    let block = RcBlock::new(move |_: NonNull<NSNotification>| dispatch(session_id, event));
    // SAFETY: No object filter is used, the block is Send and the center retains it.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
    };
    NotificationToken {
        center: center.clone(),
        token,
    }
}

fn dispatch(session_id: u64, event: OverlayEvent) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let result = match event_action(event) {
        RefreshAction::Hide => super::host::hide_transient(session_id),
        RefreshAction::Refresh if event == OverlayEvent::CorrectionTimer => {
            super::host::refresh(session_id)
        }
        RefreshAction::Refresh => super::host::refresh_after_environment_change(session_id),
        RefreshAction::RefreshAfterWindowOrdering => {
            schedule_refresh_after_window_ordering(session_id);
            Ok(())
        }
        RefreshAction::TrackWindowFrame => super::host::track_window_frame(session_id),
    };
    if let Err(error) = result {
        tracing::debug!(%error, session_id, ?event, "macOS 浮层事件刷新失败");
    }
}
