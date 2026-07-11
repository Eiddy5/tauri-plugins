# Windows Share Border Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Windows-only local green sharing indicator that frames the shared display or window without appearing in the captured stream.

**Architecture:** Split the feature into pure overlay geometry, a session-facing overlay trait, and a Windows Win32 implementation. `ScreenCaptureState` owns an overlay manager per session and drives show/hide/stop with capture lifecycle calls.

**Tech Stack:** Rust, Tauri v2, Windows Win32 APIs via the existing `windows` crate, `windows-capture`, Tokio tests.

---

## Files

- Create: `src/overlay/mod.rs` for cross-platform overlay interfaces, default style, and pure corner geometry.
- Create: `src/overlay/windows.rs` for the Win32 overlay manager, overlay windows, bounds polling, and WinEvent hooks.
- Modify: `src/lib.rs` to expose the overlay module internally.
- Modify: `src/state.rs` to create and drive one overlay manager per capture session.
- Modify: `Cargo.toml` to add the Windows Win32 feature flags needed by overlay windows.
- Test: `tests/overlay_geometry.rs` for pure geometry.
- Test: `tests/session_state.rs` for lifecycle integration through a test overlay manager.

## Task 1: Corner Geometry

**Files:**
- Create: `src/overlay/mod.rs`
- Modify: `src/lib.rs`
- Test: `tests/overlay_geometry.rs`

- [ ] **Step 1: Write the failing geometry tests**

Create `tests/overlay_geometry.rs`:

```rust
use tauri_plugin_screen_capture::overlay::{
    corner_segments, OverlayRect, OverlaySegment, OverlayStyle,
};

#[test]
fn corner_segments_frame_the_target_rect_with_eight_segments() {
    let style = OverlayStyle {
        color: 0x22C55E,
        thickness: 4,
        corner_length: 32,
    };

    let segments = corner_segments(
        OverlayRect {
            left: 100,
            top: 200,
            right: 500,
            bottom: 600,
        },
        style,
    );

    assert_eq!(
        segments,
        vec![
            OverlaySegment::new(100, 200, 32, 4),
            OverlaySegment::new(100, 200, 4, 32),
            OverlaySegment::new(468, 200, 32, 4),
            OverlaySegment::new(496, 200, 4, 32),
            OverlaySegment::new(100, 596, 32, 4),
            OverlaySegment::new(100, 568, 4, 32),
            OverlaySegment::new(468, 596, 32, 4),
            OverlaySegment::new(496, 568, 4, 32),
        ]
    );
}

#[test]
fn corner_segments_clamp_length_for_small_targets() {
    let segments = corner_segments(
        OverlayRect {
            left: 10,
            top: 20,
            right: 50,
            bottom: 52,
        },
        OverlayStyle {
            color: 0x22C55E,
            thickness: 4,
            corner_length: 32,
        },
    );

    assert_eq!(
        segments,
        vec![
            OverlaySegment::new(10, 20, 20, 4),
            OverlaySegment::new(10, 20, 4, 16),
            OverlaySegment::new(30, 20, 20, 4),
            OverlaySegment::new(46, 20, 4, 16),
            OverlaySegment::new(10, 48, 20, 4),
            OverlaySegment::new(10, 36, 4, 16),
            OverlaySegment::new(30, 48, 20, 4),
            OverlaySegment::new(46, 36, 4, 16),
        ]
    );
}

#[test]
fn default_style_matches_the_share_border_spec() {
    assert_eq!(
        OverlayStyle::default(),
        OverlayStyle {
            color: 0x22C55E,
            thickness: 4,
            corner_length: 32,
        }
    );
}
```

- [ ] **Step 2: Run the geometry tests and verify red**

Run:

```powershell
cargo test --test overlay_geometry
```

Expected: FAIL because `tauri_plugin_screen_capture::overlay` does not exist.

- [ ] **Step 3: Implement pure geometry**

Add this to `src/lib.rs`:

```rust
pub mod overlay;
```

Create `src/overlay/mod.rs`:

```rust
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::WindowsShareOverlay;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayStyle {
    pub color: u32,
    pub thickness: i32,
    pub corner_length: i32,
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            color: 0x22C55E,
            thickness: 4,
            corner_length: 32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl OverlayRect {
    pub fn width(self) -> i32 {
        self.right.saturating_sub(self.left).max(0)
    }

    pub fn height(self) -> i32 {
        self.bottom.saturating_sub(self.top).max(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlaySegment {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl OverlaySegment {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

pub fn corner_segments(rect: OverlayRect, style: OverlayStyle) -> Vec<OverlaySegment> {
    let thickness = style.thickness.max(1);
    let horizontal_length = style.corner_length.max(thickness).min((rect.width() / 2).max(thickness));
    let vertical_length = style
        .corner_length
        .max(thickness)
        .min((rect.height() / 2).max(thickness));

    vec![
        OverlaySegment::new(rect.left, rect.top, horizontal_length, thickness),
        OverlaySegment::new(rect.left, rect.top, thickness, vertical_length),
        OverlaySegment::new(rect.right - horizontal_length, rect.top, horizontal_length, thickness),
        OverlaySegment::new(rect.right - thickness, rect.top, thickness, vertical_length),
        OverlaySegment::new(rect.left, rect.bottom - thickness, horizontal_length, thickness),
        OverlaySegment::new(rect.left, rect.bottom - vertical_length, thickness, vertical_length),
        OverlaySegment::new(
            rect.right - horizontal_length,
            rect.bottom - thickness,
            horizontal_length,
            thickness,
        ),
        OverlaySegment::new(
            rect.right - thickness,
            rect.bottom - vertical_length,
            thickness,
            vertical_length,
        ),
    ]
}
```

Create `src/overlay/windows.rs` with a temporary compile-only stub:

```rust
#[derive(Debug, Default)]
pub struct WindowsShareOverlay;
```

- [ ] **Step 4: Run the geometry tests and verify green**

Run:

```powershell
cargo test --test overlay_geometry
```

Expected: PASS.

- [ ] **Step 5: Commit Task 1**

```powershell
git add src/lib.rs src/overlay/mod.rs src/overlay/windows.rs tests/overlay_geometry.rs
git commit -m "Add share border geometry"
```

## Task 2: Overlay Lifecycle Interface

**Files:**
- Modify: `src/overlay/mod.rs`
- Modify: `src/state.rs`
- Test: `tests/session_state.rs`

- [ ] **Step 1: Write the failing lifecycle test**

In `tests/session_state.rs`, add `OverlayLifecycleCounters`, `OverlayProbe`, and this test near the existing state lifecycle tests:

```rust
#[derive(Debug, Default)]
struct OverlayLifecycleCounters {
    starts: AtomicUsize,
    hides: AtomicUsize,
    shows: AtomicUsize,
    stops: AtomicUsize,
}

#[derive(Debug)]
struct OverlayProbe {
    counters: Arc<OverlayLifecycleCounters>,
}

#[async_trait]
impl tauri_plugin_screen_capture::overlay::ShareOverlay for OverlayProbe {
    async fn start(&self, _target: tauri_plugin_screen_capture::overlay::OverlayTarget) -> Result<()> {
        self.counters.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn show(&self) -> Result<()> {
        self.counters.shows.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn hide(&self) -> Result<()> {
        self.counters.hides.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.counters.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn state_drives_share_overlay_lifecycle() {
    let backend_counters = Arc::new(BackendLifecycleCounters::default());
    let last_options = Arc::new(Mutex::new(None));
    let backend = Arc::new(ObservableBackend {
        counters: Arc::clone(&backend_counters),
        last_options,
    });
    let overlay_counters = Arc::new(OverlayLifecycleCounters::default());
    let overlay = Arc::new(OverlayProbe {
        counters: Arc::clone(&overlay_counters),
    });
    let state = ScreenCaptureState::with_backend_and_overlay(backend, overlay);

    let session = state.start_capture(start_options()).await.expect("start");
    assert_eq!(overlay_counters.starts.load(Ordering::SeqCst), 1);

    state.pause_capture(&session.session_id).await.expect("pause");
    assert_eq!(overlay_counters.hides.load(Ordering::SeqCst), 1);

    state.resume_capture(&session.session_id).await.expect("resume");
    assert_eq!(overlay_counters.shows.load(Ordering::SeqCst), 1);

    state.stop_capture(&session.session_id).await.expect("stop");
    assert_eq!(overlay_counters.stops.load(Ordering::SeqCst), 1);
}
```

- [ ] **Step 2: Run the lifecycle test and verify red**

Run:

```powershell
cargo test --test session_state state_drives_share_overlay_lifecycle
```

Expected: FAIL because `ShareOverlay`, `OverlayTarget`, and `with_backend_and_overlay` do not exist.

- [ ] **Step 3: Add overlay interface and no-op implementation**

Extend `src/overlay/mod.rs`:

```rust
use async_trait::async_trait;

use crate::{models::CaptureSourceKind, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayTarget {
    pub source_id: String,
    pub source_kind: CaptureSourceKind,
}

#[async_trait]
pub trait ShareOverlay: Send + Sync {
    async fn start(&self, target: OverlayTarget) -> Result<()>;
    async fn show(&self) -> Result<()>;
    async fn hide(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct NoopShareOverlay;

#[async_trait]
impl ShareOverlay for NoopShareOverlay {
    async fn start(&self, _target: OverlayTarget) -> Result<()> {
        Ok(())
    }

    async fn show(&self) -> Result<()> {
        Ok(())
    }

    async fn hide(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }
}
```

Modify `src/state.rs`:

```rust
use crate::overlay::{NoopShareOverlay, OverlayTarget, ShareOverlay};
```

Add `overlay: Arc<dyn ShareOverlay>` to `SessionRecord`.

Add `overlay: Arc<dyn ShareOverlay>` to `ScreenCaptureState`.

Update constructors:

```rust
pub fn with_backend(backend: Arc<dyn CaptureBackend>) -> Self {
    Self::with_backend_and_overlay(backend, Arc::new(NoopShareOverlay))
}

pub fn with_backend_and_overlay(
    backend: Arc<dyn CaptureBackend>,
    overlay: Arc<dyn ShareOverlay>,
) -> Self {
    Self {
        backend,
        overlay,
        sessions: Mutex::new(HashMap::new()),
    }
}
```

In `start_capture`, after `publisher.start(options.clone()).await?` and before inserting the session:

```rust
let overlay = Arc::clone(&self.overlay);
overlay
    .start(OverlayTarget {
        source_id: options.source_id.clone(),
        source_kind: options.source_kind,
    })
    .await?;
```

Store `overlay` in the `SessionRecord`.

In `pause_capture`, clone the overlay from the record and call `overlay.hide().await?` after publisher pause succeeds.

In `resume_capture`, clone the overlay from the record and call `overlay.show().await?` after publisher resume succeeds.

In `stop_capture`, clone the overlay from the record and call `overlay.stop().await?` after publisher and running capture stop succeed.

- [ ] **Step 4: Run the lifecycle test and verify green**

Run:

```powershell
cargo test --test session_state state_drives_share_overlay_lifecycle
```

Expected: PASS.

- [ ] **Step 5: Run existing session tests**

Run:

```powershell
cargo test --test session_state
```

Expected: PASS.

- [ ] **Step 6: Commit Task 2**

```powershell
git add src/overlay/mod.rs src/state.rs tests/session_state.rs
git commit -m "Drive share overlay from capture sessions"
```

## Task 3: Windows Overlay Window Creation

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/overlay/windows.rs`
- Test: `cargo check` and manual Windows example verification

- [ ] **Step 1: Add required Windows features**

Modify the Windows dependency feature list in `Cargo.toml`:

```toml
windows = { version = "0.62", features = [
  "Graphics_Capture",
  "Win32_Foundation",
  "Win32_Graphics_Gdi",
  "Win32_UI_WindowsAndMessaging",
] }
```

- [ ] **Step 2: Implement Win32 overlay windows**

Replace `src/overlay/windows.rs` with:

```rust
use std::{
    ffi::OsStr,
    os::windows::ffi::OsStrExt,
    ptr::null_mut,
    sync::Mutex,
};

use async_trait::async_trait;
use windows::Win32::{
    Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{CreateSolidBrush, DeleteObject, HBRUSH},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetModuleHandleW, RegisterClassW,
        SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
        CS_HREDRAW, CS_VREDRAW, GWL_EXSTYLE, HMENU, HWND_TOPMOST, LWA_ALPHA, SW_HIDE,
        SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, WDA_EXCLUDEFROMCAPTURE, WNDCLASSW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
        WS_POPUP,
    },
};
use windows::core::PCWSTR;

use crate::{
    overlay::{corner_segments, OverlayRect, OverlayStyle, OverlayTarget, ShareOverlay},
    Result,
};

const CLASS_NAME: &str = "TauriPluginScreenCaptureShareBorder";

#[derive(Debug)]
pub struct WindowsShareOverlay {
    style: OverlayStyle,
    windows: Mutex<Vec<OverlayWindow>>,
}

impl Default for WindowsShareOverlay {
    fn default() -> Self {
        Self {
            style: OverlayStyle::default(),
            windows: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ShareOverlay for WindowsShareOverlay {
    async fn start(&self, _target: OverlayTarget) -> Result<()> {
        let rect = OverlayRect {
            left: 100,
            top: 100,
            right: 500,
            bottom: 400,
        };
        self.show_rect(rect)
    }

    async fn show(&self) -> Result<()> {
        for window in self.windows.lock().expect("overlay window lock").iter() {
            window.show();
        }
        Ok(())
    }

    async fn hide(&self) -> Result<()> {
        for window in self.windows.lock().expect("overlay window lock").iter() {
            window.hide();
        }
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.destroy_all();
        Ok(())
    }
}

impl WindowsShareOverlay {
    fn show_rect(&self, rect: OverlayRect) -> Result<()> {
        let segments = corner_segments(rect, self.style);
        let mut windows = self.windows.lock().expect("overlay window lock");
        if windows.len() != segments.len() {
            drop(windows);
            self.destroy_all();
            windows = self.windows.lock().expect("overlay window lock");
            for _ in 0..segments.len() {
                windows.push(OverlayWindow::new(self.style.color)?);
            }
        }

        for (window, segment) in windows.iter().zip(segments) {
            window.position(segment.x, segment.y, segment.width, segment.height);
            window.show();
        }
        Ok(())
    }

    fn destroy_all(&self) {
        self.windows.lock().expect("overlay window lock").clear();
    }
}

#[derive(Debug)]
struct OverlayWindow {
    hwnd: HWND,
}

impl OverlayWindow {
    fn new(color: u32) -> Result<Self> {
        register_window_class(color)?;
        let class_name = wide(CLASS_NAME);
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_TRANSPARENT
                    | WS_EX_LAYERED
                    | WS_EX_NOACTIVATE,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(class_name.as_ptr()),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                HMENU::default(),
                GetModuleHandleW(None).unwrap_or_default(),
                null_mut(),
            )
        };
        if hwnd.0 == 0 {
            return Err(crate::Error::new(
                crate::CaptureErrorCode::Internal,
                "failed to create share border overlay window",
                true,
            ));
        }

        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        }

        Ok(Self { hwnd })
    }

    fn position(&self, x: i32, y: i32, width: i32, height: i32) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }
    }

    fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn register_window_class(color: u32) -> Result<()> {
    let class_name = wide(CLASS_NAME);
    let brush = unsafe {
        CreateSolidBrush(COLORREF(
            ((color & 0x0000FF) << 16 | (color & 0x00FF00) | ((color & 0xFF0000) >> 16)) as u32,
        ))
    };
    if brush.0 == 0 {
        return Err(crate::Error::new(
            crate::CaptureErrorCode::Internal,
            "failed to create share border brush",
            true,
        ));
    }

    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: unsafe { GetModuleHandleW(None).unwrap_or_default() },
        hbrBackground: HBRUSH(brush.0),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };

    let atom = unsafe { RegisterClassW(&class) };
    unsafe {
        let _ = DeleteObject(brush.into());
    }
    if atom == 0 {
        return Ok(());
    }
    Ok(())
}

extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
```

- [ ] **Step 3: Compile-check the Windows overlay**

Run:

```powershell
cargo check
```

Expected: PASS. If the `windows` crate API names differ, adjust imports to the crate version but keep the same behavior: topmost click-through tool windows, solid brush, `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`.

- [ ] **Step 4: Commit Task 3**

```powershell
git add Cargo.toml src/overlay/windows.rs
git commit -m "Create Windows share border overlay windows"
```

## Task 4: Real Bounds Resolution

**Files:**
- Modify: `src/overlay/windows.rs`
- Modify: `src/platform/windows/graphics_capture.rs` only if existing source-id parsing must be shared
- Test: `tests/overlay_geometry.rs`

- [ ] **Step 1: Write source-id parsing tests**

Append to `tests/overlay_geometry.rs`:

```rust
#[cfg(target_os = "windows")]
#[test]
fn windows_overlay_target_parses_window_hwnd_source_ids() {
    use tauri_plugin_screen_capture::overlay::windows_target_handle_from_source_id;

    assert_eq!(windows_target_handle_from_source_id("window:2a").unwrap(), 0x2a);
    assert!(windows_target_handle_from_source_id("display:abc:1").is_none());
}
```

- [ ] **Step 2: Run test and verify red**

Run:

```powershell
cargo test --test overlay_geometry windows_overlay_target_parses_window_hwnd_source_ids
```

Expected: FAIL because `windows_target_handle_from_source_id` does not exist.

- [ ] **Step 3: Implement target parsing and bounds lookup**

In `src/overlay/mod.rs`, export the helper on Windows:

```rust
#[cfg(target_os = "windows")]
pub use windows::windows_target_handle_from_source_id;
```

In `src/overlay/windows.rs`, add:

```rust
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, IsWindow, IsWindowVisible};

pub fn windows_target_handle_from_source_id(source_id: &str) -> Option<isize> {
    source_id
        .strip_prefix("window:")
        .and_then(|id| usize::from_str_radix(id, 16).ok())
        .map(|hwnd| hwnd as isize)
}

fn window_bounds(source_id: &str) -> Option<OverlayRect> {
    let hwnd = HWND(windows_target_handle_from_source_id(source_id)?);
    unsafe {
        if !IsWindow(hwnd).as_bool() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut rect = RECT::default();
        if !GetWindowRect(hwnd, &mut rect).as_bool() {
            return None;
        }
        Some(OverlayRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    }
}
```

Update `WindowsShareOverlay::start` to use `window_bounds` for `CaptureSourceKind::Window`. Keep display targets hidden until Task 5:

```rust
async fn start(&self, target: OverlayTarget) -> Result<()> {
    match target.source_kind {
        crate::CaptureSourceKind::Window => {
            if let Some(rect) = window_bounds(&target.source_id) {
                self.show_rect(rect)?;
            }
        }
        crate::CaptureSourceKind::Display => {}
    }
    Ok(())
}
```

- [ ] **Step 4: Run source parsing tests**

Run:

```powershell
cargo test --test overlay_geometry windows_overlay_target_parses_window_hwnd_source_ids
```

Expected: PASS.

- [ ] **Step 5: Compile-check**

Run:

```powershell
cargo check
```

Expected: PASS.

- [ ] **Step 6: Commit Task 4**

```powershell
git add src/overlay/mod.rs src/overlay/windows.rs tests/overlay_geometry.rs
git commit -m "Resolve Windows window bounds for share border"
```

## Task 5: WinEvent Hide and Reshow

**Files:**
- Modify: `src/overlay/windows.rs`
- Test: `cargo check`

- [ ] **Step 1: Add WinEvent hook support**

Extend the Windows feature list in `Cargo.toml` if these constants are unavailable:

```toml
"Win32_System_Threading",
```

Add imports to `src/overlay/windows.rs`:

```rust
use std::sync::{Arc, Weak};
use windows::Win32::UI::WindowsAndMessaging::{
    SetWinEventHook, UnhookWinEvent, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE,
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW, EVENT_SYSTEM_MOVESIZEEND,
    EVENT_SYSTEM_MOVESIZESTART, HWINEVENTHOOK, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS,
};
```

Add state:

```rust
struct OverlayState {
    target: Option<OverlayTarget>,
    moving_or_sizing: bool,
}

static WINDOWS_OVERLAY_CALLBACK: std::sync::OnceLock<Weak<WindowsShareOverlay>> =
    std::sync::OnceLock::new();
```

Add `state: Mutex<OverlayState>` and `hook: Mutex<Option<HWINEVENTHOOK>>` to `WindowsShareOverlay`.

In `Default`, initialize them.

- [ ] **Step 2: Register and handle events**

Add methods:

```rust
fn install_event_hook(self: &Arc<Self>) {
    let _ = WINDOWS_OVERLAY_CALLBACK.set(Arc::downgrade(self));
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_MOVESIZESTART,
            EVENT_OBJECT_DESTROY,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if hook.0 != 0 {
        *self.hook.lock().expect("overlay hook lock") = Some(hook);
    }
}

fn handle_window_event(&self, event: u32, hwnd: HWND, object_id: i32) {
    if object_id != OBJID_WINDOW.0 {
        return;
    }
    let Some(target) = self.state.lock().expect("overlay state lock").target.clone() else {
        return;
    };
    let Some(target_hwnd) = windows_target_handle_from_source_id(&target.source_id) else {
        return;
    };
    if hwnd.0 != target_hwnd {
        return;
    }

    match event {
        EVENT_SYSTEM_MOVESIZESTART => {
            self.state.lock().expect("overlay state lock").moving_or_sizing = true;
            for window in self.windows.lock().expect("overlay window lock").iter() {
                window.hide();
            }
        }
        EVENT_SYSTEM_MOVESIZEEND => {
            self.state.lock().expect("overlay state lock").moving_or_sizing = false;
            if let Some(rect) = window_bounds(&target.source_id) {
                let _ = self.show_rect(rect);
            }
        }
        EVENT_OBJECT_HIDE => {
            for window in self.windows.lock().expect("overlay window lock").iter() {
                window.hide();
            }
        }
        EVENT_OBJECT_SHOW | EVENT_OBJECT_LOCATIONCHANGE => {
            if !self.state.lock().expect("overlay state lock").moving_or_sizing {
                if let Some(rect) = window_bounds(&target.source_id) {
                    let _ = self.show_rect(rect);
                }
            }
        }
        EVENT_OBJECT_DESTROY => {
            self.destroy_all();
        }
        _ => {}
    }
}
```

Add callback:

```rust
extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object_id: i32,
    _child_id: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    let Some(overlay) = WINDOWS_OVERLAY_CALLBACK
        .get()
        .and_then(|weak| weak.upgrade())
    else {
        return;
    };
    overlay.handle_window_event(event, hwnd, object_id);
}
```

In `stop`, unhook:

```rust
if let Some(hook) = self.hook.lock().expect("overlay hook lock").take() {
    unsafe {
        let _ = UnhookWinEvent(hook);
    }
}
```

- [ ] **Step 3: Compile-check**

Run:

```powershell
cargo check
```

Expected: PASS.

- [ ] **Step 4: Manual Windows check**

Run the example app, share a window, then move and resize it.

Expected:

- Green corner border appears around target window.
- Border hides while dragging or resizing.
- Border reappears after drag/resize ends.
- Border hides if the target window is minimized.

- [ ] **Step 5: Commit Task 5**

```powershell
git add Cargo.toml src/overlay/windows.rs
git commit -m "Track Windows window events for share border"
```

## Task 6: Display Bounds

**Files:**
- Modify: `src/overlay/windows.rs`
- Test: `cargo check`

- [ ] **Step 1: Implement display source-id parsing**

Add to `src/overlay/windows.rs`:

```rust
fn display_index_from_source_id(source_id: &str) -> Option<usize> {
    source_id
        .rsplit_once(':')
        .and_then(|(_, index)| index.parse::<usize>().ok())
        .and_then(|index| index.checked_sub(1))
}

fn display_bounds(source_id: &str) -> Option<OverlayRect> {
    let index = display_index_from_source_id(source_id)?;
    let monitor = windows_capture::monitor::Monitor::from_index(index + 1).ok()?;
    let mut info = windows::Win32::Graphics::Gdi::MONITORINFO {
        cbSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>() as u32,
        ..Default::default()
    };
    let ok = unsafe {
        windows::Win32::Graphics::Gdi::GetMonitorInfoW(
            windows::Win32::Foundation::HMONITOR(monitor.as_raw_hmonitor()),
            &mut info,
        )
        .as_bool()
    };
    if !ok {
        return None;
    }
    Some(OverlayRect {
        left: info.rcMonitor.left,
        top: info.rcMonitor.top,
        right: info.rcMonitor.right,
        bottom: info.rcMonitor.bottom,
    })
}
```

Update `start`:

```rust
async fn start(&self, target: OverlayTarget) -> Result<()> {
    self.state.lock().expect("overlay state lock").target = Some(target.clone());
    match target.source_kind {
        crate::CaptureSourceKind::Window => {
            if let Some(rect) = window_bounds(&target.source_id) {
                self.show_rect(rect)?;
            }
        }
        crate::CaptureSourceKind::Display => {
            if let Some(rect) = display_bounds(&target.source_id) {
                self.show_rect(rect)?;
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Compile-check**

Run:

```powershell
cargo check
```

Expected: PASS.

- [ ] **Step 3: Manual Windows display check**

Run the example app and share a display.

Expected:

- Green corner border appears on the selected display only.
- Overlay does not appear in the shared stream.
- Overlay does not block clicks.

- [ ] **Step 4: Commit Task 6**

```powershell
git add src/overlay/windows.rs
git commit -m "Show share border for display captures"
```

## Task 7: Wire Real Overlay into Default State

**Files:**
- Modify: `src/state.rs`
- Modify: `src/overlay/mod.rs`
- Test: `cargo test`

- [ ] **Step 1: Select the real Windows overlay by default**

In `src/overlay/mod.rs`, add:

```rust
pub fn default_share_overlay() -> std::sync::Arc<dyn ShareOverlay> {
    #[cfg(target_os = "windows")]
    {
        std::sync::Arc::new(WindowsShareOverlay::default())
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::sync::Arc::new(NoopShareOverlay)
    }
}
```

In `src/state.rs`, replace `Arc::new(NoopShareOverlay)` with `crate::overlay::default_share_overlay()`.

- [ ] **Step 2: Run all tests**

Run:

```powershell
cargo test
```

Expected: PASS.

- [ ] **Step 3: Manual end-to-end verification**

Run the example Tauri app on Windows.

Expected:

- Refreshing sources no longer flashes a Windows yellow border.
- Starting display sharing shows green corner border on the selected display.
- Starting window sharing shows green corner border around the selected window.
- Moving or resizing the shared window hides the border until the operation ends.
- The border is not visible in the local preview video.
- Stop capture destroys all border windows.

- [ ] **Step 4: Commit Task 7**

```powershell
git add src/overlay/mod.rs src/state.rs
git commit -m "Enable Windows share border overlay by default"
```

## Self-Review

Spec coverage:

- Local green indicator: Tasks 1, 3, and 7.
- Corner-only style: Task 1.
- Display sharing bounds: Task 6.
- Window outer bounds: Task 4.
- Hide during move/resize and reshow after: Task 5.
- Exclude overlay from capture: Task 3.
- Session lifecycle integration: Task 2 and Task 7.
- Manual Windows verification: Tasks 5, 6, and 7.

No placeholders remain. Type names used across tasks are consistent: `OverlayStyle`, `OverlayRect`, `OverlaySegment`, `OverlayTarget`, `ShareOverlay`, `WindowsShareOverlay`.
