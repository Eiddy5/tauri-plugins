# Windows Share Border Design

## Goal

The screen capture plugin should show a local green sharing indicator while a Windows capture session is active. The indicator replaces the Windows system capture border that is disabled with `DrawBorderSettings::WithoutBorder`.

The indicator must make it clear which display or window is being shared, without appearing in the captured stream.

## Non-Goals

- Do not rely on the Windows yellow capture border.
- Do not draw the indicator in the frontend DOM.
- Do not include the indicator in the remote shared video.
- Do not implement macOS overlay behavior in this change.
- Do not add a full styling API before the default behavior works.

## User Behavior

For display sharing, the indicator frames only the selected display.

For window sharing, the indicator frames the target window outer bounds, including title bar and native window frame. The indicator is not required to follow continuously while the user is dragging or resizing the target window. During move or resize, it should hide. When the operation ends, it should recalculate the latest target bounds and show again.

If the target window is hidden or minimized, the indicator hides. If the target window becomes visible again, the indicator reappears at the latest bounds. If the target window is destroyed, the indicator is destroyed and the capture session should move toward the same source-unavailable behavior as the capture backend.

## Visual Style

The first version uses corner-only green markers:

- Color: `#22C55E`
- Thickness: `4px`
- Corner length: `32px`
- Shape: eight rectangular overlay segments, two per corner

The indicator does not draw full-length edges. This keeps the signal visible without looking like the old Windows system border and leaves room for a future branded sharing UI.

## Windows Overlay Architecture

Add a Windows-only overlay subsystem under the plugin. It owns native Win32 overlay windows for a capture session.

Each active indicator consists of eight small top-level windows:

- top-left horizontal
- top-left vertical
- top-right horizontal
- top-right vertical
- bottom-left horizontal
- bottom-left vertical
- bottom-right horizontal
- bottom-right vertical

Each overlay window should be:

- always on top
- click-through
- not shown in the taskbar
- visually just a solid green rectangle
- excluded from capture using `SetWindowDisplayAffinity(..., WDA_EXCLUDEFROMCAPTURE)` when supported

The overlay should belong to the current process so `SetWindowDisplayAffinity` can be applied.

## Bounds and Geometry

The overlay subsystem receives a target rectangle in physical screen coordinates and computes eight segment rectangles from the style values.

For a target rect `(left, top, right, bottom)`, thickness `t`, and corner length `l`, the computed overlay segments are:

- top-left horizontal: `(left, top, left + l, top + t)`
- top-left vertical: `(left, top, left + t, top + l)`
- top-right horizontal: `(right - l, top, right, top + t)`
- top-right vertical: `(right - t, top, right, top + l)`
- bottom-left horizontal: `(left, bottom - t, left + l, bottom)`
- bottom-left vertical: `(left, bottom - l, left + t, bottom)`
- bottom-right horizontal: `(right - l, bottom - t, right, bottom)`
- bottom-right vertical: `(right - t, bottom - l, right, bottom)`

If the target is smaller than twice the corner length, clamp the corner length to fit without crossing opposite corners.

## Event Model

For window sharing, use WinEvent hooks as the primary trigger:

- `EVENT_SYSTEM_MOVESIZESTART`: hide the indicator
- `EVENT_SYSTEM_MOVESIZEEND`: read current window bounds and show the indicator
- `EVENT_OBJECT_LOCATIONCHANGE`: update bounds after movement or size changes when not in move/size mode
- `EVENT_OBJECT_HIDE`: hide the indicator
- `EVENT_OBJECT_SHOW`: read bounds and show the indicator
- `EVENT_OBJECT_DESTROY`: destroy the indicator and report source unavailability through the same session error path used by the capture backend

Use low-frequency polling as a fallback because system events can be missed for some windows or privilege boundaries. Polling should correct stale bounds, visibility, and destroyed-window state.

For display sharing, polling is enough for the first version. Recompute display bounds periodically to handle display layout changes.

## Capture Session Integration

The overlay should be started after a capture session starts successfully and stopped when the session stops. It should hide on pause and show again on resume if the target is visible.

The overlay should not be coupled to frame publishing. A slow WebRTC encoder must not affect overlay visibility updates.

The overlay manager should be stored with the session record so each session can own its own indicator lifecycle.

## Public API

The first version does not expose style configuration. Defaults are internal constants.

The design leaves room for a later `StartCaptureOptions` extension:

```ts
border?: {
  enabled?: boolean
  color?: string
  thickness?: number
  cornerLength?: number
  mode?: "corners" | "outline"
}
```

## Testing Strategy

Automated tests cover pure and session-level behavior:

- Given a target rect and style, geometry calculation returns eight corner segment rects.
- Small targets clamp corner lengths without invalid rectangles.
- Default style uses green `#22C55E`, `4px` thickness, and `32px` corner length.
- Session lifecycle starts, hides, resumes, and stops the overlay manager through an abstraction.

Manual Windows verification is required for native behavior:

- Overlay appears for display sharing.
- Overlay appears for window sharing.
- Overlay hides while moving or resizing a window and reappears afterward.
- Overlay does not intercept clicks.
- Overlay does not appear in the captured stream.
- Overlay does not show in the taskbar.

## Risks

`WDA_EXCLUDEFROMCAPTURE` is supported starting with Windows 10 version 2004. Older systems may behave like `WDA_MONITOR`, which can black out the overlay area instead of excluding it. The plugin should detect failures and log a clear warning.

WinEvent hooks can miss events for elevated or special windows. Polling fallback is required.

Coordinate handling must be DPI-aware. The overlay subsystem should use physical screen coordinates consistently with the capture source bounds.
