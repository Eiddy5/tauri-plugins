use crate::{
    error::Error,
    models::{CaptureErrorCode, PermissionStatus},
};
use windows::{
    Graphics::Capture::{GraphicsCaptureAccess, GraphicsCaptureAccessKind, GraphicsCaptureSession},
    Security::Authorization::AppCapabilityAccess::AppCapabilityAccessStatus,
};

pub fn check_permission() -> PermissionStatus {
    match GraphicsCaptureSession::IsSupported() {
        Ok(true) => PermissionStatus::Granted,
        Ok(false) => PermissionStatus::Unsupported,
        Err(_) => PermissionStatus::Unsupported,
    }
}

pub async fn request_permission() -> crate::Result<PermissionStatus> {
    if check_permission() != PermissionStatus::Granted {
        return Ok(PermissionStatus::Unsupported);
    }

    let operation =
        GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
            .map_err(borderless_permission_error)?;
    let status = operation.await.map_err(borderless_permission_error)?;
    Ok(borderless_permission_status(status))
}

pub async fn require_borderless_capture() -> crate::Result<()> {
    match request_permission().await? {
        PermissionStatus::Granted => Ok(()),
        PermissionStatus::Denied => Err(Error::new(
            CaptureErrorCode::PermissionDenied,
            "Windows borderless screen capture permission was denied",
            true,
        )),
        PermissionStatus::NotDetermined => Err(Error::new(
            CaptureErrorCode::PermissionNotDetermined,
            "Windows borderless screen capture permission has not been decided",
            true,
        )),
        PermissionStatus::Restricted => Err(Error::new(
            CaptureErrorCode::PermissionDenied,
            "Windows borderless screen capture is restricted; packaged apps must declare <uap11:Capability Name=\"graphicsCaptureWithoutBorder\" />",
            false,
        )),
        PermissionStatus::Unsupported => Err(Error::new(
            CaptureErrorCode::UnsupportedPlatform,
            "This Windows version does not support borderless screen capture",
            false,
        )),
    }
}

fn borderless_permission_status(status: AppCapabilityAccessStatus) -> PermissionStatus {
    match status {
        AppCapabilityAccessStatus::Allowed => PermissionStatus::Granted,
        AppCapabilityAccessStatus::DeniedByUser => PermissionStatus::Denied,
        AppCapabilityAccessStatus::UserPromptRequired => PermissionStatus::NotDetermined,
        AppCapabilityAccessStatus::DeniedBySystem | AppCapabilityAccessStatus::NotDeclaredByApp => {
            PermissionStatus::Restricted
        }
        _ => PermissionStatus::Restricted,
    }
}

fn borderless_permission_error(error: windows::core::Error) -> Error {
    Error::new(
        CaptureErrorCode::UnsupportedPlatform,
        format!("Windows borderless screen capture permission is unavailable: {error}"),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_borderless_capture_access_statuses() {
        assert_eq!(
            borderless_permission_status(AppCapabilityAccessStatus::Allowed),
            PermissionStatus::Granted
        );
        assert_eq!(
            borderless_permission_status(AppCapabilityAccessStatus::DeniedByUser),
            PermissionStatus::Denied
        );
        assert_eq!(
            borderless_permission_status(AppCapabilityAccessStatus::UserPromptRequired),
            PermissionStatus::NotDetermined
        );
        assert_eq!(
            borderless_permission_status(AppCapabilityAccessStatus::NotDeclaredByApp),
            PermissionStatus::Restricted
        );
    }
}
