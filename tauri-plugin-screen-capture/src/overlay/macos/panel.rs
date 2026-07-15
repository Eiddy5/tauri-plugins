use objc2::{rc::Retained, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSView, NSWindowCollectionBehavior, NSWindowOrderingMode,
    NSWindowStyleMask,
};
use objc2_foundation::{NSInteger, NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CATransaction};

use crate::{models::CaptureErrorCode, Error, Result};

use super::{register_overlay_window, unregister_overlay_window, OVERLAY_WINDOW_TITLE_PREFIX};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MacRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl MacRect {
    pub fn is_valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }
}

pub fn corner_panel_frames(target: MacRect, corner_length: f64) -> [MacRect; 4] {
    let length = corner_length
        .max(1.0)
        .min(target.width.max(1.0) / 2.0)
        .min(target.height.max(1.0) / 2.0);
    let right = target.x + target.width - length;
    let top = target.y + target.height - length;

    [
        MacRect {
            x: target.x,
            y: top,
            width: length,
            height: length,
        },
        MacRect {
            x: right,
            y: top,
            width: length,
            height: length,
        },
        MacRect {
            x: target.x,
            y: target.y,
            width: length,
            height: length,
        },
        MacRect {
            x: right,
            y: target.y,
            width: length,
            height: length,
        },
    ]
}

fn corner_panel_collection_behavior() -> NSWindowCollectionBehavior {
    NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::Transient
        | NSWindowCollectionBehavior::IgnoresCycle
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    const ALL: [Self; 4] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    fn title_suffix(self) -> &'static str {
        match self {
            Self::TopLeft => "tl",
            Self::TopRight => "tr",
            Self::BottomLeft => "bl",
            Self::BottomRight => "br",
        }
    }
}

pub(crate) struct CornerPanel {
    panel: Retained<NSPanel>,
    horizontal: Retained<CALayer>,
    vertical: Retained<CALayer>,
    corner: Corner,
    window_id: u32,
}

impl CornerPanel {
    pub(crate) fn create_set(session_id: u64) -> Result<[Self; 4]> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            Error::new(
                CaptureErrorCode::Internal,
                "NSPanel 只能在 AppKit 主线程创建",
                true,
            )
        })?;
        Ok([
            Self::new(mtm, session_id, Corner::ALL[0]),
            Self::new(mtm, session_id, Corner::ALL[1]),
            Self::new(mtm, session_id, Corner::ALL[2]),
            Self::new(mtm, session_id, Corner::ALL[3]),
        ])
    }

    fn new(mtm: MainThreadMarker, session_id: u64, corner: Corner) -> Self {
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(32.0, 32.0));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            rect,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect);
        let root = CALayer::layer();
        let horizontal = CALayer::layer();
        let vertical = CALayer::layer();
        let green = NSColor::colorWithSRGBRed_green_blue_alpha(
            34.0 / 255.0,
            197.0 / 255.0,
            94.0 / 255.0,
            1.0,
        )
        .CGColor();

        horizontal.setBackgroundColor(Some(&green));
        vertical.setBackgroundColor(Some(&green));
        root.addSublayer(&horizontal);
        root.addSublayer(&vertical);
        view.setWantsLayer(true);
        view.setLayer(Some(&root));

        panel.setContentView(Some(&view));
        panel.setOpaque(false);
        panel.setHasShadow(false);
        panel.setIgnoresMouseEvents(true);
        panel.setMovable(false);
        panel.setMovableByWindowBackground(false);
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setExcludedFromWindowsMenu(true);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setCollectionBehavior(corner_panel_collection_behavior());
        panel.setTitle(&NSString::from_str(&format!(
            "{OVERLAY_WINDOW_TITLE_PREFIX}{session_id}:{}",
            corner.title_suffix()
        )));
        // SAFETY: CornerPanel retains the NSPanel and explicitly closes it on drop.
        unsafe { panel.setReleasedWhenClosed(false) };

        let window_id = panel.windowNumber() as u32;
        register_overlay_window(window_id);
        let panel = Self {
            panel,
            horizontal,
            vertical,
            corner,
            window_id,
        };
        panel.update_frame(MacRect {
            x: 0.0,
            y: 0.0,
            width: 32.0,
            height: 32.0,
        });
        panel
    }

    pub(crate) fn update_frame(&self, frame: MacRect) {
        self.panel.setFrame_display(ns_rect(frame), false);
        let contents_scale = self.panel.backingScaleFactor();
        self.horizontal.setContentsScale(contents_scale);
        self.vertical.setContentsScale(contents_scale);
        let width = frame.width.max(1.0);
        let height = frame.height.max(1.0);
        let thickness = 4.0_f64.min(width).min(height);
        let (horizontal_y, vertical_x) = match self.corner {
            Corner::TopLeft => (height - thickness, 0.0),
            Corner::TopRight => (height - thickness, width - thickness),
            Corner::BottomLeft => (0.0, 0.0),
            Corner::BottomRight => (0.0, width - thickness),
        };

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.horizontal.setFrame(objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(0.0, horizontal_y),
            objc2_core_foundation::CGSize::new(width, thickness),
        ));
        self.vertical.setFrame(objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(vertical_x, 0.0),
            objc2_core_foundation::CGSize::new(thickness, height),
        ));
        CATransaction::commit();
    }

    pub(crate) fn set_level(&self, level: i32) {
        self.panel.setLevel(level as NSInteger);
    }

    pub(crate) fn order_above(&self, target_window_id: u32) {
        self.panel
            .orderWindow_relativeTo(NSWindowOrderingMode::Above, target_window_id as NSInteger);
    }

    pub(crate) fn order_front(&self) {
        self.panel.orderFrontRegardless();
    }

    pub(crate) fn hide(&self) {
        self.panel.orderOut(None);
    }

    pub(crate) fn window_id(&self) -> u32 {
        self.window_id
    }
}

impl Drop for CornerPanel {
    fn drop(&mut self) {
        unregister_overlay_window(self.window_id);
        self.panel.orderOut(None);
        self.panel.close();
    }
}

fn ns_rect(rect: MacRect) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.x, rect.y),
        NSSize::new(rect.width, rect.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_panels_are_transient_and_do_not_participate_in_window_management() {
        let behavior = corner_panel_collection_behavior();

        assert!(behavior.contains(NSWindowCollectionBehavior::CanJoinAllSpaces));
        assert!(behavior.contains(NSWindowCollectionBehavior::FullScreenAuxiliary));
        assert!(behavior.contains(NSWindowCollectionBehavior::Transient));
        assert!(behavior.contains(NSWindowCollectionBehavior::IgnoresCycle));
        assert!(!behavior.contains(NSWindowCollectionBehavior::Stationary));
        assert!(!behavior.contains(NSWindowCollectionBehavior::Managed));
    }
}
