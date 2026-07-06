use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Mutex, OnceLock,
    },
};

use windows::Win32::{
    Foundation::{GetLastError, HWND},
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        WindowsAndMessaging::{
            GetWindowThreadProcessId, IsWindow, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE,
            EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW, EVENT_SYSTEM_MOVESIZEEND,
            EVENT_SYSTEM_MOVESIZESTART, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
        },
    },
};

use crate::{error::Error, models::CaptureErrorCode, overlay::OverlayStyle, Result};

use super::{
    bounds::window_bounds_from_source_id, host::OverlayCommand, placement::OverlayPlacement,
};

static NEXT_WINDOW_EVENT_REGISTRATION_ID: AtomicU64 = AtomicU64::new(1);
static WINDOW_EVENT_REGISTRY: OnceLock<Mutex<WindowEventRegistry>> = OnceLock::new();

#[derive(Debug)]
pub(crate) struct WindowEventHooks {
    pub(crate) registration_id: u64,
    pub(crate) target_hwnd: usize,
    pub(crate) process_id: u32,
    pub(crate) thread_id: u32,
    pub(crate) handles: Vec<usize>,
    pub(crate) location_change_handle: Option<usize>,
}

impl WindowEventHooks {
    pub(crate) fn all_handles(&self) -> Vec<usize> {
        let mut handles = self.handles.clone();
        if let Some(handle) = self.location_change_handle {
            handles.push(handle);
        }
        handles
    }

    pub(crate) fn unregister(self) {
        unregister_window_event_target(self.registration_id);

        for handle in self.handles {
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(handle as *mut c_void));
            }
        }
        if let Some(handle) = self.location_change_handle {
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(handle as *mut c_void));
            }
        }
    }

    pub(crate) fn suspend_location_change(&mut self) {
        if let Some(handle) = self.location_change_handle.take() {
            unregister_window_event_hook_handle(handle);
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(handle as *mut c_void));
            }
        }
    }

    pub(crate) fn resume_location_change(&mut self) -> Result<()> {
        if self.location_change_handle.is_some() {
            return Ok(());
        }

        let handle = install_window_event_hook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            self.process_id,
            self.thread_id,
        )?;
        if let Err(error) =
            register_window_event_hook_handles(self.registration_id, self.target_hwnd, &[handle])
        {
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(handle as *mut c_void));
            }
            return Err(error);
        }

        self.location_change_handle = Some(handle);
        Ok(())
    }
}

#[derive(Default)]
struct WindowEventRegistry {
    targets_by_hwnd: HashMap<usize, Vec<WindowEventTarget>>,
    hook_targets: HashMap<usize, WindowEventHookTarget>,
}

#[derive(Clone)]
struct WindowEventHookTarget {
    registration_id: u64,
    target_hwnd: usize,
}

#[derive(Clone)]
struct WindowEventTarget {
    registration_id: u64,
    source_id: String,
    style: OverlayStyle,
    sender: mpsc::Sender<OverlayCommand>,
    moving_or_sizing: bool,
}

enum WindowEventAction {
    Hide {
        sender: mpsc::Sender<OverlayCommand>,
    },
    HideAndSuspendLocationChange {
        sender: mpsc::Sender<OverlayCommand>,
        registration_id: u64,
    },
    HideAndClear {
        sender: mpsc::Sender<OverlayCommand>,
        registration_id: u64,
    },
    RefreshBounds {
        registration_id: Option<u64>,
        source_id: String,
        style: OverlayStyle,
        target_hwnd: usize,
        sender: mpsc::Sender<OverlayCommand>,
    },
    DeferRefresh {
        registration_id: u64,
        source_id: String,
        style: OverlayStyle,
        target_hwnd: usize,
        sender: mpsc::Sender<OverlayCommand>,
    },
}

pub(crate) fn next_window_event_registration_id() -> u64 {
    NEXT_WINDOW_EVENT_REGISTRATION_ID.fetch_add(1, Ordering::Relaxed)
}

fn window_event_registry() -> &'static Mutex<WindowEventRegistry> {
    WINDOW_EVENT_REGISTRY.get_or_init(|| Mutex::new(WindowEventRegistry::default()))
}

pub(crate) fn register_window_event_target(
    registration_id: u64,
    target_hwnd: usize,
    source_id: String,
    style: OverlayStyle,
    sender: mpsc::Sender<OverlayCommand>,
) -> Result<()> {
    let mut registry = lock_window_event_registry()?;
    let targets = registry.targets_by_hwnd.entry(target_hwnd).or_default();
    targets.retain(|target| target.registration_id != registration_id);
    targets.push(WindowEventTarget {
        registration_id,
        source_id,
        style,
        sender,
        moving_or_sizing: false,
    });

    Ok(())
}

pub(crate) fn register_window_event_hook_handles(
    registration_id: u64,
    target_hwnd: usize,
    handles: &[usize],
) -> Result<()> {
    let mut registry = lock_window_event_registry()?;
    registry
        .hook_targets
        .retain(|_, target| target.registration_id != registration_id);

    for handle in handles {
        registry.hook_targets.insert(
            *handle,
            WindowEventHookTarget {
                registration_id,
                target_hwnd,
            },
        );
    }

    Ok(())
}

pub(crate) fn unregister_window_event_target(registration_id: u64) {
    let Ok(mut registry) = window_event_registry().lock() else {
        return;
    };

    let mut empty_hwnds = Vec::new();
    for (target_hwnd, targets) in &mut registry.targets_by_hwnd {
        targets.retain(|target| target.registration_id != registration_id);
        if targets.is_empty() {
            empty_hwnds.push(*target_hwnd);
        }
    }

    for target_hwnd in empty_hwnds {
        registry.targets_by_hwnd.remove(&target_hwnd);
    }

    registry
        .hook_targets
        .retain(|_, target| target.registration_id != registration_id);
}

fn unregister_window_event_hook_handle(handle: usize) {
    let Ok(mut registry) = window_event_registry().lock() else {
        return;
    };

    registry.hook_targets.remove(&handle);
}

fn lock_window_event_registry() -> Result<std::sync::MutexGuard<'static, WindowEventRegistry>> {
    window_event_registry().lock().map_err(|_| {
        Error::new(
            CaptureErrorCode::Internal,
            "share border overlay WinEvent registry lock was poisoned",
            false,
        )
    })
}

pub(crate) struct InstalledWindowEventHooks {
    pub(crate) handles: Vec<usize>,
    pub(crate) location_change_handle: Option<usize>,
}

impl InstalledWindowEventHooks {
    fn unregister(self) {
        for handle in self.handles {
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(handle as *mut c_void));
            }
        }
        if let Some(handle) = self.location_change_handle {
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(handle as *mut c_void));
            }
        }
    }
}

pub(crate) fn install_window_event_hooks(
    process_id: u32,
    thread_id: u32,
) -> Result<InstalledWindowEventHooks> {
    let mut installed = InstalledWindowEventHooks {
        handles: Vec::with_capacity(window_event_hook_ranges().len()),
        location_change_handle: None,
    };

    for &(event_min, event_max) in window_event_hook_ranges() {
        match install_window_event_hook(event_min, event_max, process_id, thread_id) {
            Ok(handle) if event_min == EVENT_OBJECT_LOCATIONCHANGE => {
                installed.location_change_handle = Some(handle);
            }
            Ok(handle) => installed.handles.push(handle),
            Err(error) => {
                installed.unregister();
                return Err(error);
            }
        }
    }

    Ok(installed)
}

pub(crate) fn window_event_hook_ranges() -> &'static [(u32, u32)] {
    &[
        (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
        (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY),
        (EVENT_OBJECT_HIDE, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW),
    ]
}

fn install_window_event_hook(
    event_min: u32,
    event_max: u32,
    process_id: u32,
    thread_id: u32,
) -> Result<usize> {
    let hook = unsafe {
        SetWinEventHook(
            event_min,
            event_max,
            None,
            Some(win_event_proc),
            process_id,
            thread_id,
            WINEVENT_OUTOFCONTEXT,
        )
    };

    if hook.is_invalid() {
        return Err(Error::new(
            CaptureErrorCode::Internal,
            format!(
                "failed to install Windows share border WinEvent hook for event range {event_min}..={event_max}: {:?}",
                unsafe { GetLastError() }
            ),
            true,
        ));
    }

    Ok(hook.0 as usize)
}

pub(crate) fn target_window_thread(hwnd: HWND) -> Option<(u32, u32)> {
    if !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return None;
    }

    let mut process_id = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    if thread_id == 0 {
        return None;
    }

    Some((process_id, thread_id))
}

unsafe extern "system" fn win_event_proc(
    hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object_id: i32,
    _child_id: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    dispatch_window_event(hook.0 as usize, event, hwnd.0 as usize, object_id);
}

pub(crate) fn dispatch_window_event(hook: usize, event: u32, hwnd: usize, object_id: i32) {
    if object_id != OBJID_WINDOW.0 {
        return;
    }

    let action = {
        let Ok(mut registry) = window_event_registry().lock() else {
            return;
        };
        let Some(hook_target) = registry.hook_targets.get(&hook).cloned() else {
            return;
        };
        if hook_target.target_hwnd != hwnd {
            return;
        }

        let Some(targets) = registry.targets_by_hwnd.get_mut(&hwnd) else {
            return;
        };
        let Some(target_index) = targets
            .iter()
            .position(|target| target.registration_id == hook_target.registration_id)
        else {
            return;
        };

        match event {
            EVENT_SYSTEM_MOVESIZESTART => {
                let target = &mut targets[target_index];
                target.moving_or_sizing = true;
                Some(WindowEventAction::HideAndSuspendLocationChange {
                    sender: target.sender.clone(),
                    registration_id: target.registration_id,
                })
            }
            EVENT_SYSTEM_MOVESIZEEND => {
                let target = &mut targets[target_index];
                target.moving_or_sizing = false;
                Some(WindowEventAction::DeferRefresh {
                    registration_id: target.registration_id,
                    source_id: target.source_id.clone(),
                    style: target.style,
                    target_hwnd: hwnd,
                    sender: target.sender.clone(),
                })
            }
            EVENT_OBJECT_LOCATIONCHANGE => {
                let target = &targets[target_index];
                if target.moving_or_sizing {
                    None
                } else {
                    Some(WindowEventAction::DeferRefresh {
                        registration_id: target.registration_id,
                        source_id: target.source_id.clone(),
                        style: target.style,
                        target_hwnd: hwnd,
                        sender: target.sender.clone(),
                    })
                }
            }
            EVENT_OBJECT_HIDE => {
                let target = &mut targets[target_index];
                target.moving_or_sizing = false;
                Some(WindowEventAction::Hide {
                    sender: target.sender.clone(),
                })
            }
            EVENT_OBJECT_DESTROY => {
                let target = targets.remove(target_index);
                if targets.is_empty() {
                    registry.targets_by_hwnd.remove(&hwnd);
                }
                registry
                    .hook_targets
                    .retain(|_, existing| existing.registration_id != hook_target.registration_id);
                Some(WindowEventAction::HideAndClear {
                    sender: target.sender,
                    registration_id: target.registration_id,
                })
            }
            EVENT_OBJECT_SHOW => {
                let target = &targets[target_index];
                if target.moving_or_sizing {
                    None
                } else {
                    Some(WindowEventAction::RefreshBounds {
                        registration_id: None,
                        source_id: target.source_id.clone(),
                        style: target.style,
                        target_hwnd: hwnd,
                        sender: target.sender.clone(),
                    })
                }
            }
            _ => None,
        }
    };

    if let Some(action) = action {
        match action {
            WindowEventAction::Hide { sender } => {
                let _ = sender.send(OverlayCommand::HideForWindowEvent);
            }
            WindowEventAction::HideAndSuspendLocationChange {
                sender,
                registration_id,
            } => {
                let _ = sender.send(OverlayCommand::HideForWindowEvent);
                let _ = sender.send(OverlayCommand::SuspendWindowLocationChangeHook {
                    registration_id,
                    clear_deferred_refresh: true,
                });
            }
            WindowEventAction::HideAndClear {
                sender,
                registration_id,
            } => {
                let _ = sender.send(OverlayCommand::HideForWindowEvent);
                let _ = sender
                    .send(OverlayCommand::ClearWindowEventTargetForWindowEvent { registration_id });
            }
            WindowEventAction::RefreshBounds {
                registration_id,
                source_id,
                style,
                target_hwnd,
                sender,
            } => {
                let command = match window_bounds_from_source_id(&source_id) {
                    Ok(Some(rect)) => OverlayCommand::ShowRectForWindowEvent {
                        rect,
                        style,
                        placement: OverlayPlacement::for_window(target_hwnd),
                    },
                    Ok(None) | Err(_) => OverlayCommand::HideForWindowEvent,
                };
                let _ = sender.send(command);
                if let Some(registration_id) = registration_id {
                    let _ = sender
                        .send(OverlayCommand::ResumeWindowLocationChangeHook { registration_id });
                }
            }
            WindowEventAction::DeferRefresh {
                registration_id,
                source_id,
                style,
                target_hwnd,
                sender,
            } => {
                let _ = sender.send(OverlayCommand::DeferWindowEventRefresh {
                    registration_id,
                    source_id,
                    style,
                    target_hwnd,
                });
                let _ = sender.send(OverlayCommand::SuspendWindowLocationChangeHook {
                    registration_id,
                    clear_deferred_refresh: false,
                });
            }
        }
    }
}
