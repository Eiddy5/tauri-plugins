# Screen Capture Plugin Design

Date: 2026-05-19

## Context

`tauri-plugin-screen-capture` is currently the default Tauri plugin scaffold. The goal is to turn it into a desktop screen capture plugin for meeting-style screen sharing, similar to DingTalk meeting screen share.

The first implementation phase must support a real local validation loop:

- enumerate displays and windows;
- exclude system UI/tool windows by default;
- capture a selected display or window on macOS;
- publish captured frames through a native WebRTC loopback path;
- render the received WebRTC track in the plugin example app;
- keep extension points for later Agora publishing.

The first phase targets a cross-platform API shape, but only macOS capture is implemented. Windows support is represented by capability reporting and backend placeholders.

## Goals

- Provide a Tauri command API for capture capabilities, permissions, source enumeration, capture lifecycle, WebRTC signaling, session status, and stats.
- Support both display capture and window capture in phase one.
- Filter macOS and Windows system UI/tool windows from normal source lists.
- Exclude the current Tauri app window by default to avoid recursive preview capture, while allowing debug mode to include it.
- Use macOS ScreenCaptureKit for real capture in phase one.
- Use a native Rust WebRTC stack for local loopback publishing.
- Use the example Tauri app as the acceptance target: choose a source, start capture, and render the native WebRTC stream in a WebView `<video>`.
- Preserve a clean `CapturePublisher` abstraction so Agora can be added later without changing capture backends or source enumeration.

## Non-Goals

- Windows real capture in phase one.
- Agora SDK integration in phase one.
- Cross-device or dual-instance P2P WebRTC.
- STUN, TURN, NAT traversal, or production signaling.
- Sending high-frequency raw video frames through Tauri IPC.
- Recording to file, annotation, remote control, or system audio sharing.

## Product Scenarios

### Display Sharing

A meeting user opens the example app, selects one display from the display list, starts capture, and sees the selected display rendered in the app through WebRTC loopback.

### Window Sharing

A meeting user selects a normal application window from the window list. System UI windows, toolbars, taskbars, menu bars, and the current Tauri app window are excluded by default. The selected window is captured and rendered in the app through WebRTC loopback.

### Permission Handling

On macOS, the plugin checks Screen Recording permission before capture. If permission is missing, commands return structured errors or guidance that the frontend can use to prompt the user.

### Diagnostics

The example app can enable debug source listing to compare raw and filtered source lists. It can also show capture status, frame counts, dropped frames, publishing state, and last error.

## Use Cases

| ID | Use Case | Phase One |
| --- | --- | --- |
| UC-01 | Get platform capture capabilities | Required |
| UC-02 | Check screen capture permission | Required |
| UC-03 | Request or guide permission setup | Required |
| UC-04 | Enumerate displays | Required |
| UC-05 | Enumerate windows | Required |
| UC-06 | Filter current app and system UI sources | Required |
| UC-07 | Start display capture | Required |
| UC-08 | Start window capture | Required |
| UC-09 | Pause capture | Required |
| UC-10 | Resume capture | Required |
| UC-11 | Stop capture | Required |
| UC-12 | Query capture session state | Required |
| UC-13 | Query capture stats | Required |
| UC-14 | Perform local WebRTC loopback signaling | Required |
| UC-15 | Render captured video in the example app | Required |
| UC-16 | Report source unavailable/window closed errors | Required |
| UC-17 | Expose Windows backend capabilities/placeholders | Required |
| UC-18 | Reserve Agora publisher config and extension point | Required |

## Public API

Command names use `capture` terminology, not `share` terminology.

### Core Commands

- `get_capabilities() -> Capabilities`
- `check_permission() -> PermissionStatus`
- `request_permission() -> PermissionStatus`
- `list_sources(options: ListSourcesOptions) -> Vec<CaptureSource>`
- `start_capture(options: StartCaptureOptions) -> CaptureSession`
- `pause_capture(session_id: String) -> ()`
- `resume_capture(session_id: String) -> ()`
- `stop_capture(session_id: String) -> ()`
- `get_capture_session(session_id: String) -> CaptureSession`
- `get_capture_stats(session_id: String) -> CaptureStats`

### WebRTC Loopback Commands

- `create_webrtc_offer(session_id: String) -> WebRtcOffer`
- `accept_webrtc_answer(session_id: String, answer: WebRtcAnswer) -> ()`
- `add_webrtc_ice_candidate(session_id: String, candidate: WebRtcIceCandidate) -> ()`

### Events

- `screen-capture://session-state`
- `screen-capture://stats`
- `screen-capture://error`
- `screen-capture://webrtc-ice-candidate`

Events carry state, stats, errors, and signaling data only. They must not carry high-frequency video frames.

## Data Model

### Source Kinds

- `display`
- `window`

### Publisher Kinds

- `webrtcLoopback`
- `null`
- `agora`

`agora` is part of the public model in phase one, but returns an explicit unsupported error until the Agora SDK adapter is implemented.

### Capture Status

- `idle`
- `starting`
- `capturing`
- `publishing`
- `paused`
- `stopping`
- `stopped`
- `failed`

`capturing` means frames are being received from the platform backend. `publishing` means frames are flowing into the selected publisher, such as WebRTC loopback.

### List Sources Options

`list_sources` accepts:

- `kinds`: display/window filter;
- `includeThumbnails`: whether to include thumbnails;
- `includeCurrentApp`: default `false`;
- `includeSystemUi`: default `false`;
- `debugRawSources`: default `false`.

Normal UI uses filtered sources. Debug mode may return raw and filtered metadata for diagnosis.

## Architecture

The plugin has three main layers.

### Control Plane

Tauri commands and events expose source enumeration, permission checks, lifecycle control, stats, errors, and WebRTC signaling. This layer does not implement platform capture logic and does not transfer high-frequency frames.

### Capture Pipeline

The capture layer owns platform capture sessions. Phase one implements macOS ScreenCaptureKit. Windows has placeholder capability reporting and interfaces. Captured frames are normalized into an internal frame representation or platform frame wrapper and passed to the pipeline.

### Output / Publisher Layer

The publisher layer consumes capture frames. Phase one implements a WebRTC loopback publisher and a null publisher. Agora is represented as an extension point.

## Rust Module Layout

```text
src/
  lib.rs
  commands.rs
  models.rs
  error.rs
  state.rs

  capture/
    mod.rs
    dummy.rs
    macos.rs
    windows.rs

  sources/
    mod.rs
    filter.rs
    thumbnails.rs

  pipeline/
    mod.rs
    frame.rs
    stats.rs

  publisher/
    mod.rs
    null.rs
    webrtc.rs
    agora.rs

  webrtc/
    mod.rs
    signaling.rs
    track.rs

  platform/
    mod.rs
    macos/
      permissions.rs
      screencapturekit.rs
      window_filter.rs
    windows/
      permissions.rs
      graphics_capture.rs
      window_filter.rs
```

## Module Responsibilities

### `commands.rs`

Adapts Tauri IPC to internal state methods. It should not contain platform capture, WebRTC, or filtering logic.

### `state.rs`

Owns `ScreenCaptureState`, session registry, source cache, publisher registry, and event emission. It coordinates backend, pipeline, and publisher objects.

### `capture/`

Defines `CaptureBackend` and capture session traits. Implementations produce frames and lifecycle events. The capture layer does not know about WebRTC or Agora.

### `sources/`

Defines the normalized `CaptureSource` model and source filtering logic. The filter excludes current app windows and system UI by default, with debug options to inspect raw sources.

### `pipeline/`

Routes frames from capture backends to publishers. It handles fps limiting, pause/resume, stop, backpressure, dropped-frame accounting, stats, and error propagation.

### `publisher/`

Defines `CapturePublisher`. Implementations include:

- `NullPublisher` for tests and self-checks;
- `WebRtcLoopbackPublisher` for phase one example rendering;
- `AgoraPublisher` placeholder for future RTC distribution.

### `webrtc/`

Owns native PeerConnection setup, video track creation, frame writing, SDP offer/answer handling, and ICE candidate events.

## Source Filtering

Filtering is a first-class phase one requirement.

Default behavior:

- exclude current Tauri app windows;
- exclude invisible windows;
- exclude minimized/unavailable windows;
- exclude windows with no user-meaningful title when appropriate;
- exclude dimensions below a small threshold;
- exclude known macOS system UI such as Dock, menu bar, WindowServer/system UI surfaces;
- exclude known Windows system UI such as taskbar, Start menu, Shell tool windows, and toolbar windows.

The filter must support debug options that expose raw sources and filtered reasons. This avoids hiding useful data while tuning platform rules.

## WebRTC Loopback Design

Phase one uses local loopback only:

- native Rust WebRTC is the sender;
- WebView browser WebRTC is the receiver;
- signaling happens through Tauri commands and events inside the same app;
- no external signaling server is used;
- no STUN/TURN is required for phase one.

Recommended dependency direction is `webrtc` 0.17.x because it is the last Tokio async API line before the Sans-I/O transition. The design should isolate this dependency in `webrtc/` and `publisher/webrtc.rs` so a later migration does not affect capture backends.

Flow:

```text
Frontend start_capture
  -> ScreenCaptureState creates CaptureSession
  -> CaptureBackend starts platform capture
  -> CapturePipeline receives frames
  -> WebRtcLoopbackPublisher writes frames into native video track
  -> create_webrtc_offer returns native SDP offer
  -> WebView PeerConnection creates answer
  -> accept_webrtc_answer applies browser SDP answer
  -> ICE candidates pass through commands/events
  -> WebView renders remote track in <video>
```

## Error Model

Errors should be structured.

```ts
type CaptureErrorCode =
  | "permissionDenied"
  | "permissionNotDetermined"
  | "unsupportedPlatform"
  | "sourceNotFound"
  | "sourceUnavailable"
  | "captureStartFailed"
  | "captureRuntimeFailed"
  | "publisherUnsupported"
  | "webrtcNegotiationFailed"
  | "webrtcTrackFailed"
  | "invalidSession"
  | "internal";
```

Each error includes:

- `code`;
- `message`;
- `recoverable`;
- optional `details`.

Commands return structured errors. Runtime failures are also emitted through `screen-capture://error`.

## Example App Acceptance

The generated example app should be replaced with a real capture console.

Required UI:

- source list with Displays and Windows sections;
- source metadata: name, app name, dimensions, optional thumbnail;
- controls: refresh, debug raw sources, include current app, include system UI;
- capture controls: start, pause, resume, stop;
- WebRTC preview area using `<video>`;
- session details: session id and status;
- stats: fps, frames captured, frames published, dropped frames, bitrate if available;
- last error display.

Acceptance path:

1. Start the example Tauri app.
2. Grant macOS Screen Recording permission if needed.
3. Refresh sources.
4. Confirm system UI and current app windows are hidden by default.
5. Enable debug mode and confirm raw/filtered comparison is available.
6. Select a display and start capture.
7. Confirm the selected display renders in the preview video.
8. Stop capture and confirm resources are released.
9. Select a normal application window and start capture.
10. Confirm the selected window renders in the preview video.

## Testing Strategy

- Unit test `dummy` backend in CI-friendly environments.
- Unit test source filtering with constructed macOS and Windows source records.
- Unit test session state transitions: start, pause, resume, stop, failure.
- Unit test null publisher lifecycle and stats.
- Unit test error mapping to structured error codes.
- Keep macOS ScreenCaptureKit and WebRTC loopback as manual/integration validation through the example app because CI may not have screen recording permission.
- Run `cargo check` and `cargo test` as baseline verification.
- Run the example app for end-to-end validation.

## Phase One Deliverables

- macOS display and window enumeration.
- macOS display and window capture.
- Source filtering for system UI and current app windows.
- Capture lifecycle commands using `capture` naming.
- Structured models and errors.
- Capture pipeline with stats and backpressure accounting.
- Native WebRTC loopback publisher.
- Example app local WebRTC preview.
- Windows backend placeholder and capability reporting.
- Agora publisher placeholder and configuration model.

## Future Work

- Windows Graphics Capture backend.
- Agora SDK publisher.
- Production WebRTC signaling.
- Cross-device P2P and TURN/STUN support.
- Audio capture if product requirements include screen audio.
- Optional recording/file sink.
