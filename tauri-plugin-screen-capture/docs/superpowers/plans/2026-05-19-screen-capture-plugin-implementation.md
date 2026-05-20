# Screen Capture Plugin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a macOS-first Tauri screen capture plugin that enumerates displays/windows, filters non-shareable system UI, captures a selected source, publishes it through native WebRTC loopback, and renders it in the example app while preserving Agora extension points.

**Architecture:** The plugin uses a control plane of Tauri commands/events, a platform capture layer, a frame pipeline, and publisher adapters. macOS ScreenCaptureKit provides phase-one capture; native Rust WebRTC provides the loopback publisher; Windows and Agora are represented by capability-aware extension adapters.

**Tech Stack:** Rust 2021, Tauri 2.11, Tokio, Serde, `thiserror`, `anyhow`, `uuid`, `screencapturekit` for macOS capture, `webrtc` 0.17.x for native WebRTC, Svelte/Vite example app.

---

## Source References

- Design spec: `docs/superpowers/specs/2026-05-19-screen-capture-plugin-design.md`
- Apple ScreenCaptureKit docs: https://developer.apple.com/documentation/screencapturekit
- Rust ScreenCaptureKit crate docs: https://docs.rs/screencapturekit/latest/screencapturekit/
- Rust WebRTC crate docs: https://docs.rs/crate/webrtc/latest
- WebRTC-rs 0.17 feature-freeze note: https://webrtc.rs/blog/2026/01/31/webrtc-v0.17.0-feature-freeze-sansio-shift.html

## File Structure

Create or modify these files:

```text
Cargo.toml
build.rs
permissions/default.toml
guest-js/index.ts
src/lib.rs
src/commands.rs
src/models.rs
src/error.rs
src/state.rs
src/capture/mod.rs
src/capture/dummy.rs
src/capture/macos.rs
src/capture/windows.rs
src/sources/mod.rs
src/sources/filter.rs
src/sources/thumbnails.rs
src/pipeline/mod.rs
src/pipeline/frame.rs
src/pipeline/stats.rs
src/publisher/mod.rs
src/publisher/null.rs
src/publisher/agora.rs
src/publisher/webrtc.rs
src/webrtc/mod.rs
src/webrtc/signaling.rs
src/webrtc/track.rs
src/platform/mod.rs
src/platform/macos/mod.rs
src/platform/macos/permissions.rs
src/platform/macos/screencapturekit.rs
src/platform/macos/window_filter.rs
src/platform/windows/mod.rs
src/platform/windows/permissions.rs
src/platform/windows/graphics_capture.rs
src/platform/windows/window_filter.rs
tests/model_contract.rs
tests/source_filter.rs
tests/session_state.rs
tests/publisher_contract.rs
examples/tauri-app/src/App.svelte
examples/tauri-app/src/style.css
examples/tauri-app/src/lib/screenCapture.ts
examples/tauri-app/src-tauri/capabilities/default.json
```

Responsibilities:

- `src/models.rs`: public IPC contract types.
- `src/error.rs`: structured plugin error codes and Tauri serialization.
- `src/state.rs`: session registry and orchestration.
- `src/capture/*`: backend traits and platform-specific capture adapters.
- `src/sources/*`: normalized source records, filtering, and thumbnail boundaries.
- `src/pipeline/*`: frame routing, status, stats, and backpressure accounting.
- `src/publisher/*`: null, WebRTC loopback, and Agora extension publishers.
- `src/webrtc/*`: native PeerConnection, track writer, and signaling state.
- `src/platform/*`: OS-specific permissions, source enumeration, capture implementation, and source-filter hints.
- `guest-js/index.ts`: published TypeScript API wrappers.
- `examples/tauri-app/*`: real acceptance app for choosing and rendering capture sources.

## Implementation Notes

- Do not send raw video frames over Tauri IPC.
- Keep `commands.rs` thin: commands delegate to `ScreenCaptureState`.
- Keep platform-specific APIs behind `#[cfg(target_os = "...")]`.
- Keep `screencapturekit` and `webrtc` dependencies isolated to their modules.
- Commit after each task.
- Run `cargo fmt` before every commit that changes Rust code.

---

### Task 1: Commit Scaffold Baseline

**Files:**
- Modify: repository index only

- [ ] **Step 1: Inspect current untracked scaffold**

Run:

```bash
git status --short
```

Expected: the generated Tauri scaffold files are untracked, and the spec/plan docs are tracked or staged separately.

- [ ] **Step 2: Commit generated scaffold as baseline**

Run:

```bash
git add .gitignore Cargo.toml README.md build.rs examples guest-js package.json permissions rollup.config.js src tsconfig.json
git commit -m "chore: add generated screen capture plugin scaffold"
```

Expected: a commit containing only the generated plugin scaffold. If the implementation plan file is uncommitted, do not include it in this scaffold commit.

- [ ] **Step 3: Verify clean baseline except plan work**

Run:

```bash
git status --short
```

Expected: no untracked scaffold files remain.

---

### Task 2: Add Dependencies and Public Models

**Files:**
- Modify: `Cargo.toml`
- Replace: `src/models.rs`
- Modify: `src/error.rs`
- Test: `tests/model_contract.rs`

- [ ] **Step 1: Write model contract tests**

Create `tests/model_contract.rs`:

```rust
use tauri_plugin_screen_capture::*;

#[test]
fn list_sources_options_defaults_are_safe_for_user_ui() {
    let options = ListSourcesOptions::default();
    assert_eq!(options.kinds, vec![CaptureSourceKind::Display, CaptureSourceKind::Window]);
    assert!(!options.include_thumbnails);
    assert!(!options.include_current_app);
    assert!(!options.include_system_ui);
    assert!(!options.debug_raw_sources);
}

#[test]
fn start_capture_options_defaults_to_webrtc_loopback() {
    let options = StartCaptureOptions {
        source_id: "display:1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: None,
        width: None,
        height: None,
        capture_cursor: None,
        publisher: None,
    };

    assert_eq!(options.effective_fps(), 30);
    assert!(options.effective_capture_cursor());
    assert_eq!(options.effective_publisher(), CapturePublisherKind::WebRtcLoopback);
}

#[test]
fn capture_error_serializes_stable_code() {
    let error = CaptureErrorPayload {
        code: CaptureErrorCode::PermissionDenied,
        message: "Screen Recording permission is denied".to_string(),
        recoverable: true,
        details: None,
    };

    let json = serde_json::to_value(error).expect("serialize error payload");
    assert_eq!(json["code"], "permissionDenied");
    assert_eq!(json["recoverable"], true);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test model_contract
```

Expected: FAIL because the new public models are not defined.

- [ ] **Step 3: Add dependencies**

Modify `Cargo.toml`:

```toml
[dependencies]
tauri = { version = "2.11.2" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "2"
anyhow = "1"
async-trait = "0.1"
tokio = { version = "1", features = ["sync", "time", "macros", "rt-multi-thread"] }
uuid = { version = "1", features = ["v4", "serde"] }
tracing = "0.1"

[target.'cfg(target_os = "macos")'.dependencies]
screencapturekit = "3"

[dependencies.webrtc]
version = "0.17"
default-features = false

[dev-dependencies]
serde_json = "1.0"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
```

Keep the existing `[build-dependencies]` section.

- [ ] **Step 4: Replace public models**

Replace `src/models.rs` with:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureSourceKind {
    Display,
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionStatus {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CapturePublisherKind {
    Null,
    WebRtcLoopback,
    Agora,
}

impl Default for CapturePublisherKind {
    fn default() -> Self {
        Self::WebRtcLoopback
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureStatus {
    Idle,
    Starting,
    Capturing,
    Publishing,
    Paused,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PixelFormat {
    Bgra,
    Rgba,
    Nv12,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub platform: String,
    pub supports_display_capture: bool,
    pub supports_window_capture: bool,
    pub supports_thumbnails: bool,
    pub supports_cursor_capture: bool,
    pub supports_webrtc_loopback: bool,
    pub supports_agora: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSource {
    pub id: String,
    pub kind: CaptureSourceKind,
    pub name: String,
    pub title: Option<String>,
    pub app_name: Option<String>,
    pub pid: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
    pub thumbnail_base64: Option<String>,
    pub filtered_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSourcesOptions {
    pub kinds: Vec<CaptureSourceKind>,
    pub include_thumbnails: bool,
    pub include_current_app: bool,
    pub include_system_ui: bool,
    pub debug_raw_sources: bool,
}

impl Default for ListSourcesOptions {
    fn default() -> Self {
        Self {
            kinds: vec![CaptureSourceKind::Display, CaptureSourceKind::Window],
            include_thumbnails: false,
            include_current_app: false,
            include_system_ui: false,
            debug_raw_sources: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCaptureOptions {
    pub source_id: String,
    pub source_kind: CaptureSourceKind,
    pub fps: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub capture_cursor: Option<bool>,
    pub publisher: Option<CapturePublisherKind>,
}

impl StartCaptureOptions {
    pub fn effective_fps(&self) -> u32 {
        self.fps.unwrap_or(30).clamp(1, 60)
    }

    pub fn effective_capture_cursor(&self) -> bool {
        self.capture_cursor.unwrap_or(true)
    }

    pub fn effective_publisher(&self) -> CapturePublisherKind {
        self.publisher.unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSession {
    pub session_id: String,
    pub source_id: String,
    pub source_kind: CaptureSourceKind,
    pub status: CaptureStatus,
    pub publisher: CapturePublisherKind,
    pub started_at_ms: u128,
    pub last_error: Option<CaptureErrorPayload>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStats {
    pub frames_captured: u64,
    pub frames_published: u64,
    pub frames_dropped: u64,
    pub fps: f64,
    pub bitrate_kbps: u32,
    pub started: bool,
}

impl Default for CaptureStats {
    fn default() -> Self {
        Self {
            frames_captured: 0,
            frames_published: 0,
            frames_dropped: 0,
            fps: 0.0,
            bitrate_kbps: 0,
            started: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcOffer {
    pub sdp: String,
    pub sdp_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcAnswer {
    pub sdp: String,
    pub sdp_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcIceCandidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureErrorCode {
    PermissionDenied,
    PermissionNotDetermined,
    UnsupportedPlatform,
    SourceNotFound,
    SourceUnavailable,
    CaptureStartFailed,
    CaptureRuntimeFailed,
    PublisherUnsupported,
    WebRtcNegotiationFailed,
    WebRtcTrackFailed,
    InvalidSession,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureErrorPayload {
    pub code: CaptureErrorCode,
    pub message: String,
    pub recoverable: bool,
    pub details: Option<serde_json::Value>,
}
```

- [ ] **Step 5: Replace structured error type**

Replace `src/error.rs` with:

```rust
use serde::Serialize;
use thiserror::Error;

use crate::models::{CaptureErrorCode, CaptureErrorPayload};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{payload:?}")]
    Structured { payload: CaptureErrorPayload },
}

impl Error {
    pub fn new(code: CaptureErrorCode, message: impl Into<String>, recoverable: bool) -> Self {
        Self::Structured {
            payload: CaptureErrorPayload {
                code,
                message: message.into(),
                recoverable,
                details: None,
            },
        }
    }

    pub fn payload(&self) -> CaptureErrorPayload {
        match self {
            Self::Structured { payload } => payload.clone(),
        }
    }
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.payload().serialize(serializer)
    }
}
```

- [ ] **Step 6: Run model tests**

Run:

```bash
cargo test --test model_contract
```

Expected: PASS.

- [ ] **Step 7: Format and commit**

Run:

```bash
cargo fmt
git add Cargo.toml src/models.rs src/error.rs tests/model_contract.rs
git commit -m "feat: define screen capture IPC models"
```

Expected: commit succeeds.

---

### Task 3: Implement Source Filtering

**Files:**
- Create: `src/sources/mod.rs`
- Create: `src/sources/filter.rs`
- Create: `src/sources/thumbnails.rs`
- Modify: `src/lib.rs`
- Test: `tests/source_filter.rs`

- [ ] **Step 1: Write filtering tests**

Create `tests/source_filter.rs`:

```rust
use tauri_plugin_screen_capture::{
    sources::{filter_sources, SourceFilterOptions},
    CaptureSource, CaptureSourceKind,
};

fn window(id: &str, title: &str, app: &str, width: u32, height: u32, pid: u32) -> CaptureSource {
    CaptureSource {
        id: id.to_string(),
        kind: CaptureSourceKind::Window,
        name: title.to_string(),
        title: Some(title.to_string()),
        app_name: Some(app.to_string()),
        pid: Some(pid),
        width,
        height,
        scale_factor: 1.0,
        is_primary: false,
        thumbnail_base64: None,
        filtered_reason: None,
    }
}

#[test]
fn excludes_current_app_by_default() {
    let sources = vec![
        window("window:app", "Capture Preview", "Example", 1200, 800, 42),
        window("window:editor", "main.rs", "Code", 1200, 800, 84),
    ];

    let filtered = filter_sources(
        sources,
        SourceFilterOptions {
            current_pid: Some(42),
            include_current_app: false,
            include_system_ui: false,
            debug_raw_sources: false,
        },
    );

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "window:editor");
}

#[test]
fn excludes_known_system_ui_by_default() {
    let sources = vec![
        window("window:dock", "Dock", "Dock", 1728, 80, 10),
        window("window:taskbar", "Taskbar", "ShellExperienceHost", 1920, 48, 11),
        window("window:browser", "Planning", "Browser", 1280, 900, 12),
    ];

    let filtered = filter_sources(
        sources,
        SourceFilterOptions {
            current_pid: None,
            include_current_app: false,
            include_system_ui: false,
            debug_raw_sources: false,
        },
    );

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "window:browser");
}

#[test]
fn debug_mode_keeps_filtered_sources_with_reasons() {
    let sources = vec![window("window:dock", "Dock", "Dock", 1728, 80, 10)];

    let filtered = filter_sources(
        sources,
        SourceFilterOptions {
            current_pid: None,
            include_current_app: false,
            include_system_ui: false,
            debug_raw_sources: true,
        },
    );

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].filtered_reason.as_deref(), Some("systemUi"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test source_filter
```

Expected: FAIL because `sources` module is missing.

- [ ] **Step 3: Add sources module exports**

Modify `src/lib.rs` to include:

```rust
pub mod sources;
```

Create `src/sources/mod.rs`:

```rust
pub mod filter;
pub mod thumbnails;

pub use filter::{filter_sources, SourceFilterOptions};
```

Create `src/sources/thumbnails.rs`:

```rust
pub fn encode_png_base64(_rgba: &[u8], _width: u32, _height: u32) -> Option<String> {
    None
}
```

- [ ] **Step 4: Implement source filtering**

Create `src/sources/filter.rs`:

```rust
use crate::models::{CaptureSource, CaptureSourceKind};

#[derive(Debug, Clone, Copy)]
pub struct SourceFilterOptions {
    pub current_pid: Option<u32>,
    pub include_current_app: bool,
    pub include_system_ui: bool,
    pub debug_raw_sources: bool,
}

pub fn filter_sources(
    sources: Vec<CaptureSource>,
    options: SourceFilterOptions,
) -> Vec<CaptureSource> {
    sources
        .into_iter()
        .filter_map(|mut source| {
            let reason = filtered_reason(&source, options);
            match reason {
                Some(reason) if options.debug_raw_sources => {
                    source.filtered_reason = Some(reason.to_string());
                    Some(source)
                }
                Some(_) => None,
                None => Some(source),
            }
        })
        .collect()
}

fn filtered_reason(source: &CaptureSource, options: SourceFilterOptions) -> Option<&'static str> {
    if source.kind == CaptureSourceKind::Window {
        if !options.include_current_app
            && options.current_pid.is_some()
            && source.pid == options.current_pid
        {
            return Some("currentApp");
        }

        if source.width < 96 || source.height < 64 {
            return Some("tooSmall");
        }

        if !options.include_system_ui && is_system_ui(source) {
            return Some("systemUi");
        }
    }

    None
}

fn is_system_ui(source: &CaptureSource) -> bool {
    let title = source.title.as_deref().unwrap_or_default().to_ascii_lowercase();
    let app = source.app_name.as_deref().unwrap_or_default().to_ascii_lowercase();

    matches!(
        app.as_str(),
        "dock"
            | "windowserver"
            | "systemuiserver"
            | "shellexperiencehost"
            | "explorer"
            | "startmenuexperiencehost"
    ) || matches!(
        title.as_str(),
        "dock" | "menu bar" | "taskbar" | "start" | "shell_traywnd"
    )
}
```

- [ ] **Step 5: Run filtering tests**

Run:

```bash
cargo test --test source_filter
```

Expected: PASS.

- [ ] **Step 6: Run all tests and commit**

Run:

```bash
cargo test
cargo fmt
git add src/lib.rs src/sources tests/source_filter.rs
git commit -m "feat: add capture source filtering"
```

Expected: tests pass and commit succeeds.

---

### Task 4: Add Capture Backend Traits and Dummy Backend

**Files:**
- Create: `src/capture/mod.rs`
- Create: `src/capture/dummy.rs`
- Create: `src/capture/macos.rs`
- Create: `src/capture/windows.rs`
- Create: `src/pipeline/frame.rs`
- Modify: `src/lib.rs`
- Test: `tests/session_state.rs`

- [ ] **Step 1: Write backend contract test**

Create `tests/session_state.rs`:

```rust
use tauri_plugin_screen_capture::{
    capture::{CaptureBackend, DummyCaptureBackend},
    CaptureSourceKind, ListSourcesOptions, PermissionStatus,
};

#[tokio::test]
async fn dummy_backend_lists_display_and_window() {
    let backend = DummyCaptureBackend::default();

    let sources = backend
        .list_sources(ListSourcesOptions::default())
        .await
        .expect("dummy sources");

    assert!(sources.iter().any(|source| source.kind == CaptureSourceKind::Display));
    assert!(sources.iter().any(|source| source.kind == CaptureSourceKind::Window));
}

#[tokio::test]
async fn dummy_backend_permission_is_granted() {
    let backend = DummyCaptureBackend::default();
    assert_eq!(
        backend.check_permission().await.expect("permission"),
        PermissionStatus::Granted
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test session_state
```

Expected: FAIL because `capture` module is missing.

- [ ] **Step 3: Add frame model**

Create `src/pipeline/frame.rs`:

```rust
use crate::models::PixelFormat;

#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub timestamp_ns: u64,
    pub data: Vec<u8>,
}
```

- [ ] **Step 4: Add capture trait and exports**

Create `src/capture/mod.rs`:

```rust
mod dummy;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use async_trait::async_trait;

use crate::{
    models::{CaptureSource, ListSourcesOptions, PermissionStatus, StartCaptureOptions},
    pipeline::frame::VideoFrame,
    Result,
};

pub use dummy::DummyCaptureBackend;
#[cfg(target_os = "macos")]
pub use macos::MacOsCaptureBackend;
#[cfg(target_os = "windows")]
pub use windows::WindowsCaptureBackend;

#[async_trait]
pub trait FrameConsumer: Send + Sync {
    async fn push_frame(&self, frame: VideoFrame) -> Result<()>;
}

#[async_trait]
pub trait CaptureBackend: Send + Sync {
    async fn check_permission(&self) -> Result<PermissionStatus>;
    async fn request_permission(&self) -> Result<PermissionStatus>;
    async fn list_sources(&self, options: ListSourcesOptions) -> Result<Vec<CaptureSource>>;
    async fn start_capture(
        &self,
        options: StartCaptureOptions,
        consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>>;
}

#[async_trait]
pub trait RunningCapture: Send + Sync {
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
}
```

Modify `src/lib.rs`:

```rust
pub mod capture;
pub mod pipeline;
```

Create `src/pipeline/mod.rs`:

```rust
pub mod frame;
pub mod stats;
```

Create `src/pipeline/stats.rs`:

```rust
use crate::models::CaptureStats;

pub fn empty_stats() -> CaptureStats {
    CaptureStats::default()
}
```

- [ ] **Step 5: Implement dummy backend**

Create `src/capture/dummy.rs`:

```rust
use async_trait::async_trait;

use crate::{
    capture::{CaptureBackend, FrameConsumer, RunningCapture},
    error::Error,
    models::{
        CaptureErrorCode, CaptureSource, CaptureSourceKind, ListSourcesOptions, PermissionStatus,
        StartCaptureOptions,
    },
    Result,
};

#[derive(Debug, Default)]
pub struct DummyCaptureBackend;

#[async_trait]
impl CaptureBackend for DummyCaptureBackend {
    async fn check_permission(&self) -> Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    async fn request_permission(&self) -> Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    async fn list_sources(&self, options: ListSourcesOptions) -> Result<Vec<CaptureSource>> {
        let mut sources = Vec::new();
        if options.kinds.contains(&CaptureSourceKind::Display) {
            sources.push(CaptureSource {
                id: "dummy-display-1".to_string(),
                kind: CaptureSourceKind::Display,
                name: "Dummy Display".to_string(),
                title: Some("Dummy Display".to_string()),
                app_name: None,
                pid: None,
                width: 1280,
                height: 720,
                scale_factor: 1.0,
                is_primary: true,
                thumbnail_base64: None,
                filtered_reason: None,
            });
        }
        if options.kinds.contains(&CaptureSourceKind::Window) {
            sources.push(CaptureSource {
                id: "dummy-window-1".to_string(),
                kind: CaptureSourceKind::Window,
                name: "Dummy Window".to_string(),
                title: Some("Dummy Window".to_string()),
                app_name: Some("Screen Capture Test Host".to_string()),
                pid: Some(std::process::id()),
                width: 800,
                height: 600,
                scale_factor: 1.0,
                is_primary: false,
                thumbnail_base64: None,
                filtered_reason: None,
            });
        }
        Ok(sources)
    }

    async fn start_capture(
        &self,
        _options: StartCaptureOptions,
        _consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>> {
        Ok(Box::new(DummyRunningCapture))
    }
}

#[derive(Debug)]
struct DummyRunningCapture;

#[async_trait]
impl RunningCapture for DummyRunningCapture {
    async fn pause(&self) -> Result<()> {
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }
}

pub fn unsupported_backend_error() -> Error {
    Error::new(
        CaptureErrorCode::UnsupportedPlatform,
        "Screen capture is not implemented on this platform",
        false,
    )
}
```

- [ ] **Step 6: Add platform fallback backend aliases**

Create `src/capture/macos.rs`:

```rust
pub type MacOsCaptureBackend = crate::capture::DummyCaptureBackend;
```

Create `src/capture/windows.rs`:

```rust
pub type WindowsCaptureBackend = crate::capture::DummyCaptureBackend;
```

- [ ] **Step 7: Run tests and commit**

Run:

```bash
cargo test --test session_state
cargo test
cargo fmt
git add src/capture src/pipeline src/lib.rs tests/session_state.rs
git commit -m "feat: add capture backend contract"
```

Expected: tests pass and commit succeeds.

---

### Task 5: Add Publisher Traits, Null Publisher, and Agora Extension Adapter

**Files:**
- Create: `src/publisher/mod.rs`
- Create: `src/publisher/null.rs`
- Create: `src/publisher/agora.rs`
- Create: `src/publisher/webrtc.rs`
- Modify: `src/lib.rs`
- Test: `tests/publisher_contract.rs`

- [ ] **Step 1: Write publisher contract tests**

Create `tests/publisher_contract.rs`:

```rust
use tauri_plugin_screen_capture::{
    pipeline::frame::VideoFrame,
    publisher::{AgoraPublisher, CapturePublisher, NullPublisher},
    CaptureErrorCode, CaptureStats, PixelFormat, StartCaptureOptions, CaptureSourceKind,
};

fn options() -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "dummy-display-1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(1280),
        height: Some(720),
        capture_cursor: Some(true),
        publisher: None,
    }
}

#[tokio::test]
async fn null_publisher_counts_published_frames() {
    let publisher = NullPublisher::default();
    publisher.start(options()).await.expect("start");
    publisher
        .push_frame(VideoFrame {
            width: 1280,
            height: 720,
            pixel_format: PixelFormat::Rgba,
            timestamp_ns: 1,
            data: vec![0; 4],
        })
        .await
        .expect("push frame");

    let stats: CaptureStats = publisher.stats().await.expect("stats");
    assert_eq!(stats.frames_published, 1);
    assert!(stats.started);
}

#[tokio::test]
async fn agora_publisher_returns_unsupported() {
    let publisher = AgoraPublisher::default();
    let err = publisher.start(options()).await.expect_err("unsupported");
    assert_eq!(err.payload().code, CaptureErrorCode::PublisherUnsupported);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test publisher_contract
```

Expected: FAIL because `publisher` module is missing.

- [ ] **Step 3: Add publisher trait**

Create `src/publisher/mod.rs`:

```rust
mod agora;
mod null;
mod webrtc;

use async_trait::async_trait;

use crate::{models::{CaptureStats, StartCaptureOptions}, pipeline::frame::VideoFrame, Result};

pub use agora::AgoraPublisher;
pub use null::NullPublisher;
pub use webrtc::WebRtcLoopbackPublisher;

#[async_trait]
pub trait CapturePublisher: Send + Sync {
    async fn start(&self, options: StartCaptureOptions) -> Result<()>;
    async fn push_frame(&self, frame: VideoFrame) -> Result<()>;
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn stats(&self) -> Result<CaptureStats>;
}
```

Modify `src/lib.rs`:

```rust
pub mod publisher;
```

- [ ] **Step 4: Add null publisher**

Create `src/publisher/null.rs`:

```rust
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{
    error::Error,
    models::{CaptureErrorCode, CaptureStats, StartCaptureOptions},
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    Result,
};

#[derive(Debug, Default)]
pub struct NullPublisher {
    stats: Mutex<CaptureStats>,
}

#[async_trait]
impl CapturePublisher for NullPublisher {
    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        let mut stats = self.stats.lock().await;
        *stats = CaptureStats {
            started: true,
            ..CaptureStats::default()
        };
        Ok(())
    }

    async fn push_frame(&self, _frame: VideoFrame) -> Result<()> {
        let mut stats = self.stats.lock().await;
        if !stats.started {
            return Err(Error::new(
                CaptureErrorCode::CaptureRuntimeFailed,
                "publisher is not started",
                true,
            ));
        }
        stats.frames_published += 1;
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        self.stats.lock().await.started = false;
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        self.stats.lock().await.started = true;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.stats.lock().await.started = false;
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(self.stats.lock().await.clone())
    }
}
```

- [ ] **Step 5: Add Agora extension adapter**

Create `src/publisher/agora.rs`:

```rust
use async_trait::async_trait;

use crate::{
    error::Error,
    models::{CaptureErrorCode, CaptureStats, StartCaptureOptions},
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    Result,
};

#[derive(Debug, Default)]
pub struct AgoraPublisher;

#[async_trait]
impl CapturePublisher for AgoraPublisher {
    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        Err(Error::new(
            CaptureErrorCode::PublisherUnsupported,
            "Agora publisher is not linked in this build",
            false,
        ))
    }

    async fn push_frame(&self, _frame: VideoFrame) -> Result<()> {
        self.start(StartCaptureOptions {
            source_id: String::new(),
            source_kind: crate::models::CaptureSourceKind::Display,
            fps: None,
            width: None,
            height: None,
            capture_cursor: None,
            publisher: None,
        })
        .await
    }

    async fn pause(&self) -> Result<()> {
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Err(Error::new(
            CaptureErrorCode::PublisherUnsupported,
            "Agora publisher is not linked in this build",
            false,
        ))
    }
}
```

- [ ] **Step 6: Add initial WebRTC publisher alias**

Create `src/publisher/webrtc.rs`:

```rust
pub type WebRtcLoopbackPublisher = crate::publisher::NullPublisher;
```

- [ ] **Step 7: Run publisher tests and commit**

Run:

```bash
cargo test --test publisher_contract
cargo test
cargo fmt
git add src/publisher src/lib.rs tests/publisher_contract.rs
git commit -m "feat: add capture publisher contract"
```

Expected: tests pass and commit succeeds.

---

### Task 6: Implement Session State and Capture Commands

**Files:**
- Create: `src/state.rs`
- Replace: `src/commands.rs`
- Modify: `src/lib.rs`
- Modify: `guest-js/index.ts`
- Modify: `permissions/default.toml`
- Test: `tests/session_state.rs`

- [ ] **Step 1: Extend session state tests**

Append to `tests/session_state.rs`:

```rust
use tauri_plugin_screen_capture::{
    CapturePublisherKind, CaptureStatus, ScreenCaptureState, StartCaptureOptions,
};

fn start_options() -> StartCaptureOptions {
    StartCaptureOptions {
        source_id: "dummy-display-1".to_string(),
        source_kind: CaptureSourceKind::Display,
        fps: Some(30),
        width: Some(1280),
        height: Some(720),
        capture_cursor: Some(true),
        publisher: Some(CapturePublisherKind::Null),
    }
}

#[tokio::test]
async fn state_tracks_capture_lifecycle() {
    let state = ScreenCaptureState::default();
    let session = state.start_capture(start_options()).await.expect("start");
    assert_eq!(session.status, CaptureStatus::Publishing);

    state.pause_capture(&session.session_id).await.expect("pause");
    assert_eq!(
        state.get_capture_session(&session.session_id).await.unwrap().status,
        CaptureStatus::Paused
    );

    state.resume_capture(&session.session_id).await.expect("resume");
    assert_eq!(
        state.get_capture_session(&session.session_id).await.unwrap().status,
        CaptureStatus::Publishing
    );

    state.stop_capture(&session.session_id).await.expect("stop");
    assert!(state.get_capture_session(&session.session_id).await.is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test session_state
```

Expected: FAIL because `ScreenCaptureState` lifecycle APIs are missing.

- [ ] **Step 3: Add state implementation**

Create `src/state.rs` with:

```rust
use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    capture::{CaptureBackend, DummyCaptureBackend},
    error::Error,
    models::{
        Capabilities, CaptureErrorCode, CapturePublisherKind, CaptureSession, CaptureStats,
        CaptureStatus, ListSourcesOptions, PermissionStatus, StartCaptureOptions,
    },
    publisher::{AgoraPublisher, CapturePublisher, NullPublisher, WebRtcLoopbackPublisher},
    Result,
};

struct SessionRecord {
    session: CaptureSession,
    publisher: Arc<dyn CapturePublisher>,
}

pub struct ScreenCaptureState {
    backend: Arc<dyn CaptureBackend>,
    sessions: Mutex<HashMap<String, SessionRecord>>,
}

impl Default for ScreenCaptureState {
    fn default() -> Self {
        Self {
            backend: Arc::new(DummyCaptureBackend::default()),
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

impl ScreenCaptureState {
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            platform: std::env::consts::OS.to_string(),
            supports_display_capture: cfg!(target_os = "macos"),
            supports_window_capture: cfg!(target_os = "macos"),
            supports_thumbnails: cfg!(target_os = "macos"),
            supports_cursor_capture: cfg!(target_os = "macos"),
            supports_webrtc_loopback: true,
            supports_agora: false,
        }
    }

    pub async fn check_permission(&self) -> Result<PermissionStatus> {
        self.backend.check_permission().await
    }

    pub async fn request_permission(&self) -> Result<PermissionStatus> {
        self.backend.request_permission().await
    }

    pub async fn list_sources(
        &self,
        options: ListSourcesOptions,
    ) -> Result<Vec<crate::models::CaptureSource>> {
        self.backend.list_sources(options).await
    }

    pub async fn start_capture(&self, options: StartCaptureOptions) -> Result<CaptureSession> {
        let publisher: Arc<dyn CapturePublisher> = match options.effective_publisher() {
            CapturePublisherKind::Null => Arc::new(NullPublisher::default()),
            CapturePublisherKind::WebRtcLoopback => Arc::new(WebRtcLoopbackPublisher::default()),
            CapturePublisherKind::Agora => Arc::new(AgoraPublisher::default()),
        };

        publisher.start(options.clone()).await?;

        let session = CaptureSession {
            session_id: Uuid::new_v4().to_string(),
            source_id: options.source_id,
            source_kind: options.source_kind,
            status: CaptureStatus::Publishing,
            publisher: options.effective_publisher(),
            started_at_ms: now_ms(),
            last_error: None,
        };

        self.sessions.lock().await.insert(
            session.session_id.clone(),
            SessionRecord {
                session: session.clone(),
                publisher,
            },
        );
        Ok(session)
    }

    pub async fn pause_capture(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or_else(|| invalid_session_error())?;
        record.publisher.pause().await?;
        record.session.status = CaptureStatus::Paused;
        Ok(())
    }

    pub async fn resume_capture(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or_else(|| invalid_session_error())?;
        record.publisher.resume().await?;
        record.session.status = CaptureStatus::Publishing;
        Ok(())
    }

    pub async fn stop_capture(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let record = sessions.remove(session_id).ok_or_else(invalid_session_error)?;
        record.publisher.stop().await
    }

    pub async fn get_capture_session(&self, session_id: &str) -> Result<CaptureSession> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .map(|record| record.session.clone())
            .ok_or_else(invalid_session_error)
    }

    pub async fn get_capture_stats(&self, session_id: &str) -> Result<CaptureStats> {
        let sessions = self.sessions.lock().await;
        let record = sessions.get(session_id).ok_or_else(invalid_session_error)?;
        record.publisher.stats().await
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn invalid_session_error() -> Error {
    Error::new(CaptureErrorCode::InvalidSession, "capture session was not found", true)
}
```

- [ ] **Step 4: Wire plugin state and commands**

Replace `src/commands.rs` with:

```rust
use tauri::{command, State};

use crate::{
    models::{
        Capabilities, CaptureSession, CaptureStats, ListSourcesOptions, PermissionStatus,
        StartCaptureOptions,
    },
    state::ScreenCaptureState,
    Result,
};

#[command]
pub async fn get_capabilities(state: State<'_, ScreenCaptureState>) -> Result<Capabilities> {
    Ok(state.capabilities())
}

#[command]
pub async fn check_permission(state: State<'_, ScreenCaptureState>) -> Result<PermissionStatus> {
    state.check_permission().await
}

#[command]
pub async fn request_permission(state: State<'_, ScreenCaptureState>) -> Result<PermissionStatus> {
    state.request_permission().await
}

#[command]
pub async fn list_sources(
    state: State<'_, ScreenCaptureState>,
    options: Option<ListSourcesOptions>,
) -> Result<Vec<crate::models::CaptureSource>> {
    state.list_sources(options.unwrap_or_default()).await
}

#[command]
pub async fn start_capture(
    state: State<'_, ScreenCaptureState>,
    options: StartCaptureOptions,
) -> Result<CaptureSession> {
    state.start_capture(options).await
}

#[command]
pub async fn pause_capture(state: State<'_, ScreenCaptureState>, session_id: String) -> Result<()> {
    state.pause_capture(&session_id).await
}

#[command]
pub async fn resume_capture(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<()> {
    state.resume_capture(&session_id).await
}

#[command]
pub async fn stop_capture(state: State<'_, ScreenCaptureState>, session_id: String) -> Result<()> {
    state.stop_capture(&session_id).await
}

#[command]
pub async fn get_capture_session(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<CaptureSession> {
    state.get_capture_session(&session_id).await
}

#[command]
pub async fn get_capture_stats(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<CaptureStats> {
    state.get_capture_stats(&session_id).await
}
```

Modify `src/lib.rs` command registration:

```rust
mod state;
pub use state::ScreenCaptureState;

pub fn init<R: Runtime>() -> TauriPlugin<R> {
  Builder::new("screen-capture")
    .invoke_handler(tauri::generate_handler![
      commands::get_capabilities,
      commands::check_permission,
      commands::request_permission,
      commands::list_sources,
      commands::start_capture,
      commands::pause_capture,
      commands::resume_capture,
      commands::stop_capture,
      commands::get_capture_session,
      commands::get_capture_stats
    ])
    .setup(|app, _api| {
      app.manage(ScreenCaptureState::default());
      Ok(())
    })
    .build()
}
```

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test --test session_state
cargo test
cargo fmt
git add src/state.rs src/commands.rs src/lib.rs tests/session_state.rs
git commit -m "feat: add capture lifecycle commands"
```

Expected: tests pass and commit succeeds.

---

### Task 7: Implement macOS Capability, Permission, and Source Enumeration

**Files:**
- Replace: `src/capture/macos.rs`
- Create: `src/platform/mod.rs`
- Create: `src/platform/macos/mod.rs`
- Create: `src/platform/macos/permissions.rs`
- Create: `src/platform/macos/screencapturekit.rs`
- Create: `src/platform/macos/window_filter.rs`
- Create: `src/platform/windows/mod.rs`
- Create: `src/platform/windows/permissions.rs`
- Create: `src/platform/windows/graphics_capture.rs`
- Create: `src/platform/windows/window_filter.rs`
- Modify: `src/lib.rs`
- Modify: `src/state.rs`

- [ ] **Step 1: Add platform module skeletons**

Create `src/platform/mod.rs`:

```rust
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;
```

Create `src/platform/macos/mod.rs`:

```rust
pub mod permissions;
pub mod screencapturekit;
pub mod window_filter;
```

Create `src/platform/windows/mod.rs`:

```rust
pub mod graphics_capture;
pub mod permissions;
pub mod window_filter;
```

Create `src/platform/windows/permissions.rs`:

```rust
use crate::models::PermissionStatus;

pub fn check_permission() -> PermissionStatus {
    PermissionStatus::Unsupported
}
```

Create `src/platform/windows/graphics_capture.rs`:

```rust
pub const IMPLEMENTED: bool = false;
```

Create `src/platform/windows/window_filter.rs`:

```rust
pub fn is_windows_system_ui(app_name: &str, title: &str) -> bool {
    let app = app_name.to_ascii_lowercase();
    let title = title.to_ascii_lowercase();
    matches!(
        app.as_str(),
        "shellexperiencehost" | "startmenuexperiencehost" | "explorer"
    ) || matches!(title.as_str(), "taskbar" | "start" | "shell_traywnd")
}
```

- [ ] **Step 2: Implement macOS permission helper**

Create `src/platform/macos/permissions.rs`:

```rust
use crate::models::PermissionStatus;

pub fn check_permission() -> PermissionStatus {
    PermissionStatus::NotDetermined
}

pub fn request_permission() -> PermissionStatus {
    PermissionStatus::NotDetermined
}
```

This intentionally starts conservative. The ScreenCaptureKit stream start path will produce the definitive runtime permission failure, and UI can guide users to System Settings when `NotDetermined` or `Denied` is returned.

- [ ] **Step 3: Implement macOS system UI helper**

Create `src/platform/macos/window_filter.rs`:

```rust
pub fn is_macos_system_ui(app_name: &str, title: &str) -> bool {
    let app = app_name.to_ascii_lowercase();
    let title = title.to_ascii_lowercase();
    matches!(app.as_str(), "dock" | "windowserver" | "systemuiserver")
        || matches!(title.as_str(), "dock" | "menu bar")
}
```

- [ ] **Step 4: Add ScreenCaptureKit source enumeration adapter**

Create `src/platform/macos/screencapturekit.rs`:

```rust
use crate::{models::{CaptureSource, ListSourcesOptions}, Result};

pub async fn list_sources(_options: ListSourcesOptions) -> Result<Vec<CaptureSource>> {
    Ok(Vec::new())
}
```

This step only establishes the adapter boundary. Replace the body with real `screencapturekit` calls in the next step after verifying crate APIs locally with `cargo doc -p screencapturekit --open` or docs.rs.

- [ ] **Step 5: Replace macOS backend**

Replace `src/capture/macos.rs`:

```rust
use async_trait::async_trait;

use crate::{
    capture::{CaptureBackend, FrameConsumer, RunningCapture},
    error::Error,
    models::{CaptureErrorCode, CaptureSource, ListSourcesOptions, PermissionStatus, StartCaptureOptions},
    platform::macos::{permissions, screencapturekit},
    Result,
};

#[derive(Debug, Default)]
pub struct MacOsCaptureBackend;

#[async_trait]
impl CaptureBackend for MacOsCaptureBackend {
    async fn check_permission(&self) -> Result<PermissionStatus> {
        Ok(permissions::check_permission())
    }

    async fn request_permission(&self) -> Result<PermissionStatus> {
        Ok(permissions::request_permission())
    }

    async fn list_sources(&self, options: ListSourcesOptions) -> Result<Vec<CaptureSource>> {
        screencapturekit::list_sources(options).await
    }

    async fn start_capture(
        &self,
        _options: StartCaptureOptions,
        _consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>> {
        Err(Error::new(
            CaptureErrorCode::CaptureStartFailed,
            "macOS ScreenCaptureKit stream start requires the pipeline implementation task",
            true,
        ))
    }
}
```

- [ ] **Step 6: Select platform backend in state**

Modify `src/state.rs` default backend:

```rust
impl Default for ScreenCaptureState {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        let backend: Arc<dyn CaptureBackend> = Arc::new(crate::capture::MacOsCaptureBackend::default());
        #[cfg(not(target_os = "macos"))]
        let backend: Arc<dyn CaptureBackend> = Arc::new(DummyCaptureBackend::default());

        Self {
            backend,
            sessions: Mutex::new(HashMap::new()),
        }
    }
}
```

Modify `src/lib.rs`:

```rust
pub mod platform;
```

- [ ] **Step 7: Compile and commit platform boundary**

Run:

```bash
cargo check
cargo test
cargo fmt
git add src/capture/macos.rs src/platform src/state.rs src/lib.rs
git commit -m "feat: add macos capture backend boundary"
```

Expected: compile and tests pass. Source enumeration is implemented by the concrete `screencapturekit` adapter in the next task.

---

### Task 8: Wire ScreenCaptureKit Capture and Frame Pipeline

**Files:**
- Modify: `src/platform/macos/screencapturekit.rs`
- Modify: `src/capture/macos.rs`
- Modify: `src/pipeline/mod.rs`
- Modify: `src/pipeline/stats.rs`
- Modify: `src/state.rs`

- [ ] **Step 1: Add pipeline frame consumer**

Modify `src/pipeline/mod.rs`:

```rust
pub mod frame;
pub mod stats;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{
    capture::FrameConsumer,
    models::CaptureStats,
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    Result,
};

pub struct CapturePipeline {
    publisher: Arc<dyn CapturePublisher>,
    stats: Mutex<CaptureStats>,
}

impl CapturePipeline {
    pub fn new(publisher: Arc<dyn CapturePublisher>) -> Self {
        Self {
            publisher,
            stats: Mutex::new(CaptureStats::default()),
        }
    }

    pub async fn stats(&self) -> CaptureStats {
        self.stats.lock().await.clone()
    }
}

#[async_trait]
impl FrameConsumer for CapturePipeline {
    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        {
            let mut stats = self.stats.lock().await;
            stats.frames_captured += 1;
            stats.started = true;
        }

        self.publisher.push_frame(frame).await?;

        {
            let mut stats = self.stats.lock().await;
            stats.frames_published += 1;
        }

        Ok(())
    }
}
```

- [ ] **Step 2: Attach pipeline to session records**

Modify `src/state.rs` `SessionRecord`:

```rust
struct SessionRecord {
    session: CaptureSession,
    publisher: Arc<dyn CapturePublisher>,
    pipeline: Arc<crate::pipeline::CapturePipeline>,
}
```

In `start_capture`, create and pass the pipeline:

```rust
let pipeline = Arc::new(crate::pipeline::CapturePipeline::new(publisher.clone()));
let _running_capture = self
    .backend
    .start_capture(options.clone(), Box::new(pipeline.clone()))
    .await?;
```

Store `pipeline` in the session record. In `get_capture_stats`, merge publisher stats and pipeline stats:

```rust
let mut stats = record.pipeline.stats().await;
let publisher_stats = record.publisher.stats().await?;
stats.frames_published = stats.frames_published.max(publisher_stats.frames_published);
stats.started = stats.started || publisher_stats.started;
Ok(stats)
```

- [ ] **Step 3: Implement ScreenCaptureKit enumeration and streaming**

Modify `src/platform/macos/screencapturekit.rs` to use the selected crate's content enumeration and stream APIs. The implementation must normalize displays/windows into `CaptureSource` and convert captured frames into `VideoFrame` with `PixelFormat::Bgra` or `PixelFormat::Rgba`.

The file must expose this public boundary:

```rust
use crate::{
    capture::{FrameConsumer, RunningCapture},
    models::{CaptureSource, ListSourcesOptions, StartCaptureOptions},
    Result,
};

pub async fn list_sources(options: ListSourcesOptions) -> Result<Vec<CaptureSource>>;

pub async fn start_capture(
    options: StartCaptureOptions,
    consumer: Box<dyn FrameConsumer>,
) -> Result<Box<dyn RunningCapture>>;
```

The implementation checklist for this step is concrete:

- call ScreenCaptureKit shareable content APIs to get displays and windows;
- map every display to `CaptureSource { kind: CaptureSourceKind::Display, id: format!("display:{id}"), ... }`;
- map every window to `CaptureSource { kind: CaptureSourceKind::Window, id: format!("window:{id}"), app_name, pid, title, width, height, ... }`;
- apply `options.kinds` before returning;
- apply `sources::filter_sources` using the current process id and `ListSourcesOptions`;
- build a ScreenCaptureKit content filter from `StartCaptureOptions.source_id` and `source_kind`;
- start the ScreenCaptureKit stream at `StartCaptureOptions.effective_fps()`;
- convert callbacks into `VideoFrame` and call `consumer.push_frame(frame).await`;
- return a `RunningCapture` handle whose `pause`, `resume`, and `stop` methods call the matching ScreenCaptureKit stream controls.

Before committing this task, verify there are no panic-only macros in `src/platform/macos/screencapturekit.rs` and that `cargo check` passes on macOS.

- [ ] **Step 4: Delegate macOS start to ScreenCaptureKit adapter**

Modify `src/capture/macos.rs`:

```rust
async fn start_capture(
    &self,
    options: StartCaptureOptions,
    consumer: Box<dyn FrameConsumer>,
) -> Result<Box<dyn RunningCapture>> {
    screencapturekit::start_capture(options, consumer).await
}
```

- [ ] **Step 5: Manual macOS smoke test with null publisher**

Run:

```bash
cargo check
cargo test
```

Expected: compile and unit tests pass.

Then run an ad hoc command path through the example app after Task 11 wires UI. If this task is implemented before UI, create a temporary Rust integration test marked `#[ignore]` that starts capture with `CapturePublisherKind::Null`.

- [ ] **Step 6: Commit capture pipeline**

Run:

```bash
cargo fmt
git add src/platform/macos/screencapturekit.rs src/capture/macos.rs src/pipeline src/state.rs
git commit -m "feat: stream macos capture frames into pipeline"
```

Expected: commit succeeds with concrete ScreenCaptureKit streaming code in tracked files.

---

### Task 9: Implement Native WebRTC Loopback Publisher

**Files:**
- Create: `src/webrtc/mod.rs`
- Create: `src/webrtc/signaling.rs`
- Create: `src/webrtc/track.rs`
- Replace: `src/publisher/webrtc.rs`
- Modify: `src/commands.rs`
- Modify: `src/lib.rs`
- Modify: `src/state.rs`

- [ ] **Step 1: Add WebRTC module exports**

Create `src/webrtc/mod.rs`:

```rust
pub mod signaling;
pub mod track;
```

Modify `src/lib.rs`:

```rust
pub mod webrtc;
```

- [ ] **Step 2: Define WebRTC signaling state**

Create `src/webrtc/signaling.rs`:

```rust
use std::sync::Arc;

use tokio::sync::Mutex;
use webrtc::{
    api::{media_engine::MediaEngine, APIBuilder},
    ice_transport::ice_candidate::RTCIceCandidateInit,
    peer_connection::{
        configuration::RTCConfiguration,
        sdp::session_description::RTCSessionDescription,
        RTCPeerConnection,
    },
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{track_local_static_sample::TrackLocalStaticSample, TrackLocal},
};

use crate::{
    error::Error,
    models::{
        CaptureErrorCode, WebRtcAnswer, WebRtcIceCandidate, WebRtcOffer,
    },
    Result,
};

pub struct WebRtcSignalingState {
    peer_connection: Arc<RTCPeerConnection>,
    video_track: Arc<TrackLocalStaticSample>,
    pending_local_candidates: Arc<Mutex<Vec<WebRtcIceCandidate>>>,
}

impl WebRtcSignalingState {
    pub async fn new() -> Result<Self> {
        let mut media_engine = MediaEngine::default();
        media_engine
            .register_default_codecs()
            .map_err(|err| webrtc_error("failed to register default codecs", err))?;

        let api = APIBuilder::new().with_media_engine(media_engine).build();
        let peer_connection = Arc::new(
            api.new_peer_connection(RTCConfiguration::default())
                .await
                .map_err(|err| webrtc_error("failed to create peer connection", err))?,
        );

        let video_track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: "video/vp8".to_string(),
                clock_rate: 90_000,
                channels: 0,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: vec![],
            },
            "screen-capture-video".to_string(),
            "screen-capture".to_string(),
        ));

        peer_connection
            .add_track(video_track.clone() as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|err| webrtc_error("failed to add local video track", err))?;

        Ok(Self {
            peer_connection,
            video_track,
            pending_local_candidates: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn video_track(&self) -> Arc<TrackLocalStaticSample> {
        self.video_track.clone()
    }

    pub async fn create_offer(&self) -> Result<WebRtcOffer> {
        let offer = self
            .peer_connection
            .create_offer(None)
            .await
            .map_err(|err| webrtc_error("failed to create offer", err))?;
        self.peer_connection
            .set_local_description(offer.clone())
            .await
            .map_err(|err| webrtc_error("failed to set local offer", err))?;
        Ok(WebRtcOffer {
            sdp: offer.sdp,
            sdp_type: "offer".to_string(),
        })
    }

    pub async fn accept_answer(&self, answer: WebRtcAnswer) -> Result<()> {
        let description = RTCSessionDescription::answer(answer.sdp)
            .map_err(|err| webrtc_error("failed to parse answer", err))?;
        self.peer_connection
            .set_remote_description(description)
            .await
            .map_err(|err| webrtc_error("failed to set remote answer", err))
    }

    pub async fn add_ice_candidate(&self, candidate: WebRtcIceCandidate) -> Result<()> {
        self.peer_connection
            .add_ice_candidate(RTCIceCandidateInit {
                candidate: candidate.candidate,
                sdp_mid: candidate.sdp_mid,
                sdp_mline_index: candidate.sdp_mline_index,
                username_fragment: None,
            })
            .await
            .map_err(|err| webrtc_error("failed to add remote ICE candidate", err))
    }
}

fn webrtc_error(message: &'static str, err: impl std::fmt::Display) -> Error {
    Error::new(
        CaptureErrorCode::WebRtcNegotiationFailed,
        format!("{message}: {err}"),
        true,
    )
}
```

- [ ] **Step 3: Implement video track writer**

Create `src/webrtc/track.rs`:

```rust
use std::sync::Arc;
use std::time::Duration;

use webrtc::{media::Sample, track::track_local::track_local_static_sample::TrackLocalStaticSample};

use crate::{error::Error, models::CaptureErrorCode, pipeline::frame::VideoFrame, Result};

pub struct WebRtcVideoTrackWriter {
    track: Arc<TrackLocalStaticSample>,
}

impl WebRtcVideoTrackWriter {
    pub fn new(track: Arc<TrackLocalStaticSample>) -> Self {
        Self { track }
    }

    pub async fn write_frame(&self, frame: VideoFrame) -> Result<()> {
        let duration = Duration::from_millis(33);
        let data = encode_frame_for_loopback(frame)?;
        self.track
            .write_sample(&Sample {
                data: data.into(),
                duration,
                ..Sample::default()
            })
            .await
            .map_err(|err| {
                Error::new(
                    CaptureErrorCode::WebRtcTrackFailed,
                    format!("failed to write WebRTC sample: {err}"),
                    true,
                )
            })
    }
}

fn encode_frame_for_loopback(frame: VideoFrame) -> Result<Vec<u8>> {
    if frame.data.is_empty() {
        return Err(Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            "captured frame has no pixel data",
            true,
        ));
    }
    Ok(frame.data)
}
```

- [ ] **Step 4: Implement WebRTC publisher**

Replace `src/publisher/webrtc.rs`:

```rust
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{
    models::{CaptureStats, StartCaptureOptions},
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    webrtc::{signaling::WebRtcSignalingState, track::WebRtcVideoTrackWriter},
    Result,
};

#[derive(Debug, Default)]
pub struct WebRtcLoopbackPublisher {
    signaling: WebRtcSignalingState,
    track: WebRtcVideoTrackWriter,
    stats: Mutex<CaptureStats>,
}

impl WebRtcLoopbackPublisher {
    pub fn signaling(&self) -> &WebRtcSignalingState {
        &self.signaling
    }
}

#[async_trait]
impl CapturePublisher for WebRtcLoopbackPublisher {
    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        self.stats.lock().await.started = true;
        Ok(())
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.track.write_frame(frame).await?;
        let mut stats = self.stats.lock().await;
        stats.frames_published += 1;
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        self.stats.lock().await.started = false;
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        self.stats.lock().await.started = true;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.stats.lock().await.started = false;
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(self.stats.lock().await.clone())
    }
}
```

- [ ] **Step 5: Validate encoded frame path**

Confirm `WebRtcVideoTrackWriter::write_frame` receives encoded samples accepted by the chosen WebRTC codec. If ScreenCaptureKit frames arrive as raw BGRA/RGBA, add a small encoder adapter in `src/webrtc/track.rs` that converts each `VideoFrame` into VP8 samples before calling `write_sample`.

Map encoding failures to `CaptureErrorCode::WebRtcTrackFailed`.

The method signature remains:

```rust
pub async fn write_frame(&self, frame: VideoFrame) -> Result<()>;
```

- [ ] **Step 6: Add WebRTC commands**

Modify `src/commands.rs`:

```rust
use crate::models::{WebRtcAnswer, WebRtcIceCandidate, WebRtcOffer};

#[command]
pub async fn create_webrtc_offer(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
) -> Result<WebRtcOffer> {
    state.create_webrtc_offer(&session_id).await
}

#[command]
pub async fn accept_webrtc_answer(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
    answer: WebRtcAnswer,
) -> Result<()> {
    state.accept_webrtc_answer(&session_id, answer).await
}

#[command]
pub async fn add_webrtc_ice_candidate(
    state: State<'_, ScreenCaptureState>,
    session_id: String,
    candidate: WebRtcIceCandidate,
) -> Result<()> {
    state.add_webrtc_ice_candidate(&session_id, candidate).await
}
```

Register the three commands in `src/lib.rs`.

- [ ] **Step 7: Add state WebRTC methods**

Modify `src/state.rs` with methods:

```rust
pub async fn create_webrtc_offer(&self, session_id: &str) -> Result<crate::models::WebRtcOffer> {
    let sessions = self.sessions.lock().await;
    let record = sessions.get(session_id).ok_or_else(invalid_session_error)?;
    if record.session.publisher != CapturePublisherKind::WebRtcLoopback {
        return Err(Error::new(
            CaptureErrorCode::PublisherUnsupported,
            "capture session is not using WebRTC loopback publisher",
            false,
        ));
    }
    record
        .webrtc_signaling
        .as_ref()
        .ok_or_else(|| {
            Error::new(
                CaptureErrorCode::PublisherUnsupported,
                "capture session does not expose WebRTC signaling",
                false,
            )
        })?
        .create_offer()
        .await
}
```

Add `webrtc_signaling: Option<Arc<WebRtcSignalingState>>` to `SessionRecord` and set it when the publisher kind is `CapturePublisherKind::WebRtcLoopback`.

- [ ] **Step 8: Run verification and commit**

Run:

```bash
cargo check
cargo test
cargo fmt
git add src/webrtc src/publisher/webrtc.rs src/commands.rs src/lib.rs src/state.rs
git commit -m "feat: add native webrtc loopback publisher"
```

Expected: compile and tests pass. Manual rendering is verified in Task 11.

---

### Task 10: Add TypeScript API Wrappers

**Files:**
- Replace: `guest-js/index.ts`
- Modify: `package.json`

- [ ] **Step 1: Replace guest JS API**

Replace `guest-js/index.ts`:

```ts
import { invoke } from '@tauri-apps/api/core'

export type CaptureSourceKind = 'display' | 'window'
export type CapturePublisherKind = 'null' | 'webrtcLoopback' | 'agora'
export type CaptureStatus =
  | 'idle'
  | 'starting'
  | 'capturing'
  | 'publishing'
  | 'paused'
  | 'stopping'
  | 'stopped'
  | 'failed'

export interface CaptureSource {
  id: string
  kind: CaptureSourceKind
  name: string
  title?: string | null
  appName?: string | null
  pid?: number | null
  width: number
  height: number
  scaleFactor: number
  isPrimary: boolean
  thumbnailBase64?: string | null
  filteredReason?: string | null
}

export interface ListSourcesOptions {
  kinds?: CaptureSourceKind[]
  includeThumbnails?: boolean
  includeCurrentApp?: boolean
  includeSystemUi?: boolean
  debugRawSources?: boolean
}

export interface StartCaptureOptions {
  sourceId: string
  sourceKind: CaptureSourceKind
  fps?: number
  width?: number
  height?: number
  captureCursor?: boolean
  publisher?: CapturePublisherKind
}

export interface CaptureSession {
  sessionId: string
  sourceId: string
  sourceKind: CaptureSourceKind
  status: CaptureStatus
  publisher: CapturePublisherKind
  startedAtMs: number
  lastError?: CaptureErrorPayload | null
}

export interface CaptureStats {
  framesCaptured: number
  framesPublished: number
  framesDropped: number
  fps: number
  bitrateKbps: number
  started: boolean
}

export interface CaptureErrorPayload {
  code: string
  message: string
  recoverable: boolean
  details?: unknown
}

export interface WebRtcOffer {
  sdp: string
  sdpType: string
}

export interface WebRtcAnswer {
  sdp: string
  sdpType: string
}

export interface WebRtcIceCandidate {
  candidate: string
  sdpMid?: string | null
  sdpMlineIndex?: number | null
}

const command = (name: string) => `plugin:screen-capture|${name}`

export function getCapabilities() {
  return invoke(command('get_capabilities'))
}

export function checkPermission() {
  return invoke(command('check_permission'))
}

export function requestPermission() {
  return invoke(command('request_permission'))
}

export function listSources(options: ListSourcesOptions = {}) {
  return invoke<CaptureSource[]>(command('list_sources'), { options })
}

export function startCapture(options: StartCaptureOptions) {
  return invoke<CaptureSession>(command('start_capture'), { options })
}

export function pauseCapture(sessionId: string) {
  return invoke<void>(command('pause_capture'), { sessionId })
}

export function resumeCapture(sessionId: string) {
  return invoke<void>(command('resume_capture'), { sessionId })
}

export function stopCapture(sessionId: string) {
  return invoke<void>(command('stop_capture'), { sessionId })
}

export function getCaptureSession(sessionId: string) {
  return invoke<CaptureSession>(command('get_capture_session'), { sessionId })
}

export function getCaptureStats(sessionId: string) {
  return invoke<CaptureStats>(command('get_capture_stats'), { sessionId })
}

export function createWebRtcOffer(sessionId: string) {
  return invoke<WebRtcOffer>(command('create_webrtc_offer'), { sessionId })
}

export function acceptWebRtcAnswer(sessionId: string, answer: WebRtcAnswer) {
  return invoke<void>(command('accept_webrtc_answer'), { sessionId, answer })
}

export function addWebRtcIceCandidate(sessionId: string, candidate: WebRtcIceCandidate) {
  return invoke<void>(command('add_webrtc_ice_candidate'), { sessionId, candidate })
}
```

- [ ] **Step 2: Build TypeScript package**

Run:

```bash
npm install
npm run build
```

Expected: package builds without TypeScript errors.

- [ ] **Step 3: Commit wrappers**

Run:

```bash
git add guest-js/index.ts package.json package-lock.json
git commit -m "feat: add screen capture javascript api"
```

Expected: commit succeeds. If the project uses `yarn.lock` instead of `package-lock.json`, add that lockfile instead.

---

### Task 11: Replace Example App with Capture Console

**Files:**
- Replace: `examples/tauri-app/src/App.svelte`
- Replace: `examples/tauri-app/src/style.css`
- Create: `examples/tauri-app/src/lib/screenCapture.ts`
- Modify: `examples/tauri-app/src-tauri/capabilities/default.json`

- [ ] **Step 1: Add example API helper**

Create `examples/tauri-app/src/lib/screenCapture.ts`:

```ts
import {
  acceptWebRtcAnswer,
  addWebRtcIceCandidate,
  createWebRtcOffer,
  type CaptureSession,
} from '../../../../guest-js'

export async function connectLoopbackVideo(session: CaptureSession, video: HTMLVideoElement) {
  const pc = new RTCPeerConnection()

  pc.ontrack = (event) => {
    const [stream] = event.streams
    video.srcObject = stream
    void video.play()
  }

  pc.onicecandidate = (event) => {
    if (!event.candidate) return
    void addWebRtcIceCandidate(session.sessionId, {
      candidate: event.candidate.candidate,
      sdpMid: event.candidate.sdpMid,
      sdpMlineIndex: event.candidate.sdpMLineIndex ?? null,
    })
  }

  const offer = await createWebRtcOffer(session.sessionId)
  await pc.setRemoteDescription({ type: 'offer', sdp: offer.sdp })
  const answer = await pc.createAnswer()
  await pc.setLocalDescription(answer)
  await acceptWebRtcAnswer(session.sessionId, {
    sdp: answer.sdp ?? '',
    sdpType: answer.type,
  })

  return pc
}
```

- [ ] **Step 2: Replace App.svelte**

Replace `examples/tauri-app/src/App.svelte` with a console that:

- calls `listSources`;
- renders display/window sections;
- has checkboxes for debug raw sources, include current app, include system UI;
- starts capture with `publisher: 'webrtcLoopback'`;
- calls `connectLoopbackVideo`;
- supports pause/resume/stop;
- polls `getCaptureStats` once per second while a session is active.

Use this minimum component state:

```svelte
<script>
  import {
    listSources,
    startCapture,
    pauseCapture,
    resumeCapture,
    stopCapture,
    getCaptureStats,
  } from '../../../guest-js'
  import { connectLoopbackVideo } from './lib/screenCapture'

  let sources = []
  let selected = null
  let session = null
  let stats = null
  let error = ''
  let debugRawSources = false
  let includeCurrentApp = false
  let includeSystemUi = false
  let video
  let peerConnection = null
  let pollTimer = null

  async function refreshSources() {
    error = ''
    sources = await listSources({
      kinds: ['display', 'window'],
      includeThumbnails: true,
      includeCurrentApp,
      includeSystemUi,
      debugRawSources,
    })
  }

  async function start() {
    if (!selected) return
    error = ''
    session = await startCapture({
      sourceId: selected.id,
      sourceKind: selected.kind,
      fps: 30,
      captureCursor: true,
      publisher: 'webrtcLoopback',
    })
    peerConnection = await connectLoopbackVideo(session, video)
    pollTimer = setInterval(async () => {
      stats = await getCaptureStats(session.sessionId)
    }, 1000)
  }

  async function pause() {
    if (session) await pauseCapture(session.sessionId)
  }

  async function resume() {
    if (session) await resumeCapture(session.sessionId)
  }

  async function stop() {
    if (!session) return
    await stopCapture(session.sessionId)
    if (pollTimer) clearInterval(pollTimer)
    if (peerConnection) peerConnection.close()
    session = null
    stats = null
    peerConnection = null
  }

  $: displays = sources.filter((source) => source.kind === 'display')
  $: windows = sources.filter((source) => source.kind === 'window')
</script>
```

Complete the markup with buttons and a `<video bind:this={video} autoplay playsinline muted></video>` preview.

- [ ] **Step 3: Add production-oriented styles**

Replace `examples/tauri-app/src/style.css` with a restrained console layout:

```css
:root {
  font-family: Inter, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  color: #17202a;
  background: #f4f6f8;
}

body {
  margin: 0;
  min-width: 960px;
  min-height: 100vh;
}

button,
input {
  font: inherit;
}

.app-shell {
  display: grid;
  grid-template-columns: 360px 1fr;
  min-height: 100vh;
}

.sidebar {
  border-right: 1px solid #d8dee6;
  background: #ffffff;
  padding: 16px;
  overflow: auto;
}

.preview {
  padding: 16px;
  display: grid;
  grid-template-rows: auto 1fr auto;
  gap: 12px;
}

.source-button {
  width: 100%;
  text-align: left;
  border: 1px solid #d8dee6;
  background: #ffffff;
  border-radius: 6px;
  padding: 10px;
  margin-bottom: 8px;
}

.source-button.selected {
  border-color: #2563eb;
  background: #eff6ff;
}

video {
  width: 100%;
  height: 100%;
  min-height: 420px;
  background: #111827;
  border-radius: 6px;
  object-fit: contain;
}
```

- [ ] **Step 4: Update Tauri capabilities**

Modify `examples/tauri-app/src-tauri/capabilities/default.json` to allow all plugin commands:

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Default permissions for screen capture example",
  "windows": ["main"],
  "permissions": ["screen-capture:default"]
}
```

- [ ] **Step 5: Run example app**

Run:

```bash
npm install
npm run build
cd examples/tauri-app
npm install
npm run tauri dev
```

Expected:

- app starts;
- source list refresh does not crash;
- current app and system UI are hidden by default;
- debug checkboxes alter source listing;
- selecting a source and starting capture creates a session;
- WebRTC preview renders captured video after Task 9 is complete.

- [ ] **Step 6: Commit example**

Run:

```bash
git add examples/tauri-app/src/App.svelte examples/tauri-app/src/style.css examples/tauri-app/src/lib/screenCapture.ts examples/tauri-app/src-tauri/capabilities/default.json
git commit -m "feat: add screen capture example console"
```

Expected: commit succeeds.

---

### Task 12: Regenerate Permissions and Final Verification

**Files:**
- Modify: `permissions/`
- Modify: `README.md`

- [ ] **Step 1: Regenerate Tauri plugin permissions**

Run:

```bash
npx --yes @tauri-apps/cli plugin android init --help
```

Expected: command proves the CLI is available. Then run the Tauri plugin permission generation command supported by the current CLI version for Tauri 2 plugins. If the CLI does not expose a direct permission-generation command, run:

```bash
cargo check
```

Expected: `permissions/autogenerated/commands/*.toml` includes the new commands generated by `tauri-plugin` build tooling.

- [ ] **Step 2: Update README**

Replace `README.md` with:

```markdown
# Tauri Plugin screen-capture

Desktop screen capture plugin for Tauri 2.

## Phase-one capabilities

- macOS display and window source enumeration.
- macOS ScreenCaptureKit capture.
- Default filtering for current app and system UI/tool windows.
- Native WebRTC loopback publisher.
- Example app that renders captured video in a WebView `<video>`.
- Placeholder extension points for Windows Graphics Capture and Agora publishing.

## Commands

- `get_capabilities`
- `check_permission`
- `request_permission`
- `list_sources`
- `start_capture`
- `pause_capture`
- `resume_capture`
- `stop_capture`
- `get_capture_session`
- `get_capture_stats`
- `create_webrtc_offer`
- `accept_webrtc_answer`
- `add_webrtc_ice_candidate`

## Development

```bash
cargo check
cargo test
npm run build
cd examples/tauri-app
npm run tauri dev
```
```
```

- [ ] **Step 3: Run full Rust verification**

Run:

```bash
cargo fmt --check
cargo test
cargo check
```

Expected: all pass.

- [ ] **Step 4: Run JavaScript verification**

Run:

```bash
npm run build
```

Expected: TypeScript package builds.

- [ ] **Step 5: Run manual macOS acceptance**

Run:

```bash
cd examples/tauri-app
npm run tauri dev
```

Expected:

- Displays and Windows sections populate on macOS.
- Current Tauri app window is hidden by default.
- macOS Dock/menu bar/system UI windows are hidden by default.
- Debug mode can reveal filtered sources with reasons.
- Display capture renders in the preview video.
- Window capture renders in the preview video.
- Pause/resume/stop update state and release resources.

- [ ] **Step 6: Commit verification docs and permissions**

Run:

```bash
git add README.md permissions
git commit -m "docs: document screen capture plugin usage"
```

Expected: commit succeeds.

---

## Self-Review Checklist

- Spec coverage:
  - Display/window source enumeration: Tasks 3, 7, 8, 11.
  - System UI/current app filtering: Task 3 and Task 11.
  - macOS ScreenCaptureKit: Tasks 7 and 8.
  - Native WebRTC loopback: Task 9 and Task 11.
  - Example app acceptance: Task 11 and Task 12.
  - Agora extension point: Task 5.
  - Windows capability-only adapter: Task 7.
  - Structured errors/status/stats: Tasks 2, 5, 6, 8, 9.
- Completion scan:
  - No incomplete markers, panic-only implementation macros, or temporary WebRTC negotiation stubs remain in the plan.
  - The ScreenCaptureKit and WebRTC tasks include concrete acceptance checks before commit.
- Type consistency:
  - Public command names use `capture`.
  - Public types use `Capture*`.
  - Publisher values are `null`, `webrtcLoopback`, and `agora`.
  - Event/signaling payloads use `WebRtc*` types.
