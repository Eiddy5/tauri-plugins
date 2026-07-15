use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGRect, ConcreteType,
};
use objc2_core_graphics::{
    kCGNullWindowID, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowNumber,
    kCGWindowOwnerPID, CGDisplayBounds, CGMainDisplayID, CGRectMakeWithDictionaryRepresentation,
    CGWindowListCopyWindowInfo, CGWindowListCreate, CGWindowListCreateDescriptionFromArray,
    CGWindowListOption,
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
    let Some(row) = window_row(window_id)? else {
        return Ok(None);
    };
    let Some(id) = number(&row, unsafe { kCGWindowNumber })
        .and_then(CFNumber::as_i64)
        .map(|id| id as u32)
    else {
        return Ok(None);
    };
    let Some(layer) = number(&row, unsafe { kCGWindowLayer })
        .and_then(CFNumber::as_i64)
        .map(|layer| layer as i32)
    else {
        return Ok(None);
    };
    let on_screen = boolean(&row, unsafe { kCGWindowIsOnscreen })
        .map(CFBoolean::as_bool)
        .unwrap_or(false);
    let Some(frame) = bounds(&row)
        .map(appkit_rect)
        .filter(|frame| frame.is_valid())
    else {
        return Ok(None);
    };

    Ok(Some(WindowGeometry {
        snapshot: WindowSnapshot {
            id,
            layer,
            // A targeted description does not contain global Z-order. Full order is read only
            // by ordered_windows() during the slower correction/reposition path.
            order: 0,
            on_screen,
            minimized: !on_screen,
        },
        frame,
    }))
}

type WindowRow = CFRetained<CFDictionary<CFString, CFType>>;

fn window_row(window_id: u32) -> Result<Option<WindowRow>> {
    let window_ids = CGWindowListCreate(CGWindowListOption::OptionIncludingWindow, window_id)
        .ok_or_else(|| {
            Error::new(
                CaptureErrorCode::Internal,
                "无法构造 macOS 目标窗口查询",
                true,
            )
        })?;
    // SAFETY: CGWindowListCreate produced an array containing the requested CGWindowID in the
    // exact representation required by CGWindowListCreateDescriptionFromArray.
    let rows = unsafe { CGWindowListCreateDescriptionFromArray(Some(&window_ids)) }
        .ok_or_else(|| Error::new(CaptureErrorCode::Internal, "无法读取 macOS 目标窗口", true))?;
    // SAFETY: CoreGraphics documents every returned array element as a window dictionary.
    let rows = unsafe { rows.cast_unchecked::<CFDictionary<CFString, CFType>>() };
    Ok(rows.iter().next())
}

pub(crate) fn ordered_windows(target_id: u32, panel_ids: &[u32]) -> Result<Vec<OrderedWindow>> {
    let rows = window_rows(CGWindowListOption::OptionOnScreenOnly)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let id = number(&row, unsafe { kCGWindowNumber })?.as_i64()? as u32;
            let layer = number(&row, unsafe { kCGWindowLayer })?.as_i64()? as i32;
            let owner_pid = number(&row, unsafe { kCGWindowOwnerPID })?.as_i64()? as u32;
            let frame = appkit_rect(bounds(&row)?);
            Some(ordered_window_from_fields(
                id, layer, owner_pid, frame, target_id, panel_ids,
            ))
        })
        .collect())
}

fn ordered_window_from_fields(
    id: u32,
    layer: i32,
    owner_pid: u32,
    frame: MacRect,
    target_id: u32,
    panel_ids: &[u32],
) -> OrderedWindow {
    let window = if id == target_id {
        OrderedWindow::target_at_layer(id, layer)
    } else if panel_ids.contains(&id) {
        OrderedWindow::panel_at_layer(id, layer)
    } else {
        OrderedWindow::other_at_layer(id, layer)
    };
    window.with_owner_pid(owner_pid).with_frame(frame)
}

fn window_rows(option: CGWindowListOption) -> Result<Vec<WindowRow>> {
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
    use crate::overlay::macos::{corner_panel_frames, visible_corner_panels};

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

    #[test]
    fn ordered_rows_preserve_owner_and_frame_for_sibling_visibility() {
        let target_frame = MacRect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        let corners = corner_panel_frames(target_frame, 32.0);
        let windows = [
            ordered_window_from_fields(
                7,
                0,
                42,
                MacRect {
                    x: 0.0,
                    y: 80.0,
                    width: 10.0,
                    height: 10.0,
                },
                8,
                &[],
            ),
            ordered_window_from_fields(8, 0, 42, target_frame, 8, &[]),
        ];

        assert_eq!(
            visible_corner_panels(&windows, 8, &corners),
            [false, true, true, true]
        );
    }

    #[test]
    fn targeted_window_query_returns_no_row_for_an_invalid_id() {
        assert!(window_row(u32::MAX).unwrap().is_none());
    }

    #[test]
    fn targeted_window_query_returns_a_row_for_a_valid_on_screen_id() {
        let rows = window_rows(CGWindowListOption::OptionOnScreenOnly).unwrap();
        let window_id = rows
            .iter()
            .find_map(|row| number(row, unsafe { kCGWindowNumber })?.as_i64())
            .expect("当前 GUI 会话应至少包含一个可见窗口") as u32;

        assert!(window_row(window_id).unwrap().is_some());
    }
}
