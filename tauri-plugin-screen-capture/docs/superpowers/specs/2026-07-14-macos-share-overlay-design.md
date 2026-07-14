# macOS Share Overlay Design

## Status

Accepted for implementation on 2026-07-14.

## Goal

Add a native macOS sharing indicator for both display and single-window capture. The indicator must match the existing Windows behavior closely:

- use green corner markers with the existing `OverlayStyle` defaults;
- remain local and never appear in the ScreenCaptureKit output;
- never take focus or intercept pointer input;
- hide while a target window is being dragged and reappear at its latest bounds;
- hide when capture is paused and be destroyed when capture stops;
- add negligible idle CPU/GPU work and no continuous render loop.

## Existing Architecture

`ScreenCaptureState` owns one `ShareOverlay` per capture session. Capture starts first, then the publisher starts, and finally `ShareOverlay::start` shows the indicator. Pause, resume, stop, and asynchronous capture completion already drive `hide`, `show`, and `stop` respectively.

Windows implements the trait with eight native Win32 popup windows. Window overlays are owned non-topmost windows; display overlays are topmost. WinEvent hooks drive visibility and geometry, and `WDA_EXCLUDEFROMCAPTURE` prevents the windows from entering captured output.

macOS currently receives `NoopShareOverlay` from `DefaultShareOverlayFactory`, while its ScreenCaptureKit display filter captures the entire selected display without exclusions.

## Non-Goals

- Do not add egui, wgpu, GLFW, a WebView, or a helper executable.
- Do not add interactive controls, annotations, text, animation, or a public overlay styling API.
- Do not require Accessibility or Input Monitoring permission.
- Do not continuously follow a window at display refresh rate while it is dragged.
- Do not change the Windows overlay implementation or its behavior.
- Do not promise zero resource use; enforce measurable performance budgets instead.

## Chosen Approach

Use four small borderless `NSPanel` instances per active overlay, one for each target corner. Each panel contains a layer-backed transparent `NSView` with two solid `CALayer` children forming an L-shaped marker.

Four `32 x 32` point surfaces avoid allocating and compositing a display-sized transparent surface. The layers are created once and remain static until the color, scale, or geometry changes. There is no `CADisplayLink`, animation timer, egui render loop, Metal render loop, or per-frame drawing callback.

The implementation uses explicit `objc2` AppKit, Foundation, QuartzCore, and CoreGraphics bindings already compatible with the Tauri dependency graph.

## Window Configuration

Every corner panel is configured as follows:

- borderless and transparent;
- no shadow;
- non-opaque with a clear background;
- ignores mouse events;
- cannot become key or main;
- excluded from the Window menu and normal window cycling;
- joins all Spaces and can appear alongside a full-screen application;
- uses no implicit Core Animation actions when moving, resizing, showing, or hiding;
- has a stable private title prefix so ScreenCaptureKit can distinguish overlay windows from normal application windows.

Display overlays use a high window level suitable for a local sharing indicator. Window overlays use conservative visibility rules: they are shown only while the target application/window is considered visible and eligible, preventing the indicator from floating above an unrelated foreground application. Native integration tests determine the final level constants and verify full-screen and Stage Manager behavior.

## Rendering and Geometry

The shared cross-platform `OverlayStyle` remains authoritative:

- color: `#22C55E`;
- thickness: `4` points;
- corner length: `32` points.

Each panel is `corner_length x corner_length`. Its two child layers form the horizontal and vertical portions of one corner. For a small target, the existing clamping semantics apply before panel frames and child-layer frames are calculated.

All geometry is expressed in logical screen points at the AppKit boundary. Each layer's `contentsScale` follows the backing scale factor of its current `NSScreen`, preserving sharp edges on Retina and mixed-scale multi-display configurations.

`CATransaction` disables implicit actions during geometry and visibility updates. The design does not use `shouldRasterize`, blur, masks, rounded corners, shadows, or asynchronous custom drawing, all of which would add unnecessary offscreen work for solid rectangles.

## Bounds and Coordinate Mapping

Display source IDs already contain the ScreenCaptureKit `CGDirectDisplayID`. The overlay resolves the corresponding `NSScreen` through its `NSScreenNumber` device description and uses that screen's AppKit frame.

Window source IDs contain a `CGWindowID`. Window bounds and visibility are read from `CGWindowListCopyWindowInfo` for that specific ID. Quartz window coordinates are converted to AppKit screen coordinates with a per-display mapping, rather than assuming the primary display's origin or scale. Tests cover displays positioned above, below, left, and right of the primary display as well as mixed Retina scales.

If the source no longer exists, is minimized, is off the active Space, or has invalid/empty bounds, the overlay hides. The capture backend remains the authority that eventually reports source unavailability and finishes the capture session.

## Event Model

The implementation is event-driven at idle:

1. Observe local and global left-mouse-drag events. On the first drag event, hide a window overlay immediately.
2. Observe left-mouse-up. Re-read the target bounds once, reposition all four panels in one main-thread transaction, and show them if the target remains eligible.
3. Observe `NSWorkspace` application activation, hiding, termination, active-Space, sleep, and wake notifications. Re-evaluate visibility only for relevant changes.
4. Observe display-configuration changes and recompute display bounds.
5. Run a low-frequency correction check while a window overlay is visible to catch programmatic moves or missed notifications. The check compares the latest target snapshot and submits no AppKit updates when nothing changed.

No Accessibility `AXObserver` or `CGEventTap` is used. This avoids adding permission prompts. The trade-off is that programmatic moves from another process may be corrected on the next low-frequency check rather than synchronously.

## AppKit Main-Thread Ownership

AppKit windows and event monitors are created, accessed, and destroyed only on the macOS main thread.

The plugin setup already receives `AppHandle<R>`. Add an internal object-safe main-thread dispatcher backed by `AppHandle::run_on_main_thread` and inject it into `MacOsShareOverlayFactory` during plugin setup.

`ShareOverlay` must remain `Send + Sync`, so it must not contain `NSPanel`, `NSView`, `CALayer`, or event-monitor objects. The Rust overlay handle contains only a generated overlay ID and the dispatcher. Actual AppKit state lives in a main-thread-only registry keyed by overlay ID. Async trait methods dispatch a command and await a one-shot response.

This mirrors the Windows separation between the session-facing handle and the UI-thread-owned native window state without introducing a second macOS event loop.

## ScreenCaptureKit Exclusion

Do not use `NSWindow.sharingType`; Apple defines the non-sharing value as a legacy mechanism that must not be relied on to omit content from capture.

For display capture, construct the ScreenCaptureKit filter using the current `SCRunningApplication` as an excluded application and pass all visible non-overlay windows belonging to the current process as `exceptingWindows`.

This has three important properties:

1. Existing Tauri application windows remain visible in display sharing.
2. All overlay panels are excluded, including panels created after the stream starts.
3. Overlays from concurrent capture sessions are excluded without maintaining a per-stream list of overlay window IDs.

Overlay windows are recognized by both current process ID and the reserved private title prefix. If the current process cannot be resolved to an `SCRunningApplication`, display capture startup fails instead of risking that the overlay leaks into the shared output.

Single-window capture continues to use `SCContentFilter::with_window`. Because the filter includes only the selected target window, overlay panels are not part of the stream.

When a new normal Tauri window is created while display capture is already active, the filter must be refreshed so it becomes an `exceptingWindow`; otherwise it remains excluded with the rest of the current application. This is a correctness issue for application-window visibility, not an overlay leak. The first implementation may refresh on the existing low-frequency correction path, with an immediate refresh added when a suitable Tauri window-created event is available.

## Module Layout

```text
src/overlay/
  mod.rs                         cross-platform traits, style, geometry
  macos/
    mod.rs                       MacOsShareOverlay facade and factory
    dispatcher.rs                object-safe Tauri main-thread dispatch
    registry.rs                  main-thread-only overlay state registry
    window.rs                    NSPanel/NSView/CALayer creation and updates
    bounds.rs                    source IDs, CGWindow/NSScreen bounds, coordinates
    events.rs                    NSEvent and NSWorkspace observer lifecycle
  windows/                       unchanged Windows implementation

src/platform/macos/
  screencapturekit.rs            display filter exclusion policy
```

The exact file split may be collapsed if a module remains trivial, but AppKit ownership, bounds conversion, event handling, and ScreenCaptureKit filtering must remain distinct responsibilities.

## Session Lifecycle and Rollback

The existing session order remains valid because the display filter excludes the overlay's entire owning application before the panels are created:

1. start ScreenCaptureKit with the exclusion-aware filter;
2. start the publisher;
3. create and show the overlay;
4. insert the session record.

If overlay startup fails, stop capture and publisher as the existing state logic already does. Native startup must also remove any partially registered monitors and close any partially created panels before returning the error.

Pause orders panels out and suspends the correction check. Resume recomputes the latest bounds before ordering panels front. Stop removes monitors/notifications, invalidates the correction timer, closes all panels, and removes the registry entry. All operations are idempotent so asynchronous capture completion can safely race with an explicit stop.

## Error Handling

- Invalid source IDs return `SourceNotFound`.
- A vanished window during startup returns `SourceUnavailable`.
- AppKit creation or main-thread dispatch failures return `Internal` with a platform-specific message.
- Failure to construct a safe display filter returns `CaptureStartFailed` and does not fall back to an unfiltered display capture.
- Event-monitor or observer installation failures tear down the partial overlay and fail startup.
- Runtime bounds lookup failures hide the overlay and log a throttled diagnostic; they do not stop frame publishing directly.
- `show`, `hide`, and `stop` remain idempotent.

## Testing Strategy

### Pure and mocked tests

- four corner-panel frames are derived correctly from `OverlayRect` and `OverlayStyle`;
- child horizontal/vertical layer frames form the same eight segments as `corner_segments`;
- small targets clamp without negative or overlapping geometry;
- Quartz-to-AppKit conversion handles negative origins and mixed display layouts;
- session lifecycle calls macOS overlay start/show/hide/stop through the existing abstraction;
- startup rollback destroys partially created native state;
- the display-filter classifier excludes reserved overlay windows and excepts normal current-process windows;
- window capture ignores unrelated overlay windows because it includes only its target.

### Native ignored tests on macOS

- four real panels are created and remain non-key, click-through, shadowless, and borderless;
- display and window panel coordinates match real target bounds on Retina displays;
- a synthetic drag dispatch hides panels within one 60 Hz frame;
- mouse-up refresh repositions and shows panels within one frame after bounds resolution;
- pause/resume/stop leave no visible panels or monitors;
- switching applications, Spaces, full-screen mode, and Stage Manager never leaves the window overlay above unrelated content;
- ScreenCaptureKit display frames do not contain the green overlay while ordinary Tauri windows remain visible;
- single-window frames do not contain the overlay;
- two simultaneous capture sessions do not capture each other's overlays.

### Regression commands

```bash
cargo fmt --check
cargo test
cargo test --features macos-screencapturekit
cargo clippy --features macos-screencapturekit --all-targets -- -D warnings
```

Native ignored tests run on a logged-in macOS GUI session with Screen Recording permission.

## Performance Budgets

Measure a release build with Instruments; Core Animation alone is not considered proof of performance.

- no display-link or continuous rendering loop;
- no full-display transparent surface;
- idle overlay CPU below `0.5%` on the supported Apple Silicon test machine;
- drag-event-to-hide P95 below `16 ms`;
- mouse-up-to-visible P95 below `16 ms` after bounds lookup completes;
- no continuous Core Animation offscreen-rendering pass;
- no extra ScreenCaptureKit frame copies, encoder work, or publisher work caused by overlay updates;
- no AppKit geometry submission when the low-frequency snapshot is unchanged.

## Risks and Mitigations

### Cross-process z-order

macOS does not provide the same owned-window relationship used by the Win32 implementation. A permanently topmost window overlay could float above unrelated applications. The implementation therefore combines conservative visibility, workspace activation notifications, ordered window inspection, and hiding during drags. Native tests are mandatory before accepting the level/ordering policy.

### Coordinate systems

Quartz and AppKit use different global coordinate conventions, and multi-display layouts may have negative origins and different scales. Keep all conversions in `bounds.rs`, use per-display mapping, and cover the layout matrix with pure tests and real Retina tests.

### ScreenCaptureKit filter exceptions

Excluding the current app and excepting its normal windows is safer than attempting to hide overlay panels with deprecated AppKit sharing flags. New normal application windows require a filter refresh. The implementation must fail closed if it cannot construct the exclusion policy.

### Event gaps without Accessibility permission

Avoiding Accessibility permission means no cross-process AX move/resize observer. Mouse drag events cover interactive movement; workspace/display notifications cover lifecycle changes; a low-frequency snapshot corrects missed and programmatic changes.

### Core Animation assumptions

Four small static surfaces should be inexpensive, but performance is hardware- and compositor-dependent. Enforce the stated Instruments budgets and retain native regression measurements rather than relying on architectural expectation alone.
