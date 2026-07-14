use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFString, CFType, CGRect, ConcreteType,
};
use objc2_core_graphics::{
    kCGNullWindowID, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowNumber,
    CGDisplayBounds, CGMainDisplayID, CGRectMakeWithDictionaryRepresentation,
    CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::{NSNumber, NSString};

use crate::{models::CaptureErrorCode, Error, Result};

use super::{MacRect, OrderedWindow, WindowSnapshot};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WindowGeometry {
    pub snapshot: WindowSnapshot,
    pub frame: MacRect,
}

pub(crate) fn display_frame(display_id: u32) -> Option<MacRect> {
    let screens = NSScreen::screens(MainThreadMarker::new()?);
    let screen_number_key = NSString::from_str("NSScreenNumber");
    let candidates = screens.iter().filter_map(|screen| {
        let description = screen.deviceDescription();
        let value = description.objectForKey(&screen_number_key)?;
        let screen_number = value.downcast_ref::<NSNumber>()?.as_u32();
        let frame = screen.frame();
        Some((
            screen_number,
            MacRect {
                x: frame.origin.x,
                y: frame.origin.y,
                width: frame.size.width,
                height: frame.size.height,
            },
        ))
    });
    display_frame_from_candidates(display_id, candidates)
}

fn display_frame_from_candidates(
    display_id: u32,
    candidates: impl IntoIterator<Item = (u32, MacRect)>,
) -> Option<MacRect> {
    candidates.into_iter().find_map(|(candidate_id, frame)| {
        (candidate_id == display_id && frame.is_valid()).then_some(frame)
    })
}

pub(crate) fn window_geometry(window_id: u32) -> Result<Option<WindowGeometry>> {
    let rows = window_rows(CGWindowListOption::OptionAll)?;
    Ok(rows.into_iter().enumerate().find_map(|(order, row)| {
        let id = number(&row, unsafe { kCGWindowNumber })?.as_i64()? as u32;
        if id != window_id {
            return None;
        }
        let layer = number(&row, unsafe { kCGWindowLayer })?.as_i64()? as i32;
        let on_screen = boolean(&row, unsafe { kCGWindowIsOnscreen })
            .map(CFBoolean::as_bool)
            .unwrap_or(false);
        let frame = appkit_rect(bounds(&row)?);
        if !frame.is_valid() {
            return None;
        }
        Some(WindowGeometry {
            snapshot: WindowSnapshot {
                id,
                layer,
                order,
                on_screen,
                minimized: !on_screen,
            },
            frame,
        })
    }))
}

pub(crate) fn ordered_windows(target_id: u32, panel_ids: &[u32]) -> Result<Vec<OrderedWindow>> {
    let rows = window_rows(CGWindowListOption::OptionOnScreenOnly)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let id = number(&row, unsafe { kCGWindowNumber })?.as_i64()? as u32;
            let layer = number(&row, unsafe { kCGWindowLayer })?.as_i64()? as i32;
            Some(if id == target_id {
                OrderedWindow::target_at_layer(id, layer)
            } else if panel_ids.contains(&id) {
                OrderedWindow::panel_at_layer(id, layer)
            } else {
                OrderedWindow::other_at_layer(id, layer)
            })
        })
        .collect())
}

fn window_rows(
    option: CGWindowListOption,
) -> Result<Vec<objc2_core_foundation::CFRetained<CFDictionary<CFString, CFType>>>> {
    let rows = CGWindowListCopyWindowInfo(option, kCGNullWindowID)
        .ok_or_else(|| Error::new(CaptureErrorCode::Internal, "无法读取 macOS 窗口层级", true))?;
    // SAFETY: CGWindowListCopyWindowInfo documents every array element as a CFDictionary
    // whose keys are CFString and whose values are Core Foundation objects.
    let rows = unsafe { rows.cast_unchecked::<CFDictionary<CFString, CFType>>() };
    Ok(rows.iter().collect())
}

fn value<'a, T: ConcreteType>(
    row: &'a CFDictionary<CFString, CFType>,
    key: &CFString,
) -> Option<&'a T> {
    // SAFETY: The dictionary is immutable for the duration of this borrow.
    unsafe { row.get_unchecked(key) }?.downcast_ref::<T>()
}

fn number<'a>(row: &'a CFDictionary<CFString, CFType>, key: &CFString) -> Option<&'a CFNumber> {
    value(row, key)
}

fn boolean<'a>(row: &'a CFDictionary<CFString, CFType>, key: &CFString) -> Option<&'a CFBoolean> {
    value(row, key)
}

fn bounds(row: &CFDictionary<CFString, CFType>) -> Option<CGRect> {
    let dictionary = value::<CFDictionary>(row, unsafe { kCGWindowBounds })?;
    let mut rect = CGRect::ZERO;
    // SAFETY: CoreGraphics produced the bounds dictionary and rect is a valid out pointer.
    let valid = unsafe { CGRectMakeWithDictionaryRepresentation(Some(dictionary), &mut rect) };
    valid.then_some(rect)
}

fn appkit_rect(rect: CGRect) -> MacRect {
    let main = CGDisplayBounds(CGMainDisplayID());
    MacRect {
        x: rect.origin.x,
        y: main.size.height - rect.origin.y - rect.size.height,
        width: rect.size.width,
        height: rect.size.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_display_frame_by_legacy_screen_number() {
        let expected = MacRect {
            x: 2560.0,
            y: 0.0,
            width: 2560.0,
            height: 1440.0,
        };
        let candidates = [
            (
                2026488832,
                MacRect {
                    x: 0.0,
                    y: 0.0,
                    width: 2560.0,
                    height: 1440.0,
                },
            ),
            (1783228483, expected),
        ];

        assert_eq!(
            display_frame_from_candidates(1783228483, candidates),
            Some(expected)
        );
    }
}
