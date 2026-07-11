mod bounds;
mod events;
mod host;
mod placement;
mod util;
mod window;

use std::sync::{mpsc, Mutex};

use async_trait::async_trait;

use crate::{
    error::Error,
    models::{CaptureErrorCode, CaptureSourceKind},
    overlay::{OverlayRect, OverlayStyle, OverlayTarget, ShareOverlay},
    Result,
};

use bounds::{display_bounds_from_source_id, window_bounds_from_source_id};
use events::next_window_event_registration_id;
use host::{send_overlay_command, OverlayCommand, OverlayHost};
use placement::{OverlayFocus, OverlayPlacement};

pub use bounds::{windows_display_index_from_source_id, windows_target_handle_from_source_id};

pub struct WindowsShareOverlay {
    style: OverlayStyle,
    host: Mutex<Option<OverlayHost>>,
    window_event_registration_id: u64,
}

impl Default for WindowsShareOverlay {
    fn default() -> Self {
        Self {
            style: OverlayStyle::default(),
            host: Mutex::new(None),
            window_event_registration_id: next_window_event_registration_id(),
        }
    }
}

#[async_trait]
impl ShareOverlay for WindowsShareOverlay {
    async fn start(&self, target: OverlayTarget) -> Result<()> {
        match target.source_kind {
            CaptureSourceKind::Window => {
                let target_hwnd = match windows_target_handle_from_source_id(&target.source_id) {
                    Some(target_hwnd) => target_hwnd,
                    None => {
                        self.clear_window_event_hooks().await?;
                        self.hide().await?;
                        return Ok(());
                    }
                };
                let sender = self.ensure_host_sender()?;
                let rect = match window_bounds_from_source_id(&target.source_id) {
                    Ok(rect) => rect,
                    Err(_) => {
                        self.clear_window_event_hooks().await?;
                        send_overlay_command(sender, OverlayCommand::hide).await?;
                        return Ok(());
                    }
                };
                let source_id = target.source_id.clone();
                let style = self.style;
                let registration_id = self.window_event_registration_id;
                let event_sender = sender.clone();

                send_overlay_command(sender.clone(), move |reply| {
                    OverlayCommand::RegisterWindowEventTarget {
                        registration_id,
                        target_hwnd,
                        source_id,
                        style,
                        event_sender,
                        reply,
                    }
                })
                .await?;

                match rect {
                    Some(rect) => {
                        let style = self.style;
                        let placement = OverlayPlacement::for_window(target_hwnd);
                        send_overlay_command(sender, move |reply| OverlayCommand::ShowRect {
                            rect,
                            style,
                            placement,
                            focus: OverlayFocus::for_window_start(target_hwnd),
                            reply,
                        })
                        .await
                    }
                    None => {
                        send_overlay_command(sender, OverlayCommand::hide).await?;
                        Ok(())
                    }
                }
            }
            CaptureSourceKind::Display => {
                self.clear_window_event_hooks().await?;
                match display_bounds_from_source_id(&target.source_id) {
                    Some(rect) => self.show_rect(rect).await,
                    None => {
                        self.hide().await?;
                        Ok(())
                    }
                }
            }
        }
    }

    async fn show(&self) -> Result<()> {
        if let Some(sender) = self.host_sender()? {
            send_overlay_command(sender, OverlayCommand::show).await?;
        }

        Ok(())
    }

    async fn hide(&self) -> Result<()> {
        if let Some(sender) = self.host_sender()? {
            send_overlay_command(sender, OverlayCommand::hide).await?;
        }

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        if let Some(host) = self.take_host()? {
            host.shutdown().await?;
        }

        Ok(())
    }
}

impl WindowsShareOverlay {
    async fn show_rect(&self, rect: OverlayRect) -> Result<()> {
        let sender = self.ensure_host_sender()?;
        let style = self.style;
        let placement = OverlayPlacement::for_display();

        send_overlay_command(sender, move |reply| OverlayCommand::ShowRect {
            rect,
            style,
            placement,
            focus: OverlayFocus::none(),
            reply,
        })
        .await
    }

    fn ensure_host_sender(&self) -> Result<mpsc::Sender<OverlayCommand>> {
        let mut host = self.lock_host()?;

        if host.is_none() {
            *host = Some(OverlayHost::spawn()?);
        }

        Ok(host
            .as_ref()
            .expect("overlay host must exist after initialization")
            .sender
            .clone())
    }

    fn host_sender(&self) -> Result<Option<mpsc::Sender<OverlayCommand>>> {
        Ok(self.lock_host()?.as_ref().map(|host| host.sender.clone()))
    }

    fn take_host(&self) -> Result<Option<OverlayHost>> {
        Ok(self.lock_host()?.take())
    }

    fn lock_host(&self) -> Result<std::sync::MutexGuard<'_, Option<OverlayHost>>> {
        self.host.lock().map_err(|_| {
            Error::new(
                CaptureErrorCode::Internal,
                "share border overlay host state lock was poisoned",
                false,
            )
        })
    }

    async fn clear_window_event_hooks(&self) -> Result<()> {
        if let Some(sender) = self.host_sender()? {
            let registration_id = self.window_event_registration_id;
            send_overlay_command(sender, move |reply| {
                OverlayCommand::ClearWindowEventTarget {
                    registration_id,
                    reply,
                }
            })
            .await?;
        }

        Ok(())
    }
}

#[cfg(test)]
use bounds::preferred_window_bounds;
#[cfg(test)]
use events::{
    dispatch_window_event, register_window_event_hook_handles, register_window_event_target,
    unregister_window_event_target, window_event_hook_ranges,
};
#[cfg(test)]
use host::{deferred_refresh_is_ready, OverlayThread, PendingWindowEventRefresh};
#[cfg(test)]
use window::OverlayWindow;
#[cfg(test)]
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_DESTROY, EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, OBJID_WINDOW,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{HWND, RECT},
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, GetWindow, GetWindowRect, IsWindowVisible,
                SetWindowPos, ShowWindow, GW_HWNDPREV, HMENU, HWND_TOP, SWP_NOACTIVATE,
                SWP_SHOWWINDOW, SW_MINIMIZE, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
            },
        },
    };

    use super::util::wide;

    trait AmbiguousIfSend<A> {
        fn assert_not_send() {}
    }

    impl<T: ?Sized> AmbiguousIfSend<()> for T {}

    struct Invalid;

    impl<T: ?Sized + Send> AmbiguousIfSend<Invalid> for T {}

    #[test]
    fn overlay_windows_are_not_send() {
        <OverlayWindow as AmbiguousIfSend<_>>::assert_not_send();
    }

    #[test]
    fn window_capture_overlay_uses_owned_non_topmost_placement() {
        let placement = OverlayPlacement::for_window(0x2a);

        assert_eq!(placement.owner_hwnd, Some(0x2a));
        assert!(!placement.topmost);
    }

    #[test]
    fn display_capture_overlay_uses_topmost_placement() {
        let placement = OverlayPlacement::for_display();

        assert_eq!(placement.owner_hwnd, None);
        assert!(placement.topmost);
    }

    #[test]
    fn window_capture_start_focuses_owner_once() {
        assert_eq!(
            OverlayFocus::for_window_start(0x2a),
            OverlayFocus::OwnerOnce(0x2a)
        );
        assert_eq!(OverlayFocus::none(), OverlayFocus::None);
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn owned_overlay_tracks_real_window_geometry_and_z_order() {
        let class_name = wide("STATIC");
        let owner_title = wide("screen-capture-overlay-owner-test");
        let cover_title = wide("screen-capture-overlay-cover-test");
        let owner = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(owner_title.as_ptr()),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                100,
                120,
                640,
                480,
                None,
                Some(HMENU::default()),
                None,
                None,
            )
        }
        .expect("create real overlay owner window");
        let cover = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(cover_title.as_ptr()),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                80,
                100,
                680,
                520,
                None,
                Some(HMENU::default()),
                None,
                None,
            )
        }
        .expect("create real occluding window");
        let overlay = OverlayWindow::new(0x22C55E, OverlayPlacement::for_window(owner.0 as usize))
            .expect("create owned overlay");
        overlay.position(96, 116, 648, 4).expect("position overlay");
        overlay.show();

        let mut rect = RECT::default();
        unsafe { GetWindowRect(overlay.hwnd(), &mut rect) }.expect("read overlay bounds");
        assert_eq!(
            (rect.left, rect.top, rect.right, rect.bottom),
            (96, 116, 744, 120)
        );

        unsafe {
            SetWindowPos(
                owner,
                Some(HWND_TOP),
                100,
                120,
                640,
                480,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .expect("raise owner group");
            SetWindowPos(
                cover,
                Some(HWND_TOP),
                80,
                100,
                680,
                520,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .expect("raise occluding window");
        }
        assert!(window_is_above(cover, overlay.hwnd()));

        unsafe {
            let _ = ShowWindow(owner, SW_MINIMIZE);
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(!unsafe { IsWindowVisible(overlay.hwnd()).as_bool() });

        drop(overlay);
        unsafe {
            let _ = DestroyWindow(cover);
            let _ = DestroyWindow(owner);
        }
    }

    fn window_is_above(expected_above: HWND, lower: HWND) -> bool {
        let mut current = lower;
        for _ in 0..512 {
            let Ok(next) = (unsafe { GetWindow(current, GW_HWNDPREV) }) else {
                return false;
            };
            current = next;
            if current == expected_above {
                return true;
            }
        }
        false
    }

    #[test]
    fn window_visible_bounds_prefer_dwm_frame_bounds() {
        let get_window_rect = OverlayRect {
            left: 96,
            top: 96,
            right: 504,
            bottom: 404,
        };
        let dwm_frame_bounds = OverlayRect {
            left: 100,
            top: 100,
            right: 500,
            bottom: 400,
        };

        assert_eq!(
            preferred_window_bounds(get_window_rect, Some(dwm_frame_bounds)),
            dwm_frame_bounds
        );
    }

    #[test]
    fn win_event_registry_dispatches_only_to_hook_owner_for_same_window() {
        let hwnd = 0x1234usize;
        let first_hook = 0xaaa1usize;
        let second_hook = 0xaaa2usize;
        let first_id = next_window_event_registration_id();
        let second_id = next_window_event_registration_id();
        let (first_sender, first_receiver) = mpsc::channel();
        let (second_sender, second_receiver) = mpsc::channel();

        register_window_event_target(
            first_id,
            hwnd,
            "window:1234".to_string(),
            OverlayStyle::default(),
            first_sender,
        )
        .unwrap();
        register_window_event_hook_handles(first_id, hwnd, &[first_hook]).unwrap();
        register_window_event_target(
            second_id,
            hwnd,
            "window:1234".to_string(),
            OverlayStyle::default(),
            second_sender,
        )
        .unwrap();
        register_window_event_hook_handles(second_id, hwnd, &[second_hook]).unwrap();

        dispatch_window_event(first_hook, EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            first_receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));
        assert!(matches!(
            second_receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected)
        ));

        dispatch_window_event(
            second_hook,
            EVENT_SYSTEM_MOVESIZESTART,
            hwnd,
            OBJID_WINDOW.0,
        );

        assert!(matches!(
            second_receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));

        unregister_window_event_target(first_id);
        unregister_window_event_target(second_id);
    }

    #[test]
    fn win_event_registry_unregisters_one_session_without_removing_others() {
        let hwnd = 0x5678usize;
        let first_hook = 0xbbb1usize;
        let second_hook = 0xbbb2usize;
        let first_id = next_window_event_registration_id();
        let second_id = next_window_event_registration_id();
        let (first_sender, first_receiver) = mpsc::channel();
        let (second_sender, second_receiver) = mpsc::channel();

        register_window_event_target(
            first_id,
            hwnd,
            "window:5678".to_string(),
            OverlayStyle::default(),
            first_sender,
        )
        .unwrap();
        register_window_event_hook_handles(first_id, hwnd, &[first_hook]).unwrap();
        register_window_event_target(
            second_id,
            hwnd,
            "window:5678".to_string(),
            OverlayStyle::default(),
            second_sender,
        )
        .unwrap();
        register_window_event_hook_handles(second_id, hwnd, &[second_hook]).unwrap();
        unregister_window_event_target(first_id);

        dispatch_window_event(first_hook, EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            first_receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected)
        ));

        dispatch_window_event(
            second_hook,
            EVENT_SYSTEM_MOVESIZESTART,
            hwnd,
            OBJID_WINDOW.0,
        );

        assert!(matches!(
            second_receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));

        unregister_window_event_target(second_id);
    }

    #[test]
    fn win_event_registry_ignores_location_change_while_moving_or_sizing() {
        const EVENT_OBJECT_LOCATIONCHANGE_FOR_TEST: u32 = 0x800B;

        let hwnd = 0x7777usize;
        let hook = 0xddd1usize;
        let registration_id = next_window_event_registration_id();
        let (sender, receiver) = mpsc::channel();

        register_window_event_target(
            registration_id,
            hwnd,
            "window:7777".to_string(),
            OverlayStyle::default(),
            sender,
        )
        .unwrap();
        register_window_event_hook_handles(registration_id, hwnd, &[hook]).unwrap();

        dispatch_window_event(hook, EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::SuspendWindowLocationChangeHook {
                registration_id: id,
                clear_deferred_refresh: true,
            }) if id == registration_id
        ));

        dispatch_window_event(
            hook,
            EVENT_OBJECT_LOCATIONCHANGE_FOR_TEST,
            hwnd,
            OBJID_WINDOW.0,
        );

        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected)
        ));

        unregister_window_event_target(registration_id);
    }

    #[test]
    fn win_event_registry_suspends_location_change_hook_while_moving_or_sizing() {
        let hwnd = 0x8899usize;
        let hook = 0xeeffusize;
        let registration_id = next_window_event_registration_id();
        let (sender, receiver) = mpsc::channel();

        register_window_event_target(
            registration_id,
            hwnd,
            "window:8899".to_string(),
            OverlayStyle::default(),
            sender,
        )
        .unwrap();
        register_window_event_hook_handles(registration_id, hwnd, &[hook]).unwrap();

        dispatch_window_event(hook, EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::SuspendWindowLocationChangeHook {
                registration_id: id,
                clear_deferred_refresh: true,
            }) if id == registration_id
        ));

        unregister_window_event_target(registration_id);
    }

    #[test]
    fn win_event_registry_defers_movesize_end_refresh_until_after_drag_settles() {
        let hwnd = 0x8990usize;
        let hook = 0xef01usize;
        let registration_id = next_window_event_registration_id();
        let (sender, receiver) = mpsc::channel();

        register_window_event_target(
            registration_id,
            hwnd,
            "window:8990".to_string(),
            OverlayStyle::default(),
            sender,
        )
        .unwrap();
        register_window_event_hook_handles(registration_id, hwnd, &[hook]).unwrap();

        dispatch_window_event(hook, EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::SuspendWindowLocationChangeHook {
                registration_id: id,
                clear_deferred_refresh: true,
            }) if id == registration_id
        ));

        dispatch_window_event(hook, EVENT_SYSTEM_MOVESIZEEND, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::DeferWindowEventRefresh {
                registration_id: id,
                target_hwnd: target,
                ..
            }) if id == registration_id && target == hwnd
        ));

        unregister_window_event_target(registration_id);
    }

    #[test]
    fn window_event_hook_ranges_include_location_change_for_fullscreen_resizes() {
        const EVENT_OBJECT_LOCATIONCHANGE_FOR_TEST: u32 = 0x800B;

        assert!(window_event_hook_ranges()
            .iter()
            .any(|(event_min, event_max)| {
                *event_min <= EVENT_OBJECT_LOCATIONCHANGE_FOR_TEST
                    && EVENT_OBJECT_LOCATIONCHANGE_FOR_TEST <= *event_max
            }));
    }

    #[test]
    fn win_event_registry_defers_idle_location_change_refresh() {
        const EVENT_OBJECT_LOCATIONCHANGE_FOR_TEST: u32 = 0x800B;

        let hwnd = 0x8888usize;
        let hook = 0xeee1usize;
        let registration_id = next_window_event_registration_id();
        let (sender, receiver) = mpsc::channel();

        register_window_event_target(
            registration_id,
            hwnd,
            "window:8888".to_string(),
            OverlayStyle::default(),
            sender,
        )
        .unwrap();
        register_window_event_hook_handles(registration_id, hwnd, &[hook]).unwrap();

        dispatch_window_event(
            hook,
            EVENT_OBJECT_LOCATIONCHANGE_FOR_TEST,
            hwnd,
            OBJID_WINDOW.0,
        );

        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::DeferWindowEventRefresh {
                registration_id: id,
                target_hwnd: target,
                ..
            }) if id == registration_id && target == hwnd
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::SuspendWindowLocationChangeHook {
                registration_id: id,
                clear_deferred_refresh: false,
            }) if id == registration_id
        ));

        unregister_window_event_target(registration_id);
    }

    #[test]
    fn overlay_thread_coalesces_and_clears_deferred_window_event_refreshes() {
        let mut overlay_thread = OverlayThread::default();
        let registration_id = next_window_event_registration_id();
        let style = OverlayStyle::default();

        overlay_thread.defer_window_event_refresh(
            registration_id,
            "window:1111".to_string(),
            style,
            0x1111,
        );
        overlay_thread.defer_window_event_refresh(
            registration_id,
            "window:2222".to_string(),
            style,
            0x2222,
        );

        assert_eq!(overlay_thread.pending_window_event_refreshes.len(), 1);
        let pending = overlay_thread
            .pending_window_event_refreshes
            .get(&registration_id)
            .expect("deferred refresh exists");
        assert_eq!(pending.source_id, "window:2222");
        assert_eq!(pending.target_hwnd, 0x2222);

        overlay_thread.clear_window_event_target(registration_id);

        assert!(overlay_thread.pending_window_event_refreshes.is_empty());
    }

    #[test]
    fn overlay_thread_clears_deferred_refresh_when_location_monitoring_is_suspended() {
        let mut overlay_thread = OverlayThread::default();
        let registration_id = next_window_event_registration_id();

        overlay_thread.defer_window_event_refresh(
            registration_id,
            "window:3333".to_string(),
            OverlayStyle::default(),
            0x3333,
        );

        overlay_thread.suspend_window_location_change_hook(registration_id, true);

        assert!(overlay_thread.pending_window_event_refreshes.is_empty());
    }

    #[test]
    fn overlay_thread_keeps_deferred_refresh_when_location_change_suspends_monitoring() {
        let mut overlay_thread = OverlayThread::default();
        let registration_id = next_window_event_registration_id();

        overlay_thread.defer_window_event_refresh(
            registration_id,
            "window:4444".to_string(),
            OverlayStyle::default(),
            0x4444,
        );

        overlay_thread.suspend_window_location_change_hook(registration_id, false);

        assert_eq!(overlay_thread.pending_window_event_refreshes.len(), 1);
    }

    #[test]
    fn overlay_thread_processes_queued_suspend_before_due_refresh() {
        let mut overlay_thread = OverlayThread::default();
        let registration_id = next_window_event_registration_id();
        let (sender, receiver) = mpsc::channel();

        overlay_thread.pending_window_event_refreshes.insert(
            registration_id,
            PendingWindowEventRefresh {
                source_id: "window:5555".to_string(),
                style: OverlayStyle::default(),
                target_hwnd: 0x5555,
                due_at: Instant::now(),
            },
        );
        sender
            .send(OverlayCommand::SuspendWindowLocationChangeHook {
                registration_id,
                clear_deferred_refresh: true,
            })
            .unwrap();

        assert!(!overlay_thread.handle_queued_commands(&receiver));
        overlay_thread.refresh_due_window_event_targets();

        assert!(overlay_thread.pending_window_event_refreshes.is_empty());
    }

    #[test]
    fn deferred_refresh_waits_while_pointer_button_is_down() {
        let now = Instant::now();

        assert!(!deferred_refresh_is_ready(now, now, true));
        assert!(deferred_refresh_is_ready(now, now, false));
        assert!(!deferred_refresh_is_ready(
            now,
            now + Duration::from_millis(1),
            false
        ));
    }

    #[test]
    fn win_event_registry_destroy_unregisters_target_as_terminal() {
        let hwnd = 0x9abcusize;
        let hook = 0xccc1usize;
        let registration_id = next_window_event_registration_id();
        let (sender, receiver) = mpsc::channel();

        register_window_event_target(
            registration_id,
            hwnd,
            "window:9abc".to_string(),
            OverlayStyle::default(),
            sender,
        )
        .unwrap();
        register_window_event_hook_handles(registration_id, hwnd, &[hook]).unwrap();

        dispatch_window_event(hook, EVENT_OBJECT_DESTROY, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::ClearWindowEventTargetForWindowEvent { registration_id: id })
                if id == registration_id
        ));

        dispatch_window_event(hook, EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected)
        ));

        unregister_window_event_target(registration_id);
    }
}
