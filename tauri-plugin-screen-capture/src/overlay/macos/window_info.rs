use std::ptr::{self, NonNull};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSScreen};
use objc2_application_services::{
    kAXTrustedCheckOptionPrompt, AXError, AXIsProcessTrustedWithOptions, AXUIElement, AXValue,
    AXValueType,
};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
    CGSize, ConcreteType,
};
use objc2_core_graphics::{
    kCGNullWindowID, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowName,
    kCGWindowNumber, kCGWindowOwnerPID, CGDisplayBounds, CGMainDisplayID,
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListCreate,
    CGWindowListCreateDescriptionFromArray, CGWindowListOption,
};
use objc2_foundation::{NSNumber, NSString};

use crate::{models::CaptureErrorCode, Error, Result};

use super::{MacRect, OrderedWindow, WindowSnapshot};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WindowGeometry {
    pub snapshot: WindowSnapshot,
    pub frame: MacRect,
}

#[derive(Clone, Debug, PartialEq)]
struct FocusTarget {
    owner_pid: u32,
    title: Option<String>,
    frame: MacRect,
}

#[derive(Clone, Debug, PartialEq)]
struct AxWindowCandidate {
    index: usize,
    title: Option<String>,
    frame: MacRect,
}

const AX_FRAME_TOLERANCE: f64 = 12.0;

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

pub(crate) fn tauri_input_rect(frame: MacRect) -> MacRect {
    let main = CGDisplayBounds(CGMainDisplayID());
    MacRect {
        x: frame.x,
        y: main.size.height - frame.y - frame.height,
        width: frame.width,
        height: frame.height,
    }
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

#[cfg(test)]
pub(crate) fn window_owner_pid(window_id: u32) -> Result<Option<u32>> {
    let Some(row) = window_row(window_id)? else {
        return Ok(None);
    };
    Ok(number(&row, unsafe { kCGWindowOwnerPID })
        .and_then(CFNumber::as_i64)
        .and_then(|pid| u32::try_from(pid).ok()))
}

pub(crate) fn activate_window_owner(window_id: u32) -> Result<bool> {
    let Some(target) = focus_target(window_id)? else {
        return Ok(false);
    };
    let Ok(owner_pid) = i32::try_from(target.owner_pid) else {
        return Ok(false);
    };
    let Some(application) =
        NSRunningApplication::runningApplicationWithProcessIdentifier(owner_pid)
    else {
        return Ok(false);
    };

    #[allow(deprecated)]
    let activated =
        application.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);

    if !accessibility_is_trusted() {
        tracing::debug!(
            window_id,
            owner_pid,
            "macOS 辅助功能权限未授予，目标窗口仅激活所属应用"
        );
        return Ok(activated);
    }

    match raise_accessibility_window(owner_pid, &target) {
        Ok(true) => Ok(true),
        Ok(false) => {
            tracing::debug!(
                window_id,
                owner_pid,
                "未能唯一匹配 macOS 辅助功能窗口，目标窗口仅激活所属应用"
            );
            Ok(activated)
        }
        Err(error) => {
            tracing::debug!(
                %error,
                window_id,
                owner_pid,
                "提升 macOS 目标窗口失败，已降级为激活所属应用"
            );
            Ok(activated)
        }
    }
}

fn focus_target(window_id: u32) -> Result<Option<FocusTarget>> {
    let Some(row) = window_row(window_id)? else {
        return Ok(None);
    };
    let Some(owner_pid) = number(&row, unsafe { kCGWindowOwnerPID })
        .and_then(CFNumber::as_i64)
        .and_then(|pid| u32::try_from(pid).ok())
    else {
        return Ok(None);
    };
    let Some(frame) = bounds(&row)
        .map(quartz_rect)
        .filter(|frame| frame.is_valid())
    else {
        return Ok(None);
    };
    let title = value::<CFString>(&row, unsafe { kCGWindowName })
        .map(ToString::to_string)
        .filter(|title| !title.trim().is_empty());
    Ok(Some(FocusTarget {
        owner_pid,
        title,
        frame,
    }))
}

fn accessibility_is_trusted() -> bool {
    let options = CFDictionary::<CFString, CFBoolean>::from_slices(
        &[unsafe { kAXTrustedCheckOptionPrompt }],
        &[CFBoolean::new(true)],
    );
    // SAFETY: The options dictionary contains the documented prompt key and a CFBoolean value.
    unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) }
}

fn raise_accessibility_window(owner_pid: i32, target: &FocusTarget) -> Result<bool> {
    // SAFETY: owner_pid came from CoreGraphics for a live window owner.
    let application = unsafe { AXUIElement::new_application(owner_pid) };
    let Some(windows) = ax_windows(&application) else {
        return Ok(false);
    };
    let candidates = windows
        .iter()
        .enumerate()
        .filter_map(|(index, window)| ax_window_candidate(index, &window))
        .collect::<Vec<_>>();
    let Some(index) = select_ax_window(target, &candidates) else {
        return Ok(false);
    };
    let Some(window) = windows.get(index) else {
        return Ok(false);
    };

    let minimized = CFString::from_static_str("AXMinimized");
    // SAFETY: AXMinimized accepts a CFBoolean. Unsupported attributes are harmless here.
    let _ = unsafe { window.set_attribute_value(&minimized, CFBoolean::new(false)) };

    for attribute in ["AXMainWindow", "AXFocusedWindow"] {
        let attribute = CFString::from_static_str(attribute);
        // SAFETY: Both application attributes accept an AXUIElement window from that application.
        let _ = unsafe { application.set_attribute_value(&attribute, &window) };
    }

    let raise = CFString::from_static_str("AXRaise");
    // SAFETY: AXRaise is the documented action for promoting a window within its application.
    let status = unsafe { window.perform_action(&raise) };
    if status == AXError::Success {
        Ok(true)
    } else {
        Err(Error::new(
            CaptureErrorCode::Internal,
            format!("macOS AXRaise 失败: {}", status.0),
            true,
        ))
    }
}

fn ax_windows(application: &AXUIElement) -> Option<CFRetained<CFArray<AXUIElement>>> {
    let value = copy_ax_attribute(application, "AXWindows")?;
    let array = value.downcast::<CFArray>().ok()?;
    // SAFETY: AXWindows is documented as an array of AXUIElement window objects.
    Some(unsafe { CFRetained::cast_unchecked(array) })
}

fn ax_window_candidate(index: usize, window: &AXUIElement) -> Option<AxWindowCandidate> {
    let position = ax_point(window, "AXPosition")?;
    let size = ax_size(window, "AXSize")?;
    let frame = MacRect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    if !frame.is_valid() {
        return None;
    }
    let title = copy_ax_attribute(window, "AXTitle")
        .and_then(|value| value.downcast::<CFString>().ok())
        .map(|title| title.to_string())
        .filter(|title| !title.trim().is_empty());
    Some(AxWindowCandidate {
        index,
        title,
        frame,
    })
}

fn copy_ax_attribute(element: &AXUIElement, attribute: &'static str) -> Option<CFRetained<CFType>> {
    let attribute = CFString::from_static_str(attribute);
    let mut value: *const CFType = ptr::null();
    // SAFETY: value is a valid out pointer. On success, CopyAttributeValue returns a retained
    // Core Foundation object which is transferred into CFRetained below.
    let status = unsafe { element.copy_attribute_value(&attribute, NonNull::from(&mut value)) };
    if status != AXError::Success {
        return None;
    }
    let value = NonNull::new(value.cast_mut())?;
    // SAFETY: AXUIElementCopyAttributeValue follows the Copy rule and returned a non-null CFType.
    Some(unsafe { CFRetained::from_raw(value) })
}

fn ax_point(element: &AXUIElement, attribute: &'static str) -> Option<CGPoint> {
    let value = copy_ax_attribute(element, attribute)?;
    let value = value.downcast::<AXValue>().ok()?;
    let mut point = CGPoint::ZERO;
    // SAFETY: point is a valid output pointer and AXPosition is documented as CGPoint AXValue.
    unsafe {
        value
            .value(AXValueType::CGPoint, NonNull::from(&mut point).cast())
            .then_some(point)
    }
}

fn ax_size(element: &AXUIElement, attribute: &'static str) -> Option<CGSize> {
    let value = copy_ax_attribute(element, attribute)?;
    let value = value.downcast::<AXValue>().ok()?;
    let mut size = CGSize::ZERO;
    // SAFETY: size is a valid output pointer and AXSize is documented as CGSize AXValue.
    unsafe {
        value
            .value(AXValueType::CGSize, NonNull::from(&mut size).cast())
            .then_some(size)
    }
}

fn select_ax_window(target: &FocusTarget, candidates: &[AxWindowCandidate]) -> Option<usize> {
    let mut matches = candidates
        .iter()
        .filter_map(|candidate| {
            let error = frame_error(target.frame, candidate.frame);
            (error <= AX_FRAME_TOLERANCE * 4.0).then_some((
                candidate.title == target.title && target.title.is_some(),
                error,
                candidate.index,
            ))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.total_cmp(&right.1))
    });
    let best = matches.first().copied()?;
    let tied = matches
        .get(1)
        .is_some_and(|next| next.0 == best.0 && (next.1 - best.1).abs() < f64::EPSILON);
    (!tied).then_some(best.2)
}

fn frame_error(left: MacRect, right: MacRect) -> f64 {
    (left.x - right.x).abs()
        + (left.y - right.y).abs()
        + (left.width - right.width).abs()
        + (left.height - right.height).abs()
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

pub(crate) fn visible_window_ids() -> Result<Vec<u32>> {
    let windows = CGWindowListCreate(CGWindowListOption::OptionOnScreenOnly, kCGNullWindowID)
        .ok_or_else(|| {
            Error::new(
                CaptureErrorCode::Internal,
                "无法读取 macOS 可见窗口顺序",
                true,
            )
        })?;

    Ok((0..windows.count())
        .map(|index| {
            // SAFETY: CGWindowListCreate returns pointer-encoded CGWindowID entries, and the
            // index is within the array's reported count. The pointer value is not dereferenced.
            (unsafe { windows.value_at_index(index) }) as usize as u32
        })
        .collect())
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

fn quartz_rect(rect: CGRect) -> MacRect {
    MacRect {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::macos::{visible_corner_layers, OverlayPanelLayout};

    fn focus_target_at(title: Option<&str>, frame: MacRect) -> FocusTarget {
        FocusTarget {
            owner_pid: 42,
            title: title.map(str::to_owned),
            frame,
        }
    }

    fn ax_candidate(index: usize, title: Option<&str>, frame: MacRect) -> AxWindowCandidate {
        AxWindowCandidate {
            index,
            title: title.map(str::to_owned),
            frame,
        }
    }

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
        let layout = OverlayPanelLayout::new(target_frame, 32.0);
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
            visible_corner_layers(&windows, 8, &layout.corner_frames),
            [false, true, true, true]
        );
    }

    #[test]
    fn accessibility_match_prefers_matching_title_and_geometry() {
        let frame = MacRect {
            x: 100.0,
            y: 80.0,
            width: 900.0,
            height: 700.0,
        };
        let target = focus_target_at(Some("Shared document"), frame);
        let candidates = [
            ax_candidate(0, Some("Other document"), frame),
            ax_candidate(
                1,
                Some("Shared document"),
                MacRect {
                    x: 101.0,
                    y: 80.0,
                    ..frame
                },
            ),
        ];

        assert_eq!(select_ax_window(&target, &candidates), Some(1));
    }

    #[test]
    fn accessibility_match_uses_unique_geometry_when_title_is_unavailable() {
        let frame = MacRect {
            x: -1200.0,
            y: 40.0,
            width: 800.0,
            height: 600.0,
        };
        let target = focus_target_at(None, frame);
        let candidates = [
            ax_candidate(3, None, frame),
            ax_candidate(
                4,
                None,
                MacRect {
                    x: 400.0,
                    y: 300.0,
                    ..frame
                },
            ),
        ];

        assert_eq!(select_ax_window(&target, &candidates), Some(3));
    }

    #[test]
    fn accessibility_match_rejects_ambiguous_windows() {
        let frame = MacRect {
            x: 100.0,
            y: 80.0,
            width: 900.0,
            height: 700.0,
        };
        let target = focus_target_at(Some("Duplicate"), frame);
        let candidates = [
            ax_candidate(0, Some("Duplicate"), frame),
            ax_candidate(1, Some("Duplicate"), frame),
        ];

        assert_eq!(select_ax_window(&target, &candidates), None);
    }

    #[test]
    fn accessibility_match_rejects_distant_geometry() {
        let target = focus_target_at(
            Some("Shared document"),
            MacRect {
                x: 100.0,
                y: 80.0,
                width: 900.0,
                height: 700.0,
            },
        );
        let candidates = [ax_candidate(
            0,
            Some("Shared document"),
            MacRect {
                x: 500.0,
                y: 400.0,
                width: 900.0,
                height: 700.0,
            },
        )];

        assert_eq!(select_ax_window(&target, &candidates), None);
    }

    #[test]
    fn targeted_window_query_returns_no_row_for_an_invalid_id() {
        assert!(window_row(u32::MAX).unwrap().is_none());
    }

    #[test]
    fn targeted_window_owner_query_returns_none_for_an_invalid_id() {
        assert!(window_owner_pid(u32::MAX).unwrap().is_none());
    }

    #[test]
    #[ignore = "requires a logged-in macOS GUI session"]
    fn targeted_window_query_returns_a_row_for_a_valid_on_screen_id() {
        let rows = window_rows(CGWindowListOption::OptionOnScreenOnly).unwrap();
        let window_id = rows
            .iter()
            .find_map(|row| number(row, unsafe { kCGWindowNumber })?.as_i64())
            .expect("当前 GUI 会话应至少包含一个可见窗口") as u32;

        assert!(window_row(window_id).unwrap().is_some());
    }

    #[test]
    #[ignore = "requires a logged-in macOS GUI session"]
    fn lightweight_window_ids_include_a_valid_on_screen_window() {
        let rows = window_rows(CGWindowListOption::OptionOnScreenOnly).unwrap();
        let window_id = rows
            .iter()
            .find_map(|row| number(row, unsafe { kCGWindowNumber })?.as_i64())
            .expect("当前 GUI 会话应至少包含一个可见窗口") as u32;

        assert!(visible_window_ids().unwrap().contains(&window_id));
    }
}
