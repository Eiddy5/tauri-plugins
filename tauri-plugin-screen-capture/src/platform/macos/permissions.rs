use crate::models::PermissionStatus;

pub fn check_permission() -> PermissionStatus {
    let status = if unsafe { CGPreflightScreenCaptureAccess() } {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    };
    eprintln!("[screen-capture] macOS screen permission preflight: {status:?}");
    status
}

pub fn request_permission() -> PermissionStatus {
    let status = if unsafe { CGRequestScreenCaptureAccess() } {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    };
    eprintln!("[screen-capture] macOS screen permission request: {status:?}");
    status
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}
