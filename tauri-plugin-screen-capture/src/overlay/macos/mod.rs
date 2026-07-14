mod dispatcher;
mod events;
mod host;
mod model;
mod panel;
mod window_info;

pub const OVERLAY_WINDOW_TITLE_PREFIX: &str = "TAURI_SCREEN_CAPTURE_OVERLAY:";

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use async_trait::async_trait;
use tauri::{AppHandle, Runtime};

use crate::{overlay::OverlayTarget, Result};

use self::dispatcher::{request, MainThreadDispatcher, TauriMainThreadDispatcher};

use super::{ShareOverlay, ShareOverlayFactory};

pub use events::{event_action, OverlayEvent, RefreshAction};
pub use model::{
    decide_window_overlay, needs_native_update, verify_relative_order, OrderedWindow,
    OverlayDecision, WindowSnapshot,
};
pub use panel::{corner_panel_frames, MacRect};

pub struct MacOsShareOverlayFactory {
    dispatcher: Arc<dyn MainThreadDispatcher>,
    next_id: AtomicU64,
}

impl MacOsShareOverlayFactory {
    pub fn new<R: Runtime>(app: AppHandle<R>) -> Self {
        Self {
            dispatcher: Arc::new(TauriMainThreadDispatcher::new(app)),
            next_id: AtomicU64::new(1),
        }
    }
}

impl ShareOverlayFactory for MacOsShareOverlayFactory {
    fn create_overlay(&self) -> Arc<dyn ShareOverlay> {
        Arc::new(MacOsShareOverlay {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            dispatcher: Arc::clone(&self.dispatcher),
        })
    }
}

struct MacOsShareOverlay {
    id: u64,
    dispatcher: Arc<dyn MainThreadDispatcher>,
}

#[async_trait]
impl ShareOverlay for MacOsShareOverlay {
    async fn start(&self, target: OverlayTarget) -> Result<()> {
        let id = self.id;
        request(self.dispatcher.as_ref(), move || host::start(id, target)).await
    }

    async fn show(&self) -> Result<()> {
        let id = self.id;
        request(self.dispatcher.as_ref(), move || host::show(id)).await
    }

    async fn hide(&self) -> Result<()> {
        let id = self.id;
        request(self.dispatcher.as_ref(), move || host::hide(id)).await
    }

    async fn stop(&self) -> Result<()> {
        let id = self.id;
        request(self.dispatcher.as_ref(), move || host::stop(id)).await
    }
}
