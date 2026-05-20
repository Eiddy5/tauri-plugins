use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

pub fn init<R: Runtime, C: DeserializeOwned>(
  app: &AppHandle<R>,
  _api: PluginApi<R, C>,
) -> crate::Result<ScreenCapture<R>> {
  Ok(ScreenCapture { _app: app.clone() })
}

/// Access to the screen-capture APIs.
pub struct ScreenCapture<R: Runtime> {
  _app: AppHandle<R>,
}
