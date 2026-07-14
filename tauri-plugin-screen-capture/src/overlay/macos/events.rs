use std::ptr::NonNull;

use block2::RcBlock;
use objc2::{
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    MainThreadMarker,
};
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSEvent, NSEventMask, NSEventType,
    NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidHideApplicationNotification,
    NSWorkspaceDidTerminateApplicationNotification, NSWorkspaceDidWakeNotification,
    NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSDate, NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol, NSTimer,
};

use crate::{models::CaptureErrorCode, Error, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayEvent {
    LeftMouseDragged,
    LeftMouseUp,
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
}

pub const fn event_action(event: OverlayEvent) -> RefreshAction {
    match event {
        OverlayEvent::LeftMouseDragged | OverlayEvent::SystemWillSleep => RefreshAction::Hide,
        OverlayEvent::LeftMouseUp
        | OverlayEvent::ApplicationActivated
        | OverlayEvent::ApplicationHidden
        | OverlayEvent::ApplicationTerminated
        | OverlayEvent::ActiveSpaceChanged
        | OverlayEvent::ScreenParametersChanged
        | OverlayEvent::SystemWoke
        | OverlayEvent::CorrectionTimer => RefreshAction::Refresh,
    }
}

pub(crate) struct EventObservers {
    event_monitors: Vec<Retained<objc2::runtime::AnyObject>>,
    notification_tokens: Vec<NotificationToken>,
    correction_timer: Retained<NSTimer>,
}

struct NotificationToken {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl EventObservers {
    pub(crate) fn new(session_id: u64) -> Result<Self> {
        let mask = NSEventMask::LeftMouseDragged | NSEventMask::LeftMouseUp;
        let global_block = RcBlock::new(move |event: NonNull<NSEvent>| {
            dispatch_mouse_event(session_id, event);
        });
        let global = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global_block);

        let local_block = RcBlock::new(move |event: NonNull<NSEvent>| {
            dispatch_mouse_event(session_id, event);
            event.as_ptr()
        });
        // SAFETY: Returning the same live event pointer preserves AppKit's local monitor contract.
        let local =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_block) };
        if global.is_none() || local.is_none() {
            for monitor in global.iter().chain(local.iter()) {
                // SAFETY: The object was returned by an NSEvent monitor API above.
                unsafe { NSEvent::removeMonitor(monitor) };
            }
            return Err(Error::new(
                CaptureErrorCode::Internal,
                "无法安装 macOS 浮层鼠标事件监听器",
                true,
            ));
        }
        let event_monitors = global.into_iter().chain(local).collect();

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

        let timer_block = RcBlock::new(move |_: NonNull<NSTimer>| {
            dispatch(session_id, OverlayEvent::CorrectionTimer);
        });
        // SAFETY: The block captures only a numeric session ID and runs on the main run loop.
        let correction_timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(2.0, true, &timer_block)
        };
        correction_timer.setTolerance(0.5);

        Ok(Self {
            event_monitors,
            notification_tokens,
            correction_timer,
        })
    }

    pub(crate) fn set_correction_paused(&self, paused: bool) {
        let next_fire = if paused {
            NSDate::distantFuture()
        } else {
            NSDate::dateWithTimeIntervalSinceNow(2.0)
        };
        self.correction_timer.setFireDate(&next_fire);
    }
}

impl Drop for EventObservers {
    fn drop(&mut self) {
        self.correction_timer.invalidate();
        for monitor in &self.event_monitors {
            // SAFETY: Every object in event_monitors was returned by an NSEvent monitor API.
            unsafe { NSEvent::removeMonitor(monitor) };
        }
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

fn dispatch_mouse_event(session_id: u64, event: NonNull<NSEvent>) {
    // SAFETY: AppKit guarantees the monitor callback's event pointer for the callback duration.
    let event_type = unsafe { event.as_ref() }.r#type();
    let event = if event_type == NSEventType::LeftMouseDragged {
        OverlayEvent::LeftMouseDragged
    } else {
        OverlayEvent::LeftMouseUp
    };
    dispatch(session_id, event);
}

fn dispatch(session_id: u64, event: OverlayEvent) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let result = match event_action(event) {
        RefreshAction::Hide => super::host::hide_transient(session_id),
        RefreshAction::Refresh => super::host::refresh(session_id),
    };
    if let Err(error) = result {
        tracing::debug!(%error, session_id, ?event, "macOS 浮层事件刷新失败");
    }
}
