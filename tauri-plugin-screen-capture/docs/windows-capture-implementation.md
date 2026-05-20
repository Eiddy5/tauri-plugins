# Windows Screen Capture Implementation

Date: 2026-05-20

## Goal

Add real Windows capture support without changing the plugin's public JavaScript API.

The frontend should continue to call the same commands:

- `listSources`
- `startCapture`
- `pauseCapture`
- `resumeCapture`
- `stopCapture`
- `createWebRtcOffer`
- `acceptWebRtcAnswer`
- `addWebRtcIceCandidate`

Platform-specific code stays behind `CaptureBackend`. The capture output remains `VideoFrame`, then flows through `CapturePipeline` into one or more `CapturePublisher` implementations.

## Decision

Use **Windows Graphics Capture** as the primary Windows implementation.

Reasons:

- It is the modern Microsoft API for capturing displays and application windows.
- It supports display and window capture, matching the plugin's existing `display` / `window` source model.
- It produces Direct3D frames, which are a good fit for a low-copy capture path and later native Agora integration.
- It keeps the public API aligned with macOS ScreenCaptureKit: source enumeration, capture lifecycle, and publisher fan-out stay unchanged.

Do not implement DXGI Desktop Duplication as the first Windows path. DXGI is still useful as a future display-only fallback, but it does not match the window-capture UX as cleanly.

Do not use GDI/BitBlt for realtime video. It is acceptable only for debugging or screenshots.

## Current Code State

The repository already has the right high-level seam:

- `src/capture/mod.rs` defines `CaptureBackend`.
- `src/capture/macos.rs` implements the backend through ScreenCaptureKit.
- `src/capture/windows.rs` currently aliases `DummyCaptureBackend`.
- `src/platform/windows/*` is placeholder code.
- `src/pipeline/frame.rs` uses `Arc<[u8]>` for shared frame data.
- `src/publisher/composite.rs` can fan out one shared frame to multiple publishers.

Windows implementation should replace the dummy alias with a real backend:

```rust
#[derive(Debug, Default)]
pub struct WindowsCaptureBackend;
```

and implement:

```rust
#[async_trait]
impl CaptureBackend for WindowsCaptureBackend {
    async fn check_permission(&self) -> Result<PermissionStatus>;
    async fn request_permission(&self) -> Result<PermissionStatus>;
    async fn list_sources(&self, options: ListSourcesOptions) -> Result<Vec<CaptureSource>>;
    async fn start_capture(
        &self,
        options: StartCaptureOptions,
        consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>>;
}
```

`ScreenCaptureState::default()` should select this backend on Windows:

```rust
#[cfg(target_os = "windows")]
let backend: Arc<dyn CaptureBackend> = Arc::new(crate::capture::WindowsCaptureBackend);
```

The frontend still receives the same source/session/stat models.

## Target Data Flow

```text
Windows source enumeration
  -> CaptureSource { id, kind, name, width, height, thumbnail_base64 }

startCapture(source_id)
  -> resolve source id to HWND or HMONITOR
  -> create GraphicsCaptureItem
  -> create D3D11 device
  -> create Direct3D11CaptureFramePool
  -> create GraphicsCaptureSession
  -> StartCapture()

FrameArrived
  -> TryGetNextFrame()
  -> Direct3D surface / texture
  -> convert or copy to VideoFrame { data: Arc<[u8]>, pixel_format: Bgra or Nv12 }
  -> FrameConsumer::push_frame()
  -> CapturePipeline
  -> CompositePublisher
  -> WebRtcPublisher and/or AgoraPublisher
```

The first implementation may copy GPU frames into CPU BGRA memory to match the current macOS path. A later optimization can pass GPU textures or NV12 buffers directly to an encoder/publisher.

## Source Enumeration

### Displays

Use Win32 monitor enumeration:

- `EnumDisplayMonitors`
- `GetMonitorInfoW`
- `MonitorFromWindow` where useful

Return `CaptureSourceKind::Display`.

Recommended source id:

```text
display:<index>
```

Keep a runtime mapping from index to `HMONITOR` during listing/start. Do not treat `HMONITOR` as a stable persistent id across process launches.

### Windows

Use Win32 window enumeration:

- `EnumWindows`
- `IsWindowVisible`
- `GetWindowTextW`
- `GetClassNameW`
- `GetWindowThreadProcessId`
- `DwmGetWindowAttribute` or `GetWindowRect` for bounds

Return `CaptureSourceKind::Window`.

Recommended source id:

```text
window:<hwnd>
```

Before capture, validate the handle again:

- window still exists;
- window is visible;
- window is not the current Tauri app window unless `includeCurrentApp` is enabled;
- window is not filtered system UI unless `includeSystemUi` is enabled.

Apply `platform/windows/window_filter.rs` to filter shell/taskbar/start-menu windows.

## Creating a Capture Item

For a selected source, create a `GraphicsCaptureItem` without changing the frontend selection flow.

For windows:

```text
IGraphicsCaptureItemInterop::CreateForWindow(HWND)
```

For displays:

```text
IGraphicsCaptureItemInterop::CreateForMonitor(HMONITOR)
```

This keeps the plugin's own source picker UI, instead of using the system picker as the primary UX.

## Capture Runtime

Create these runtime objects per active capture:

- D3D11 device
- WinRT `IDirect3DDevice`
- `Direct3D11CaptureFramePool`
- `GraphicsCaptureSession`
- frame-arrived event registration
- stop signal / join handle if a worker thread is used

Use `Direct3D11CaptureFramePool::CreateFreeThreaded` if the implementation does not want to depend on a UI dispatcher queue. The frame pool raises `FrameArrived`; each event should drain frames with `TryGetNextFrame`.

`RunningCapture` should own the session and event registration. `stop()` must close the session, close the frame pool, remove event handlers, and release D3D resources.

`pause()` and `resume()` can initially be implemented as a local paused flag:

- paused: keep the session alive, drop incoming frames before pushing to the pipeline;
- resumed: continue pushing frames.

If this wastes too much GPU work, a later change can stop and recreate the session.

## Pixel Format Strategy

Phase one should output BGRA CPU frames:

```rust
VideoFrame {
    width,
    height,
    pixel_format: PixelFormat::Bgra,
    timestamp_ns,
    data: Arc<[u8]>,
}
```

This matches the current WebRTC H264 encoder path.

Implementation detail:

1. Capture frame arrives as a Direct3D surface.
2. Copy the texture to a CPU-readable staging texture if needed.
3. Map the staging texture.
4. Copy row-by-row into tightly packed BGRA.
5. Store the buffer as `Arc<[u8]>`.

Do not pass platform pointers through the public API. The public Rust pipeline receives only `VideoFrame`.

Later optimization:

- add NV12 output for encoders that prefer it;
- add texture-backed publisher paths for Agora/native encoders;
- avoid CPU readback when publisher can consume D3D11 textures directly.

## Frame Rate and Size

`StartCaptureOptions` already contains:

- `fps`
- `width`
- `height`
- `capture_cursor`

Windows Graphics Capture emits frames when content changes and when the compositor produces updates. The backend should enforce the requested FPS in the capture runtime before pushing frames to the pipeline:

```text
if now - last_pushed < target_frame_interval {
    drop frame
}
```

Scaling policy:

- first version may capture native size and resize before encoding only if needed;
- if CPU resizing is added, keep it outside the platform capture event callback;
- prefer encoder/publisher scaling later to avoid extra CPU work.

## Cursor and Border

Map `capture_cursor` to `GraphicsCaptureSession::IsCursorCaptureEnabled` where supported.

Windows may show a capture border around the captured item. Treat this as platform behavior. Do not rely on disabling it in the first implementation.

## Permissions and Capability Reporting

Windows is different from macOS Screen Recording permission.

Capability checks should use:

- `GraphicsCaptureSession::IsSupported()`
- `GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Programmatic)` where programmatic access is needed and available

Recommended mapping:

- unsupported OS/API: `PermissionStatus::Unsupported`
- capture supported and programmatic creation allowed: `PermissionStatus::Granted`
- user/system denies access: `PermissionStatus::Denied`
- unknown request state: `PermissionStatus::NotDetermined`

`Capabilities` should report Windows support when the real backend is compiled:

```rust
supports_display_capture: cfg!(any(target_os = "macos", target_os = "windows")),
supports_window_capture: cfg!(any(target_os = "macos", target_os = "windows")),
supports_thumbnails: cfg!(any(target_os = "macos", target_os = "windows")),
supports_cursor_capture: cfg!(any(target_os = "macos", target_os = "windows")),
supports_webrtc: true,
```

## Thumbnails

The UI expects a temporary preview image for each display/window source.

For Windows phase one:

- displays: take one small capture frame or use a screenshot helper from the same WGC path;
- windows: capture a small frame from the selected HWND or use a DWM thumbnail path if WGC startup cost is too high.

Return PNG base64 in `thumbnail_base64`, matching the existing frontend.

Keep thumbnail generation bounded:

- limit max thumbnail width;
- use timeouts;
- avoid blocking source enumeration indefinitely on windows that cannot be captured.

## Error Handling

Map common Windows failures into existing plugin errors:

- source closed or HWND invalid: `CaptureErrorCode::SourceUnavailable`
- WGC unsupported: `CaptureErrorCode::UnsupportedPlatform`
- D3D device/frame-pool/session creation failure: `CaptureErrorCode::CaptureStartFailed`
- frame conversion failure: `CaptureErrorCode::CaptureRuntimeFailed`
- publisher/encoder failure: existing WebRTC error codes or publisher-specific errors later

Do not panic inside frame callbacks. Log and increment dropped-frame stats where possible.

## Dependencies

Add Windows-only dependencies under `target.'cfg(target_os = "windows")'.dependencies`.

Expected dependency:

```toml
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.62", features = [
  "Foundation",
  "Graphics",
  "Graphics_Capture",
  "Graphics_DirectX",
  "Graphics_DirectX_Direct3D11",
  "Win32_Foundation",
  "Win32_Graphics_Direct3D",
  "Win32_Graphics_Direct3D11",
  "Win32_Graphics_Dxgi",
  "Win32_Graphics_Dwm",
  "Win32_Graphics_Gdi",
  "Win32_System_WinRT",
  "Win32_System_WinRT_Graphics_Capture",
  "Win32_UI_WindowsAndMessaging"
] }
```

Exact feature names should be checked against the selected `windows` crate version before implementation.

## File Plan

Recommended files:

- `src/capture/windows.rs`
  - real `WindowsCaptureBackend`;
  - delegates to `platform::windows`.

- `src/platform/windows/sources.rs`
  - display/window enumeration;
  - current-app and system-UI filtering;
  - source id parsing.

- `src/platform/windows/graphics_capture.rs`
  - D3D device creation;
  - `GraphicsCaptureItem` creation;
  - frame pool/session lifecycle;
  - `RunningCapture` implementation.

- `src/platform/windows/permissions.rs`
  - `GraphicsCaptureSession::IsSupported`;
  - programmatic access request/check.

- `src/platform/windows/frame.rs`
  - D3D texture to tightly packed `VideoFrame` conversion.

- `src/platform/windows/thumbnails.rs`
  - small PNG previews for displays/windows.

Keep `src/commands.rs`, `guest-js/index.ts`, and the example frontend unchanged unless the data model itself changes.

## Testing Strategy

Unit tests that can run on any platform:

- source id parsing;
- window/system UI filter rules;
- options mapping;
- capability mapping for mocked backend states;
- frame-rate throttling logic;
- `CompositePublisher` fan-out behavior.

Windows-only integration tests/manual validation:

- list displays;
- list normal app windows;
- exclude current app by default;
- start display capture;
- start window capture;
- pause/resume/stop;
- close captured window while streaming;
- capture at 1080p, 2K, and 4K;
- verify local `<video>` preview in the Tauri example;
- verify `Captured / Published / Dropped` stats move as expected.

Performance validation:

- target 2K 30fps first;
- track CPU usage during CPU readback;
- track dropped frames;
- compare native-size capture vs scaled capture;
- confirm UI remains responsive while source list thumbnails load.

## Implementation Order

1. Replace the Windows dummy backend with `WindowsCaptureBackend`.
2. Add Windows capability and permission checks.
3. Implement display/window enumeration without thumbnails.
4. Implement `GraphicsCaptureItem` creation for HWND/HMONITOR.
5. Implement frame pool/session lifecycle and `RunningCapture`.
6. Convert D3D frames to BGRA `VideoFrame`.
7. Connect frames to the existing pipeline and WebRTC preview.
8. Add thumbnail generation.
9. Add throttling and dropped-frame diagnostics.
10. Optimize with NV12/texture paths only after the BGRA path is correct.

## Open Questions

- Whether native Agora will consume CPU frames or D3D11 textures. This affects whether NV12/texture output should be prioritized.
- Whether the first Windows release must support borderless capture. If yes, extra Windows version and consent handling is needed.
- Whether thumbnails should be generated eagerly during `listSources` or lazily after the user switches tabs.

## References

- Microsoft: Windows Graphics Capture overview: https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture
- Microsoft: `Direct3D11CaptureFramePool`: https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool
- Microsoft: `GraphicsCaptureSession`: https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession
- Microsoft: `GraphicsCaptureAccess`: https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscaptureaccess
- Microsoft: `IGraphicsCaptureItemInterop`: https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nn-windows-graphics-capture-interop-igraphicscaptureiteminterop
- windows-rs: `Direct3D11CaptureFramePool`: https://microsoft.github.io/windows-docs-rs/doc/windows/Graphics/Capture/struct.Direct3D11CaptureFramePool.html
- windows-rs: `IGraphicsCaptureItemInterop`: https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/System/WinRT/Graphics/Capture/struct.IGraphicsCaptureItemInterop.html
