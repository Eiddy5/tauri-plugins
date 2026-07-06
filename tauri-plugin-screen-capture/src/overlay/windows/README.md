# Windows share overlay

This module owns the Windows implementation of the screen sharing overlay.
Its public surface stays intentionally small: `WindowsShareOverlay` plus the
Windows source-id parsing helpers re-exported from `mod.rs`.

## Files

- `mod.rs`: facade implementing `ShareOverlay` and routing display/window targets.
- `bounds.rs`: source-id parsing and Windows display/window bounds lookup.
- `placement.rs`: overlay placement and one-shot focus behavior.
- `events.rs`: WinEvent hook registration, target registry, and event-to-command mapping.
- `host.rs`: overlay UI thread, command loop, delayed refresh, and hook lifecycle.
- `window.rs`: Win32 overlay window creation, positioning, showing, hiding, and teardown.
- `util.rs`: small Win32 conversion and error helpers shared inside this module.

## Behavior contract

Window sharing uses an owned popup overlay so the border follows the target
window's z-order instead of floating globally above unrelated windows. During
move/resize and fullscreen transitions, the overlay hides, suppresses
location-change hook noise, coalesces refreshes, waits for pointer release, and
then redraws once against the latest window bounds.
