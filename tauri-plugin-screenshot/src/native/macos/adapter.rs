use async_trait::async_trait;
use tauri::{AppHandle, Runtime};

use crate::{
    error::{Error, ErrorCode},
    models::CaptureOptions,
    native::{NativeCaptureAdapter, NativeCaptureOutcome},
    selection::{SelectionModel, Size},
    Result,
};

use super::{
    capture::{
        capture_desktop, ensure_screen_capture_permission, mac_window_catalog, virtual_desktop,
    },
    overlay::select_region,
};

pub(crate) struct MacOsNativeCaptureAdapter<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> MacOsNativeCaptureAdapter<R> {
    pub(crate) fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

#[async_trait]
impl<R: Runtime> NativeCaptureAdapter for MacOsNativeCaptureAdapter<R> {
    async fn capture_area(&self, options: CaptureOptions) -> Result<NativeCaptureOutcome> {
        if !objc2::available!(macos = 15.2) {
            return Err(Error::new(
                ErrorCode::Unsupported,
                "当前原生截图实现需要 macOS 15.2 或更高版本",
                false,
            ));
        }
        ensure_screen_capture_permission()?;

        let desktop = virtual_desktop()?;
        let desktop_bounds = desktop.bounds;
        let frame_task = tokio::task::spawn_blocking(move || capture_desktop(desktop_bounds));
        let windows_task = tokio::task::spawn_blocking(move || mac_window_catalog(desktop_bounds));
        let frame = frame_task.await.map_err(|error| {
            Error::new(
                ErrorCode::CaptureFailed,
                format!("macOS 截图任务异常结束：{error}"),
                true,
            )
        })??;
        let windows = windows_task.await.unwrap_or_default();

        // 浮层直接复用 CGImage，避免在用户开始框选前进行整屏 PNG 编解码。
        let preview_image = frame.preview_image();
        let logical_size = frame.logical_size();
        let model = SelectionModel::new_with_scale(
            logical_size,
            Size::new(
                options.effective_min_width(),
                options.effective_min_height(),
            ),
            1.0,
        );

        let selection = select_region(
            &self.app,
            preview_image,
            desktop_bounds,
            desktop.displays,
            logical_size,
            model,
            windows,
        )
        .await?;

        match selection {
            None => Ok(NativeCaptureOutcome::Cancelled),
            Some(selection) => {
                let image = tokio::task::spawn_blocking(move || frame.crop_png(selection))
                    .await
                    .map_err(|error| {
                        Error::new(
                            ErrorCode::CaptureFailed,
                            format!("macOS 截图裁剪任务异常结束：{error}"),
                            true,
                        )
                    })??;
                Ok(NativeCaptureOutcome::Captured(image))
            }
        }
    }
}
