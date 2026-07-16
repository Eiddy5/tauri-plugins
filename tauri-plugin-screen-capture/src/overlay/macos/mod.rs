mod dispatcher;
mod events;
mod host;
mod model;
mod panel;
mod window_info;

pub const OVERLAY_WINDOW_TITLE_PREFIX: &str = "TAURI_SCREEN_CAPTURE_OVERLAY:";

use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
};

use async_trait::async_trait;
use tauri::{AppHandle, Runtime};

use crate::{models::AnnotationInputTarget, overlay::OverlayTarget, Result};

use self::dispatcher::{request, MainThreadDispatcher, TauriMainThreadDispatcher};

use super::{ShareOverlay, ShareOverlayFactory};

pub use events::{event_action, OverlayEvent, RefreshAction, WINDOW_POSITION_POLL_INTERVAL};
pub use model::{
    decide_window_overlay, lightweight_order_span, needs_native_update, verify_lightweight_order,
    verify_overlay_panel_placement, visible_corner_layers, OrderVerificationState, OrderedWindow,
    OverlayDecision, WindowFrameAction, WindowFrameTracker, WindowSnapshot,
};
pub use panel::{MacRect, OverlayPanelLayout};

static OVERLAY_WINDOW_IDS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

pub(crate) fn register_overlay_window(window_id: u32) {
    overlay_window_ids().insert(window_id);
}

pub(crate) fn unregister_overlay_window(window_id: u32) {
    overlay_window_ids().remove(&window_id);
}

pub(crate) fn is_registered_overlay_window(window_id: u32) -> bool {
    overlay_window_ids().contains(&window_id)
}

fn overlay_window_ids() -> MutexGuard<'static, HashSet<u32>> {
    OVERLAY_WINDOW_IDS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

impl Drop for MacOsShareOverlay {
    fn drop(&mut self) {
        let id = self.id;
        let _ = self.dispatcher.dispatch(Box::new(move || {
            let _ = host::stop(id);
        }));
    }
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

    async fn annotation_input_target(&self) -> Result<Option<AnnotationInputTarget>> {
        let id = self.id;
        request(self.dispatcher.as_ref(), move || {
            host::annotation_input_target(id)
        })
        .await
    }
}
