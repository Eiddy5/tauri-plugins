# macOS Screen Annotation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 macOS 14+ 的显示器共享和单窗口共享中提供画笔、橡皮擦、撤销、清空、颜色和粗细控制，并通过 Metal 将标注零 CPU readback 地合成进发送视频。

**Architecture:** `AnnotationSession` 保存平台无关的归一化矢量操作；现有 macOS 全尺寸 Overlay 复用同一场景进行本地渲染和鼠标输入；ScreenCaptureKit 保留 IOSurface-backed `CVPixelBuffer`，由 latest-frame scheduler 驱动 Metal compositor 生成 `MacGpuSurface`，再由异步 VideoToolbox worker 编码并有序交给 WebRTC。

**Tech Stack:** Rust 1.77.2、Tauri 2、ScreenCaptureKit 3.0、AppKit/QuartzCore、CoreVideo、Metal、VideoToolbox、Tokio、TypeScript/Rollup。

**Approved spec:** `docs/superpowers/specs/2026-07-16-macos-screen-annotation-design.md`

---

## Execution prerequisites

- 在 `codex/macos-screen-annotation` 独立 worktree 中执行，不直接在 `main` 上实现。
- 每个任务按测试失败、最小实现、测试通过、提交的顺序完成。
- 正常媒体路径禁止调用 `CVPixelBufferLockBaseAddress` 做整帧 readback。
- 编码前允许 latest-frame 替换；VideoToolbox 提交后和 H.264 编码后禁止覆盖或乱序。
- 所有提交使用中文 Conventional Commits。

## Planned file structure

```text
src/
  annotation/
    mod.rs                    # AnnotationSession facade and revision notifications
    model.rs                  # points, strokes, tools, operations and snapshots
    history.rs                # limits, sampling, simplification, undo and clear
    geometry.rs               # overlay/source/output coordinate transforms
  overlay/macos/
    annotation_view.rs        # native pointer input
    annotation_render.rs      # CAMetalLayer adapter
    host.rs                   # existing session overlay owns annotation state
    panel.rs                  # panel/layer/hit-testing configuration
  platform/macos/media/
    mod.rs
    frame.rs                  # MacCaptureFrame and MacGpuSurface
    surface_pool.rs           # fixed Metal-compatible CVPixelBuffer pool
    metal_device.rs           # device, queue, texture cache and pipelines
    tessellator.rs            # stroke-to-triangle conversion
    metal_compositor.rs       # source/annotation composition
    scheduler.rs              # latest source + revision + cadence
    telemetry.rs
    shaders.metal
  webrtc/
    h264_encoder_macos.rs     # asynchronous VideoToolbox worker
tests/
  annotation_model.rs
  annotation_geometry.rs
  macos_metal_surface.rs
  macos_annotation_compositor.rs
  macos_frame_scheduler.rs
  macos_encoder_worker.rs
  macos_annotation_overlay.rs
  macos_annotation_end_to_end.rs
```

## Task 1: Add the public annotation and telemetry contracts

**Files:**

- Modify: `src/models.rs`
- Modify: `tests/model_contract.rs`
- Modify: `guest-js/index.ts`
- Modify: `tests/guest-js-contract.ts`

- [ ] **Step 1: Write failing Rust contract tests**

Append tests that lock the JSON shape and backward-compatible defaults:

```rust
#[test]
fn annotation_tool_uses_a_tagged_camel_case_contract() {
    let pen: AnnotationTool = serde_json::from_value(serde_json::json!({
        "kind": "pen",
        "color": "#FF3B30",
        "width": 4.0
    }))
    .expect("deserialize pen");
    assert_eq!(
        pen,
        AnnotationTool::Pen {
            color: "#FF3B30".into(),
            width: 4.0,
        }
    );

    let eraser = serde_json::to_value(AnnotationTool::Eraser { width: 24.0 })
        .expect("serialize eraser");
    assert_eq!(eraser["kind"], "eraser");
    assert_eq!(eraser["width"], 24.0);
}

#[test]
fn legacy_capabilities_and_stats_default_annotation_fields() {
    let capabilities: Capabilities = serde_json::from_value(serde_json::json!({
        "platform": "macos",
        "supportsDisplayCapture": true,
        "supportsWindowCapture": true,
        "supportsThumbnails": true,
        "supportsCursorCapture": true,
        "supportsWebrtc": true
    }))
    .expect("legacy capabilities");
    assert!(!capabilities.supports_annotations);
    assert!(capabilities.annotation_tools.is_empty());

    let stats: CaptureStats = serde_json::from_value(serde_json::json!({
        "framesCaptured": 1,
        "framesPublished": 1,
        "framesDropped": 0,
        "fps": 30.0,
        "bitrateKbps": 4000,
        "started": true
    }))
    .expect("legacy stats");
    assert_eq!(stats.frames_composited, 0);
    assert_eq!(stats.frames_gpu_backpressure_dropped, 0);
    assert_eq!(stats.annotation_revision, 0);
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
cargo test --test model_contract annotation -- --nocapture
```

Expected: compile failure because `AnnotationTool` and the new fields do not exist.

- [ ] **Step 3: Add the Rust wire types**

Add these shapes to `src/models.rs`, using `#[serde(default)]` on every additive field in `Capabilities` and `CaptureStats`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnnotationToolKind {
    Pen,
    Eraser,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AnnotationTool {
    Pen { color: String, width: f32 },
    Eraser { width: f32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationState {
    pub interaction_enabled: bool,
    pub tool: AnnotationTool,
    pub can_undo: bool,
    pub operation_count: usize,
    pub revision: u64,
    pub last_error: Option<CaptureErrorPayload>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationStateChangedEvent {
    pub session_id: String,
    pub state: AnnotationState,
}

pub const ANNOTATION_STATE_CHANGED_EVENT: &str =
    "screen-capture://annotation-state-changed";
```

Add the four annotation error variants to `CaptureErrorCode` and the seven approved telemetry fields to `CaptureStats`.

- [ ] **Step 4: Add the TypeScript contracts and compile-time usage test**

Add matching `AnnotationToolKind`, `AnnotationTool`, `AnnotationState`, `AnnotationStateChangedEvent`, capability fields and stats fields to `guest-js/index.ts`. Extend `tests/guest-js-contract.ts` with:

```ts
import {
  type AnnotationState,
  type AnnotationStateChangedEvent,
} from "../guest-js/index"

const annotation: AnnotationState = {
  interactionEnabled: false,
  tool: { kind: "pen", color: "#FF3B30", width: 4 },
  canUndo: false,
  operationCount: 0,
  revision: 0,
  lastError: null,
}

const changed: AnnotationStateChangedEvent = {
  sessionId: "session-1",
  state: annotation,
}

void changed
```

- [ ] **Step 5: Run contract verification**

Run:

```bash
cargo test --test model_contract
pnpm build
```

Expected: both commands pass.

- [ ] **Step 6: Commit**

```bash
git add src/models.rs guest-js/index.ts tests/model_contract.rs tests/guest-js-contract.ts
git commit -m "feat: 添加屏幕标注公共数据契约"
```

## Task 2: Implement the platform-independent annotation scene

**Files:**

- Create: `src/annotation/mod.rs`
- Create: `src/annotation/model.rs`
- Create: `src/annotation/history.rs`
- Modify: `src/lib.rs`
- Create: `tests/annotation_model.rs`

- [ ] **Step 1: Write failing scene tests**

Create tests for the default state, ordered pen/eraser operations, revision changes, undo, clear and input-disabled behavior:

```rust
use tauri_plugin_screen_capture::{
    annotation::{AnnotationSession, NormalizedPoint},
    AnnotationTool,
};

#[test]
fn completed_stroke_is_one_undoable_revision() {
    let session = AnnotationSession::default();
    session.set_interaction_enabled(true).unwrap();
    session
        .begin_stroke(NormalizedPoint::new(0.1, 0.2), 0.01)
        .unwrap();
    session
        .append_point(NormalizedPoint::new(0.4, 0.5))
        .unwrap();
    let state = session.end_stroke().unwrap();

    assert_eq!(state.operation_count, 1);
    assert!(state.can_undo);
    assert_eq!(state.revision, 2); // enable + committed operation
    assert_eq!(session.snapshot().operations().len(), 1);

    let undone = session.undo().unwrap();
    assert_eq!(undone.operation_count, 0);
    assert_eq!(undone.revision, 3);
}

#[test]
fn disabled_interaction_rejects_native_input_without_mutating_scene() {
    let session = AnnotationSession::default();
    let error = session
        .begin_stroke(NormalizedPoint::new(0.5, 0.5), 0.01)
        .unwrap_err();
    assert_eq!(
        error.payload().code,
        tauri_plugin_screen_capture::CaptureErrorCode::AnnotationUnavailable,
    );
    assert_eq!(session.state().revision, 0);
}

#[test]
fn eraser_is_kept_as_an_ordered_operation() {
    let session = AnnotationSession::default();
    session.set_interaction_enabled(true).unwrap();
    session
        .set_tool(AnnotationTool::Eraser { width: 24.0 })
        .unwrap();
    session
        .begin_stroke(NormalizedPoint::new(0.2, 0.2), 0.04)
        .unwrap();
    session.end_stroke().unwrap();
    assert!(session.snapshot().operations()[0].is_eraser());
}
```

- [ ] **Step 2: Verify the new test fails**

Run `cargo test --test annotation_model -- --nocapture`.

Expected: unresolved `annotation` module.

- [ ] **Step 3: Implement immutable operation snapshots**

Use these core internal types:

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedPoint {
    pub x: f32,
    pub y: f32,
}

impl NormalizedPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x: x.clamp(0.0, 1.0), y: y.clamp(0.0, 1.0) }
    }
}

#[derive(Clone, Debug)]
pub enum AnnotationOperation {
    Pen(Stroke),
    Eraser(Stroke),
}

#[derive(Clone, Debug)]
pub struct AnnotationSnapshot {
    revision: u64,
    operations: Arc<[AnnotationOperation]>,
}
```

Implement `AnnotationSession` with `std::sync::RwLock<SceneState>` and `tokio::sync::watch::Sender<u64>`. Mutation methods update state under one write lock, increment revision once per public state change or completed operation, then publish the revision after releasing the lock.

- [ ] **Step 4: Add validation and stable defaults**

Use `Pen { color: "#FF3B30", width: 4.0 }` as the default. Validate colors as exactly `#RRGGBB` or `#RRGGBBAA`, pen width `1.0..=64.0`, eraser width `4.0..=128.0`, and finite normalized widths greater than zero. Return `AnnotationInvalidOptions` without modifying state.

- [ ] **Step 5: Run focused and full tests**

Run:

```bash
cargo test --test annotation_model
cargo test --test model_contract
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/annotation src/lib.rs tests/annotation_model.rs
git commit -m "feat: 实现平台无关标注场景模型"
```

## Task 3: Add sampling, simplification, limits and geometry

**Files:**

- Modify: `src/annotation/history.rs`
- Create: `src/annotation/geometry.rs`
- Modify: `src/annotation/mod.rs`
- Modify: `tests/annotation_model.rs`
- Create: `tests/annotation_geometry.rs`

- [ ] **Step 1: Write failing limit and simplification tests**

Add tests proving near-duplicate points are removed, endpoints survive simplification, and limits fail without corrupting committed operations:

```rust
#[test]
fn simplification_keeps_endpoints_and_visible_corner() {
    let input = vec![
        NormalizedPoint::new(0.0, 0.0),
        NormalizedPoint::new(0.1, 0.001),
        NormalizedPoint::new(0.5, 0.5),
        NormalizedPoint::new(0.9, 0.501),
        NormalizedPoint::new(1.0, 1.0),
    ];
    let output = simplify_points(&input, 0.002);
    assert_eq!(output.first(), input.first());
    assert_eq!(output.last(), input.last());
    assert!(output.contains(&NormalizedPoint::new(0.5, 0.5)));
}
```

- [ ] **Step 2: Write failing geometry tests**

```rust
#[test]
fn normalized_points_map_into_letterboxed_content_rect() {
    let geometry = CaptureGeometry::aspect_fit(
        Size::new(1600.0, 1200.0),
        Size::new(1920.0, 1080.0),
    );
    assert_eq!(geometry.output_content_rect(), Rect::new(240.0, 0.0, 1440.0, 1080.0));
    assert_eq!(geometry.to_output(NormalizedPoint::new(0.0, 0.0)), Point::new(240.0, 0.0));
    assert_eq!(geometry.to_output(NormalizedPoint::new(1.0, 1.0)), Point::new(1680.0, 1080.0));
}

#[test]
fn appkit_overlay_point_maps_to_top_left_normalized_content() {
    let geometry = CaptureGeometry::from_overlay(Rect::new(-1440.0, 0.0, 1440.0, 900.0));
    assert_eq!(geometry.overlay_to_normalized(Point::new(-1440.0, 900.0)), NormalizedPoint::new(0.0, 0.0));
}
```

- [ ] **Step 3: Verify both tests fail**

Run:

```bash
cargo test --test annotation_model simplification
cargo test --test annotation_geometry
```

- [ ] **Step 4: Implement bounded history**

Set constants exactly as approved:

```rust
pub const MAX_POINTS_PER_OPERATION: usize = 8_192;
pub const MAX_OPERATIONS_PER_SESSION: usize = 4_096;
pub const MAX_POINTS_PER_SESSION: usize = 1_000_000;
```

Drop an incoming sample if its distance from the previous retained point is below half a logical output pixel after normalization. On `end_stroke`, apply Ramer-Douglas-Peucker with an epsilon derived from `normalized_width / 8.0`, preserving the first point, last point and visible corners. If a limit would be exceeded, discard only the active operation, leave committed history intact, disable interaction, and return `AnnotationLimitReached`.

- [ ] **Step 5: Implement the geometry value types**

Keep `Point`, `Size`, `Rect` and `CaptureGeometry` free of AppKit types. AppKit-to-top-left inversion belongs in one constructor; output mapping always uses a top-left pixel coordinate system. Reject non-finite or non-positive rectangles with `AnnotationInvalidOptions`.

- [ ] **Step 6: Run tests and commit**

```bash
cargo test --test annotation_model
cargo test --test annotation_geometry
git add src/annotation tests/annotation_model.rs tests/annotation_geometry.rs
git commit -m "feat: 完善标注路径简化与坐标映射"
```

## Task 4: Establish macOS Metal/CoreVideo ownership with a native probe

**Files:**

- Modify: `Cargo.toml`
- Modify: `src/platform/macos/mod.rs`
- Create: `src/platform/macos/media/mod.rs`
- Create: `src/platform/macos/media/frame.rs`
- Create: `src/platform/macos/media/metal_device.rs`
- Create: `src/platform/macos/media/surface_pool.rs`
- Create: `tests/macos_metal_surface.rs`

- [ ] **Step 1: Add an ignored native probe that fails to compile**

```rust
#![cfg(target_os = "macos")]

use tauri_plugin_screen_capture::platform::macos::media::{
    MacMetalDevice, MacSurfacePool,
};

#[test]
#[ignore = "requires a logged-in macOS Metal session"]
fn pool_surfaces_are_iosurface_backed_and_metal_compatible() {
    let device = MacMetalDevice::system_default().expect("Metal device");
    let pool = MacSurfacePool::new(&device, 1920, 1080, 3).expect("surface pool");
    let lease = pool.acquire().expect("surface lease");
    assert_eq!(lease.size(), (1920, 1080));
    assert!(lease.pixel_buffer().io_surface().is_some());
    assert!(device.texture_for_bgra(lease.pixel_buffer()).is_ok());
}
```

- [ ] **Step 2: Add exact framework bindings**

Under the existing macOS target dependencies add compatible `0.3` objc2 bindings with only required features:

```toml
objc2-core-video = { version = "0.3", default-features = false, features = [
  "std",
  "CVBuffer",
  "CVImageBuffer",
  "CVMetalTexture",
  "CVMetalTextureCache",
  "CVPixelBuffer",
  "CVPixelBufferPool",
  "CVReturn",
  "objc2",
  "objc2-metal",
] }
objc2-metal = { version = "0.3", default-features = false, features = [
  "std",
  "MTLBuffer",
  "MTLCommandBuffer",
  "MTLCommandQueue",
  "MTLDevice",
  "MTLLibrary",
  "MTLPixelFormat",
  "MTLRenderCommandEncoder",
  "MTLRenderPass",
  "MTLRenderPipeline",
  "MTLResource",
  "MTLTexture",
] }
```

Add `"CAMetalLayer"` and `"objc2-metal"` to `objc2-quartz-core` features. Enable the `iosurface` feature on `apple-cf` in addition to the existing `cm` and `cv` features. These feature names must be checked against the resolved `0.3.x` crate manifests before updating `Cargo.lock`; do not fall back to each crate's broad default feature set.

- [ ] **Step 3: Implement owned frame and surface wrappers**

Define:

```rust
use apple_cf::cv::CVPixelBuffer;

#[derive(Clone)]
pub struct MacCaptureFrame {
    pixel_buffer: CVPixelBuffer,
    timestamp_ns: u64,
    geometry: CaptureFrameGeometry,
    generation: u64,
}

#[derive(Clone)]
pub struct MacGpuSurface {
    inner: Arc<MacGpuSurfaceInner>,
    width: u32,
    height: u32,
    timestamp_ns: u64,
}
```

Use `apple_cf::cv::CVPixelBuffer` as the only owning PixelBuffer type because ScreenCaptureKit and the existing VideoToolbox bridge already use it. `metal_device.rs` contains the single audited unsafe interop seam: it either borrows an objc2 CoreVideo view while the canonical Apple-CF buffer is retained, or transfers a newly created objc2 pool buffer into the Apple-CF owner without double-retaining it. Add focused retain/release tests to the ignored native probe; do not let both wrapper types own the same +1 reference.

`MacGpuSurfaceInner` owns the `SurfaceLease`, not just a cloned PixelBuffer, plus every `CVMetalTexture` reference that must remain alive through GPU and encoder use. The slot returns to the pool only when the last cloned `MacGpuSurface` drops. Do not expose raw CoreVideo or Metal pointers outside `platform::macos::media`.

- [ ] **Step 4: Implement a fixed three-surface pool**

Create the pool with BGRA, `kCVPixelBufferMetalCompatibilityKey = true`, and an empty `kCVPixelBufferIOSurfacePropertiesKey` dictionary. A `SurfaceLease` returns its slot to `Free` on drop. State transitions are `Free -> InUse -> Ready -> InUse/Free`; acquiring when no free/ready-reclaimable slot returns a typed backpressure result rather than allocating a fourth buffer.

- [ ] **Step 5: Run the probe and normal checks**

```bash
cargo test --features macos-screencapturekit --test macos_metal_surface -- --ignored --nocapture
cargo test --features macos-screencapturekit --no-run
```

Expected: the native probe passes and all targets compile.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/platform/macos tests/macos_metal_surface.rs
git commit -m "build: 建立 macOS Metal 媒体资源探针"
```

## Task 5: Replace the synchronous macOS encoder with an asynchronous VideoToolbox worker

**Files:**

- Create: `src/webrtc/h264_encoder_macos.rs`
- Modify: `src/webrtc/h264_encoder.rs`
- Modify: `src/publisher/webrtc.rs`
- Modify: `src/publisher/mod.rs`
- Modify: `src/publisher/composite.rs`
- Create: `tests/macos_encoder_worker.rs`

- [ ] **Step 1: Write failing worker contract tests**

Add pure tests around a fake VideoToolbox adapter so they run without encoding hardware:

```rust
#[test]
fn encoded_handoff_preserves_submission_order() {
    let handoff = OrderedSampleHandoff::new(1);
    handoff.push(sample(1)).unwrap();
    assert_eq!(handoff.pop().unwrap().timestamp_ns, 1);
    handoff.push(sample(2)).unwrap();
    assert_eq!(handoff.pop().unwrap().timestamp_ns, 2);
}

#[test]
fn keyframe_request_is_consumed_by_exactly_one_submission() {
    let state = EncoderControlState::default();
    state.request_keyframe();
    assert!(state.take_force_keyframe());
    assert!(!state.take_force_keyframe());
}
```

- [ ] **Step 2: Add macOS GPU support to the publisher interface**

Add cfg-gated defaults parallel to the existing Windows surface method:

```rust
#[cfg(target_os = "macos")]
fn supports_mac_gpu_surfaces(&self) -> bool { false }

#[cfg(target_os = "macos")]
async fn push_mac_gpu_surface(&self, _surface: MacGpuSurface) -> Result<()> {
    Err(Error::new(
        CaptureErrorCode::PublisherUnsupported,
        "publisher does not support macOS GPU surfaces",
        true,
    ))
}
```

`CompositePublisher` reports support only when every child supports it and forwards a cloned surface to every child.

- [ ] **Step 3: Implement `MacEncoderWorker`**

The worker owns one serial media thread/queue, `VTCompressionSession`, keyframe flag, bounded pre-encode latest slot, ordered encoded handoff and telemetry. Configure:

```text
EnableHardwareAcceleratedVideoEncoder = true
RealTime = true
ExpectedFrameRate = configured fps
MaxFrameDelayCount = 1
AllowFrameReordering = false
MaxKeyFrameInterval = fps * 2
ProfileLevel = H264_Baseline_AutoLevel
```

Submit `MacGpuSurface::pixel_buffer()` directly to `VTCompressionSessionEncodeFrame`. Add `kVTEncodeFrameOptionKey_ForceKeyFrame` only when `take_force_keyframe()` returns true. Move AVCC-to-Annex-B conversion and SPS/PPS extraction from the old inline module without changing their byte contract.

- [ ] **Step 4: Remove per-frame draining**

Delete the per-frame `VTCompressionSessionCompleteFrames` call. Call it only from explicit `pause_and_drain`, `stop`, `reconfigure` and `Drop`. The output callback must publish samples in callback order; it must never use a latest slot.

- [ ] **Step 5: Wire `WebRtcPublisher` to `MacEncoderWorker`**

On macOS, `supports_mac_gpu_surfaces()` returns true. `push_mac_gpu_surface` submits to the worker and returns without waiting for encoding or transport. `request_keyframe` sets the worker flag. Stats read worker telemetry, including hardware backend ID and CPU readback count.

- [ ] **Step 6: Run tests and commit**

```bash
cargo test --features macos-screencapturekit --test macos_encoder_worker
cargo test --features macos-screencapturekit --test h264_annexb
cargo test --features macos-screencapturekit --test webrtc
git add src/webrtc src/publisher tests/macos_encoder_worker.rs
git commit -m "perf: 重构 macOS VideoToolbox 异步编码"
```

## Task 6: Deliver native ScreenCaptureKit frames without CPU copies

**Files:**

- Modify: `src/capture/mod.rs`
- Modify: `src/platform/macos/screencapturekit.rs`
- Modify: `src/platform/macos/media/frame.rs`
- Modify: `tests/pipeline.rs`
- Create: `tests/macos_capture_frame.rs`

- [ ] **Step 1: Write failing frame metadata tests**

```rust
#[test]
fn frame_geometry_uses_screen_capture_attachments() {
    let geometry = CaptureFrameGeometry::from_attachments(
        1920,
        1080,
        Some(FrameRect::new(240.0, 0.0, 1440.0, 1080.0)),
        Some(2.0),
        Some(1.0),
    )
    .unwrap();
    assert_eq!(geometry.output_content_rect().x, 240.0);
    assert_eq!(geometry.scale_factor(), 2.0);
}
```

- [ ] **Step 2: Extend `FrameConsumer` with a macOS native frame method**

Add cfg-gated `supports_mac_capture_frames` and `push_mac_capture_frame` methods with safe unsupported defaults. Keep the existing CPU `push_frame` method for dummy, tests and non-Metal adapters.

- [ ] **Step 3: Replace `video_frame_from_sample`**

Replace the CPU conversion with `mac_capture_frame_from_sample`:

```rust
fn mac_capture_frame_from_sample(
    sample: &CMSampleBuffer,
    generation: u64,
) -> FrameConversion {
    if sample.frame_status() != Some(SCFrameStatus::Complete) {
        return FrameConversion::Skipped(FrameSkipReason::NoImageContent(sample.frame_status()));
    }
    let Some(pixel_buffer) = sample.image_buffer() else {
        return FrameConversion::Skipped(FrameSkipReason::MissingImageBuffer(sample.frame_status()));
    };
    let Ok(geometry) = CaptureFrameGeometry::from_sample(sample, &pixel_buffer) else {
        return FrameConversion::Skipped(FrameSkipReason::InvalidGeometry);
    };
    FrameConversion::Frame(MacCaptureFrame::new(
        pixel_buffer,
        sample.display_time().unwrap_or_default(),
        geometry,
        generation,
    ))
}
```

Do not call `pixel_buffer.lock`, `as_slice`, or `tightly_packed_bgra` in this path. The callback calls `try_push`/latest replacement and returns immediately.

- [ ] **Step 4: Preserve observability**

Count complete frames, idle frames, missing buffers, latest replacements and invalid geometry separately. Remove per-frame `eprintln!`; keep first-occurrence and periodic lifecycle summaries.

- [ ] **Step 5: Verify no-copy behavior and commit**

```bash
rg -n "lock\(|as_slice\(|tightly_packed_bgra" src/platform/macos/screencapturekit.rs
cargo test --features macos-screencapturekit --test macos_capture_frame
cargo test --features macos-screencapturekit --test pipeline
git add src/capture src/platform/macos tests/macos_capture_frame.rs tests/pipeline.rs
git commit -m "perf: 保留 ScreenCaptureKit 原生像素缓冲区"
```

Expected `rg`: no match in the real-time frame conversion path.

## Task 7: Implement Metal stroke tessellation and composition

**Files:**

- Create: `src/platform/macos/media/tessellator.rs`
- Create: `src/platform/macos/media/shaders.metal`
- Create: `src/platform/macos/media/metal_compositor.rs`
- Modify: `src/platform/macos/media/metal_device.rs`
- Modify: `src/platform/macos/media/mod.rs`
- Create: `tests/macos_annotation_compositor.rs`

- [ ] **Step 1: Write pure tessellation tests**

Test a two-point horizontal stroke, a corner, round end caps, normalized width scaling and eraser mesh tagging. Assert finite vertices, expected bounds and consistent winding; do not assert opaque implementation-specific vertex counts.

```rust
#[test]
fn horizontal_stroke_mesh_covers_the_requested_width() {
    let mesh = tessellate(&stroke((0.1, 0.5), (0.9, 0.5), 0.1), Size::new(100.0, 100.0));
    let bounds = mesh.bounds();
    assert_eq!(bounds.min_y, 45.0);
    assert_eq!(bounds.max_y, 55.0);
    assert!(mesh.vertices().iter().all(|vertex| vertex.position.iter().all(|v| v.is_finite())));
}
```

- [ ] **Step 2: Write an ignored Metal golden test**

Use a deterministic 64×64 checkerboard source and a snapshot containing one red pen operation followed by one eraser operation. Render to a pooled PixelBuffer, read back only in this test, save no files, and assert selected pixels with a tolerance of 2 per channel.

- [ ] **Step 3: Add the shader source**

`shaders.metal` contains a full-screen source pass and a colored stroke pass:

```metal
#include <metal_stdlib>
using namespace metal;

struct VertexOut {
    float4 position [[position]];
    float2 uv;
    float4 color;
};

struct SourceVertex {
    float2 position;
    float2 uv;
};

struct StrokeVertex {
    float2 position;
    float4 color;
};

vertex VertexOut source_vertex(const device SourceVertex* vertices [[buffer(0)]],
                               uint vertex_id [[vertex_id]]) {
    SourceVertex input = vertices[vertex_id];
    VertexOut output;
    output.position = float4(input.position, 0.0, 1.0);
    output.uv = input.uv;
    output.color = float4(1.0);
    return output;
}

vertex VertexOut stroke_vertex(const device StrokeVertex* vertices [[buffer(0)]],
                               uint vertex_id [[vertex_id]]) {
    StrokeVertex input = vertices[vertex_id];
    VertexOut output;
    output.position = float4(input.position, 0.0, 1.0);
    output.uv = float2(0.0);
    output.color = input.color;
    return output;
}

fragment float4 source_fragment(VertexOut in [[stage_in]],
                                texture2d<float> source [[texture(0)]],
                                sampler source_sampler [[sampler(0)]]) {
    return source.sample(source_sampler, in.uv);
}

fragment float4 stroke_fragment(VertexOut in [[stage_in]]) {
    return float4(in.color.rgb * in.color.a, in.color.a);
}
```

Compile once per Metal device and cache both pen and eraser pipeline states. Configure premultiplied-alpha blending for pen and destination-out blending for eraser.

- [ ] **Step 4: Implement annotation texture caching**

Cache key: `(annotation_revision, output_width, output_height)`. Rebuild the transparent annotation texture only when the key changes. Replay ordered operations into the texture. Reuse the cached texture for ordinary source frames.

- [ ] **Step 5: Implement source + annotation composition**

`MacMetalCompositor::compose` acquires an output lease, maps source/output PixelBuffers through one `CVMetalTextureCache`, clears letterbox regions to opaque black, draws source into `geometry.output_content_rect`, blends the cached annotation texture, commits, and completes through a callback that produces `MacGpuSurface`. Retain every CoreVideo/Metal object until the completion callback fires.

- [ ] **Step 6: Run tests and commit**

```bash
cargo test --features macos-screencapturekit tessellator
cargo test --features macos-screencapturekit --test macos_annotation_compositor -- --ignored --nocapture
git add src/platform/macos/media tests/macos_annotation_compositor.rs
git commit -m "feat: 实现 Metal 标注合成器"
```

## Task 8: Implement latest-frame scheduling, cadence and GPU backpressure

**Files:**

- Create: `src/platform/macos/media/scheduler.rs`
- Create: `src/platform/macos/media/telemetry.rs`
- Modify: `src/platform/macos/media/mod.rs`
- Modify: `src/pipeline/mod.rs`
- Modify: `src/pipeline/stats.rs`
- Create: `tests/macos_frame_scheduler.rs`

- [ ] **Step 1: Write deterministic policy tests with a fake clock**

Cover these exact decisions:

```rust
#[test]
fn annotation_revision_wakes_a_static_scene_immediately() {
    let mut policy = SchedulerPolicy::new(60, 5, Duration::ZERO);
    policy.observe_source(1, Duration::ZERO);
    assert_eq!(policy.next(Duration::ZERO), SchedulerDecision::Compose { source_generation: 1 });
    policy.mark_composed(1, 0, Duration::ZERO);
    policy.observe_annotation_revision(2);
    assert_eq!(
        policy.next(Duration::from_millis(1)),
        SchedulerDecision::Compose { source_generation: 1 }
    );
}

#[test]
fn missed_deadline_does_not_burst_catch_up_frames() {
    let mut policy = SchedulerPolicy::new(60, 5, Duration::ZERO);
    policy.mark_composed(1, 0, Duration::ZERO);
    let late = Duration::from_millis(200);
    policy.mark_composed(1, 0, late);
    assert!(policy.deadline() >= late + Duration::from_millis(16));
}
```

Also test latest source replacement, one pending request, static 5 FPS, transition back to target FPS, surface exhaustion and generation invalidation.

- [ ] **Step 2: Implement `SchedulerPolicy` as pure logic**

The policy receives source generations, annotation revisions, completion timestamps and pool availability. It returns only `Wait(deadline)`, `Compose`, `RepeatLast`, `DropBackpressure`, or `Stop`. This makes cadence testable without Metal or Tokio.

- [ ] **Step 3: Implement the serial scheduler worker**

The worker owns the latest retained source, latest annotation snapshot, compositor, deadline timer, generation and a one-item pending wakeup. It never blocks the ScreenCaptureKit callback. On the first source/annotation change after static mode, request one keyframe before submitting the composed surface.

- [ ] **Step 4: Integrate with `CapturePipeline`**

On macOS, construct the pipeline with `AnnotationSession` and `StartCaptureOptions`. `push_mac_capture_frame` delegates to the scheduler. Scheduler output calls `publisher.push_mac_gpu_surface`. Existing CPU and Windows paths retain their behavior.

- [ ] **Step 5: Merge telemetry**

Update approved stats fields on the owning stage. `framesDropped` remains the sum of capture, pipeline/GPU and encoder drops. Static coalescing increments `framesStaticRepeated` and is not counted as an abnormal drop.

- [ ] **Step 6: Run tests and commit**

```bash
cargo test --features macos-screencapturekit --test macos_frame_scheduler
cargo test --features macos-screencapturekit --test pipeline
git add src/platform/macos/media src/pipeline tests/macos_frame_scheduler.rs
git commit -m "perf: 添加 macOS 最新帧调度与背压控制"
```

## Task 9: Reuse the native macOS overlay for direct annotation input

**Files:**

- Modify: `src/overlay/mod.rs`
- Modify: `src/overlay/macos/mod.rs`
- Modify: `src/overlay/macos/host.rs`
- Modify: `src/overlay/macos/panel.rs`
- Create: `src/overlay/macos/annotation_view.rs`
- Create: `src/overlay/macos/annotation_render.rs`
- Modify: `src/overlay/macos/events.rs`
- Create: `tests/macos_annotation_overlay.rs`

- [ ] **Step 1: Change the factory contract with failing lifecycle tests**

Change `ShareOverlayFactory::create_overlay` to accept `Arc<AnnotationSession>` and update the macOS, Windows, no-op and test adapters. Add tests proving two capture sessions receive different annotation sessions and that pause hides input without clearing operations.

- [ ] **Step 2: Add a native ignored interaction test**

The test creates a target window and real overlay, then verifies:

- interaction disabled: a synthetic click reaches the target;
- interaction enabled: the overlay commits one annotation operation and the target click count stays unchanged;
- moving the target hides the overlay and rejects input;
- disabling interaction restores click-through;
- stop removes the native window and event monitors.

- [ ] **Step 3: Implement the custom input view**

Use `objc2::define_class!` to subclass `NSView`. Keep only an overlay/session ID in Objective-C ivars; resolve the Rust `Arc<AnnotationSession>` through the existing main-thread registry. Implement `mouseDown:`, `mouseDragged:` and `mouseUp:`. Convert AppKit bottom-left points through `CaptureGeometry::overlay_to_normalized` before calling native input methods.

Do not install a global event tap and do not request Accessibility or Input Monitoring permission.

- [ ] **Step 4: Add the local CAMetalLayer renderer**

Attach one transparent `CAMetalLayer` under the existing green corner CALayers. Reuse the compositor tessellator and annotation snapshot. Set drawable size from `backingScaleFactor`; redraw only on revision/geometry changes. Keep corner layers visible locally and keep the whole panel registered in the current ScreenCaptureKit exclusion registry.

- [ ] **Step 5: Preserve existing window ordering semantics**

Display overlay remains topmost on the selected display. Window overlay remains at the target layer and immediately above the target while respecting occluding windows. During target movement/resize, hide the full panel, suspend input, then restore geometry and ordering on the first stable sample.

- [ ] **Step 6: Run overlay regressions and commit**

```bash
cargo test --features macos-screencapturekit --test overlay_geometry
cargo test --features macos-screencapturekit --test macos_overlay_policy
cargo test --features macos-screencapturekit --test macos_annotation_overlay -- --ignored --nocapture
git add src/overlay tests/macos_annotation_overlay.rs tests/session_state.rs
git commit -m "feat: 支持 macOS 原生屏幕标注交互"
```

## Task 10: Wire session lifecycle, commands, events and permissions

**Files:**

- Modify: `src/state.rs`
- Modify: `src/commands.rs`
- Modify: `src/lib.rs`
- Modify: `build.rs`
- Modify: `guest-js/index.ts`
- Modify: `permissions/default.toml`
- Regenerate: `permissions/autogenerated/`
- Regenerate: `permissions/schemas/schema.json`
- Modify: `tests/session_state.rs`
- Modify: `tests/guest-js-contract.ts`

- [ ] **Step 1: Write failing state tests**

Add tests for all five commands, event emission, pause/resume retention, independent concurrent sessions, invalid sessions, invalid tool rollback, start rollback and passive source close cleanup:

```rust
#[tokio::test]
async fn state_keeps_annotations_across_pause_and_resume() {
    let state = ScreenCaptureState::with_backend(Arc::new(DummyCaptureBackend));
    let session = state.start_capture(start_options()).await.unwrap();
    state.set_annotation_interaction(&session.session_id, true).await.unwrap();
    state.pause_capture(&session.session_id).await.unwrap();
    state.resume_capture(&session.session_id).await.unwrap();
    assert!(state.get_annotation_state(&session.session_id).await.unwrap().interaction_enabled);
}

#[tokio::test]
async fn invalid_tool_does_not_replace_the_previous_tool() {
    let state = ScreenCaptureState::with_backend(Arc::new(DummyCaptureBackend));
    let session = state.start_capture(start_options()).await.unwrap();
    let before = state.get_annotation_state(&session.session_id).await.unwrap();
    assert!(state
        .set_annotation_tool(
            &session.session_id,
            AnnotationTool::Pen { color: "red".into(), width: 0.0 },
        )
        .await
        .is_err());
    assert_eq!(state.get_annotation_state(&session.session_id).await.unwrap().tool, before.tool);
}
```

- [ ] **Step 2: Reorder capture startup and rollback**

Generate `session_id` first, create `AnnotationSession`, publisher, scheduler/pipeline and overlay, start publisher, start scheduler, start backend capture, start overlay, then insert `SessionRecord`. Every failure path calls already-started resources in reverse order. Store `annotation_session` and scheduler handle in `SessionRecord`.

- [ ] **Step 3: Add state methods and Tauri commands**

Implement exact commands:

```rust
set_annotation_interaction(session_id, enabled) -> AnnotationState
set_annotation_tool(session_id, tool) -> AnnotationState
undo_annotation(session_id) -> AnnotationState
clear_annotations(session_id) -> AnnotationState
get_annotation_state(session_id) -> AnnotationState
```

State mutation emits `AnnotationStateChangedEvent` after the lock is released. Extend `CaptureEventSink` with `emit_annotation_state_changed`; update the no-op and Tauri adapters.

- [ ] **Step 4: Add Guest JS functions and listener**

```ts
export function setAnnotationInteraction(sessionId: string, enabled: boolean) {
  return invoke<AnnotationState>(command("set_annotation_interaction"), { sessionId, enabled })
}

export function setAnnotationTool(sessionId: string, tool: AnnotationTool) {
  return invoke<AnnotationState>(command("set_annotation_tool"), { sessionId, tool })
}

export function undoAnnotation(sessionId: string) {
  return invoke<AnnotationState>(command("undo_annotation"), { sessionId })
}

export function clearAnnotations(sessionId: string) {
  return invoke<AnnotationState>(command("clear_annotations"), { sessionId })
}

export function getAnnotationState(sessionId: string) {
  return invoke<AnnotationState>(command("get_annotation_state"), { sessionId })
}
```

Add `onAnnotationStateChanged` using the approved event constant.

- [ ] **Step 5: Generate permissions**

Add the five snake_case commands to `build.rs`, register them in `lib.rs`, add all five `allow-*` permissions to `permissions/default.toml`, then run:

```bash
cargo build --features macos-screencapturekit
```

Verify generated permission TOML files and schemas contain all commands.

- [ ] **Step 6: Run tests and commit**

```bash
cargo test --features macos-screencapturekit --test session_state
cargo test --features macos-screencapturekit --test model_contract
pnpm build
git add src/state.rs src/commands.rs src/lib.rs build.rs guest-js permissions tests/session_state.rs tests/guest-js-contract.ts
git commit -m "feat: 接入标注会话命令与事件"
```

## Task 11: Add the example annotation toolbar

**Files:**

- Modify: `examples/tauri-app/src/main.js`
- Modify: `examples/tauri-app/src/style.css`
- Modify: `examples/tauri-app/README.md`

- [ ] **Step 1: Add toolbar state and controls**

Extend example state with the last `AnnotationState`. Add controls shown only during an active macOS annotation-capable session:

```html
<div class="annotation-toolbar" data-annotation-toolbar hidden>
  <button type="button" data-annotation-mode>标注</button>
  <button type="button" data-annotation-tool="pen">画笔</button>
  <button type="button" data-annotation-tool="eraser">橡皮擦</button>
  <input type="color" value="#ff3b30" data-annotation-color aria-label="标注颜色" />
  <input type="range" min="1" max="64" value="4" data-annotation-width aria-label="标注粗细" />
  <button type="button" data-annotation-undo>撤销</button>
  <button type="button" data-annotation-clear>清空</button>
</div>
```

- [ ] **Step 2: Wire commands and state events**

Mode toggles `setAnnotationInteraction`. Tool/color/width changes call `setAnnotationTool`. Undo/clear call their commands. `onAnnotationStateChanged` is authoritative for button state and `canUndo`. On stop/session-ended, reset toolbar state without sending commands to the removed session.

- [ ] **Step 3: Style for clear interaction state**

Use existing toolbar tokens. Active mode and active tool need distinct selected states; disabled controls must use native `disabled` semantics. Keep the toolbar usable at 1280px without covering video controls.

- [ ] **Step 4: Build and manually verify**

```bash
pnpm --dir examples/tauri-app build
pnpm build
```

Manual acceptance: start display share, enable annotation, draw, switch eraser, undo, clear, disable mode and confirm the underlying desktop receives clicks again; repeat for window sharing.

- [ ] **Step 5: Commit**

```bash
git add examples/tauri-app
git commit -m "feat: 添加示例应用标注工具栏"
```

## Task 12: Add end-to-end correctness, recovery and performance gates

**Files:**

- Create: `tests/macos_annotation_end_to_end.rs`
- Create: `tests/macos_annotation_performance.rs`
- Modify: `tests/macos_overlay_focus_probe.swift`
- Modify: `README.md`
- Create: `docs/macos-annotation-acceptance.md`

- [ ] **Step 1: Add an ignored end-to-end decode test**

The test creates a deterministic native test window, starts window capture at 1280×720/30, injects a committed scene operation through the internal test adapter, receives H.264 from the WebRTC loopback, and decodes it with a test-only `VTDecompressionSession` wrapper in the same test file. Assert the expected red pixels, then repeat after eraser, undo and clear. Assert `framesCpuReadback == 0` and that the green border pixels are absent.

- [ ] **Step 2: Add static-scene and recovery tests**

Hold the source window static, mutate annotation revision and assert a newly encoded frame arrives within 50 ms. Inject a stale generation, pool exhaustion and one recoverable compositor failure. Verify one rebuild, forced IDR, no unannotated output, and fatal session-ended after the second consecutive failure.

- [ ] **Step 3: Add the performance harness**

Use a deterministic Metal/CoreAnimation animation window and collect 60-second release metrics for 1080p60, 1440p60 and 4K30. Fail on the approved thresholds:

```text
1080p60 capture/publish >= 55 FPS
1440p60 capture/publish >= 55 FPS on supported hardware
4K30 capture/publish >= 28 FPS
1080p compose P95 <= 4 ms
4K compose P95 <= 8 ms
capture->encoded P95 <= 25 ms; P99 <= 40 ms
annotation input->encoded P95 <= 50 ms
latest queue age P95 <= 33 ms
abnormal drops < 1%
CPU readback = 0
```

- [ ] **Step 4: Document real-machine and soak commands**

`docs/macos-annotation-acceptance.md` must contain exact commands for display/window, Retina/multi-monitor, move/resize, sleep/wake, permission revocation, 8-hour smoke and 24-hour release-candidate soak. Every result records machine model, macOS version, Metal device, VideoToolbox encoder ID and whether hardware encoding is active.

- [ ] **Step 5: Run the full verification matrix**

```bash
cargo fmt --check
cargo test
cargo test --features macos-screencapturekit
cargo clippy --features macos-screencapturekit --all-targets -- -D warnings
pnpm build
cargo test --release --features macos-screencapturekit --test macos_annotation_end_to_end -- --ignored --nocapture
cargo test --release --features macos-screencapturekit --test macos_annotation_performance -- --ignored --nocapture
```

Expected: all automated checks pass; native reports meet the approved thresholds. Do not mark the feature complete if a required real-machine or soak result is missing.

- [ ] **Step 6: Update README and commit**

Document capabilities, commands, tool behavior, macOS requirements, click interception, performance backend and unsupported first-version features.

```bash
git add README.md docs/macos-annotation-acceptance.md tests/macos_annotation_end_to_end.rs tests/macos_annotation_performance.rs tests/macos_overlay_focus_probe.swift
git commit -m "test: 完善 macOS 标注端到端验收"
```

## Final integration gate

- [ ] Re-run `git status --short` and confirm no generated or test artifact is untracked.
- [ ] Confirm every task commit is present and uses a Chinese Conventional Commit subject.
- [ ] Confirm `framesCpuReadback` remains zero in the macOS native media path.
- [ ] Confirm no `VTCompressionSessionCompleteFrames` call remains in per-frame submission.
- [ ] Confirm ScreenCaptureKit display and window filters still exclude the unified Overlay panel.
- [ ] Confirm encoded H.264 samples use ordered handoff.
- [ ] Confirm the implementation matches every requirement and non-goal in the approved spec.
- [ ] Use `requesting-code-review` before merge, then use `finishing-a-development-branch` to choose PR/merge/cleanup.
