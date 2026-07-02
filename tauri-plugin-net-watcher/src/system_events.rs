use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatcherSignal {
    Stop,
    SystemNetworkChanged,
}

pub(crate) struct SystemNetworkWatcher {
    #[cfg(target_os = "windows")]
    #[allow(dead_code)]
    inner: windows::WindowsSystemNetworkWatcher,
}

pub(crate) fn start_system_network_watcher(
    tx: mpsc::UnboundedSender<WatcherSignal>,
) -> crate::Result<Option<SystemNetworkWatcher>> {
    start_platform_watcher(tx)
}

#[cfg(target_os = "windows")]
fn start_platform_watcher(
    tx: mpsc::UnboundedSender<WatcherSignal>,
) -> crate::Result<Option<SystemNetworkWatcher>> {
    windows::WindowsSystemNetworkWatcher::start(tx)
        .map(|inner| Some(SystemNetworkWatcher { inner }))
}

#[cfg(target_os = "macos")]
fn start_platform_watcher(
    _tx: mpsc::UnboundedSender<WatcherSignal>,
) -> crate::Result<Option<SystemNetworkWatcher>> {
    Ok(None)
}

#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    use tokio::sync::mpsc;
    use windows_sys::Win32::{
        Foundation::{HANDLE, NO_ERROR},
        NetworkManagement::IpHelper::{
            CancelMibChangeNotify2, NotifyIpInterfaceChange, MIB_IPINTERFACE_ROW,
            MIB_NOTIFICATION_TYPE,
        },
        Networking::WinSock::AF_UNSPEC,
    };

    use super::WatcherSignal;

    struct CallbackContext {
        tx: mpsc::UnboundedSender<WatcherSignal>,
    }

    pub(super) struct WindowsSystemNetworkWatcher {
        handle: HANDLE,
        context: *mut CallbackContext,
    }

    unsafe impl Send for WindowsSystemNetworkWatcher {}

    impl WindowsSystemNetworkWatcher {
        pub(super) fn start(tx: mpsc::UnboundedSender<WatcherSignal>) -> crate::Result<Self> {
            let context = Box::into_raw(Box::new(CallbackContext { tx }));
            let mut handle = null_mut();

            let error = unsafe {
                NotifyIpInterfaceChange(
                    AF_UNSPEC,
                    Some(network_changed),
                    context.cast::<c_void>(),
                    false,
                    &mut handle,
                )
            };

            if error != NO_ERROR {
                unsafe {
                    drop(Box::from_raw(context));
                }
                return Err(crate::Error::internal(format!(
                    "failed to subscribe to Windows network changes: {error}"
                )));
            }

            Ok(Self { handle, context })
        }
    }

    impl Drop for WindowsSystemNetworkWatcher {
        fn drop(&mut self) {
            unsafe {
                if !self.handle.is_null() {
                    let _ = CancelMibChangeNotify2(self.handle);
                }

                if !self.context.is_null() {
                    drop(Box::from_raw(self.context));
                    self.context = null_mut();
                }
            }
        }
    }

    unsafe extern "system" fn network_changed(
        caller_context: *const c_void,
        _row: *const MIB_IPINTERFACE_ROW,
        _notification_type: MIB_NOTIFICATION_TYPE,
    ) {
        if caller_context.is_null() {
            return;
        }

        let context = unsafe { &*(caller_context as *const CallbackContext) };
        let _ = context.tx.send(WatcherSignal::SystemNetworkChanged);
    }
}
