use crate::models::PermissionStatus;
use windows::Graphics::Capture::GraphicsCaptureSession;

pub fn check_permission() -> PermissionStatus {
    match GraphicsCaptureSession::IsSupported() {
        Ok(true) => PermissionStatus::Granted,
        Ok(false) => PermissionStatus::Unsupported,
        Err(_) => PermissionStatus::Unsupported,
    }
}

pub fn request_permission() -> crate::Result<PermissionStatus> {
    Ok(check_permission())
}
