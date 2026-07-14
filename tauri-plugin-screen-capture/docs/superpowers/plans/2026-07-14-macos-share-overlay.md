# macOS 屏幕共享浮层 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 macOS 的“显示器 + 单窗口”共享实现低开销四角绿色浮层，并保证单窗口浮层与目标窗口同层、紧邻目标窗口之上且会被原本位于目标之上的窗口自然遮挡。

**Architecture:** `ShareOverlay` 只保存可跨线程的会话 ID 与 Tauri 主线程调度器，所有 `NSPanel`、`NSView`、`CALayer` 和 AppKit 监听器都保存在主线程 registry。显示器目标使用四个无边框 `NSPanel` 描边；窗口目标额外读取 CoreGraphics 窗口列表，采用 `orderWindow_relativeTo(.above, targetWindowID)` 建立相对层级并在事件后校验，失败时隐藏而不是降级到置顶。

**Tech Stack:** Rust 2021、Tauri 2、objc2 0.6、objc2-app-kit 0.3、objc2-foundation 0.3、objc2-core-graphics 0.3、objc2-quartz-core 0.3、ScreenCaptureKit 3.0、tokio、Core Animation。

---

## 文件结构

- `src/overlay/macos/mod.rs`：macOS 工厂、`ShareOverlay` 生命周期与模块出口。
- `src/overlay/macos/dispatcher.rs`：对象安全的 AppKit 主线程调度接口及 Tauri 实现。
- `src/overlay/macos/model.rs`：窗口快照、可见性与层级决策的纯 Rust 模型。
- `src/overlay/macos/window_info.rs`：读取 `CGWindowListCopyWindowInfo`、显示器边界与坐标转换。
- `src/overlay/macos/panel.rs`：创建四个 `NSPanel`，用 `NSView + CALayer` 绘制四角。
- `src/overlay/macos/host.rs`：主线程 registry、会话创建/刷新/显示/隐藏/销毁。
- `src/overlay/macos/events.rs`：鼠标拖动、工作区、屏幕与睡眠事件监听，事件触发刷新。
- `src/overlay/mod.rs`：按平台选择默认工厂，并导出 macOS 窗口标题前缀。
- `src/state.rs`、`src/lib.rs`：在插件 setup 时把 `AppHandle` 驱动的 macOS 工厂注入状态。
- `src/platform/macos/screencapturekit.rs`：显示器捕获排除本进程浮层，保留本进程普通窗口。
- `tests/macos_overlay_policy.rs`：纯逻辑层级、遮挡、隐藏和四角布局回归测试。
- `docs/macos-overlay-acceptance.md`：真机层级、空间切换、Stage Manager、捕获排除和性能验收步骤。

### Task 1: 建立可测试的窗口层级决策模型

**Files:**
- Create: `src/overlay/macos/model.rs`
- Create: `tests/macos_overlay_policy.rs`
- Modify: `src/overlay/mod.rs`

- [ ] **Step 1: 写失败测试**

```rust
#![cfg(target_os = "macos")]

use tauri_plugin_screen_capture::overlay::macos::{
    decide_window_overlay, OverlayDecision, WindowSnapshot,
};

fn window(id: u32, layer: i32, order: usize, on_screen: bool) -> WindowSnapshot {
    WindowSnapshot { id, layer, order, on_screen, minimized: false }
}

#[test]
fn overlay_uses_target_layer_and_immediately_above_order() {
    let decision = decide_window_overlay(&window(42, 0, 7, true), true);
    assert_eq!(decision, OverlayDecision::Show { target_id: 42, level: 0, target_order: 7 });
}

#[test]
fn overlay_hides_when_target_cannot_be_safely_ordered() {
    assert_eq!(decide_window_overlay(&window(42, 0, 7, false), true), OverlayDecision::Hide);
    assert_eq!(decide_window_overlay(&window(42, 0, 7, true), false), OverlayDecision::Hide);
}

#[test]
fn overlay_hides_for_minimized_target() {
    let mut target = window(42, 0, 7, true);
    target.minimized = true;
    assert_eq!(decide_window_overlay(&target, true), OverlayDecision::Hide);
}
```

- [ ] **Step 2: 运行测试并确认 RED**

Run: `cargo test --test macos_overlay_policy`

Expected: FAIL，提示 `overlay::macos` 或 `decide_window_overlay` 尚不存在。

- [ ] **Step 3: 实现最小模型**

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowSnapshot {
    pub id: u32,
    pub layer: i32,
    pub order: usize,
    pub on_screen: bool,
    pub minimized: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayDecision {
    Show { target_id: u32, level: i32, target_order: usize },
    Hide,
}

pub fn decide_window_overlay(target: &WindowSnapshot, relative_order_supported: bool) -> OverlayDecision {
    if !target.on_screen || target.minimized || !relative_order_supported {
        return OverlayDecision::Hide;
    }
    OverlayDecision::Show {
        target_id: target.id,
        level: target.layer,
        target_order: target.order,
    }
}
```

并在 `src/overlay/mod.rs` 中加入：

```rust
#[cfg(target_os = "macos")]
pub mod macos;
```

- [ ] **Step 4: 运行测试并确认 GREEN**

Run: `cargo test --test macos_overlay_policy`

Expected: 3 passed。

- [ ] **Step 5: 提交**

```bash
git add src/overlay/mod.rs src/overlay/macos/model.rs tests/macos_overlay_policy.rs
git commit -m "feat: 添加 macOS 浮层层级决策模型"
```

### Task 2: 接入 Tauri AppKit 主线程调度器

**Files:**
- Create: `src/overlay/macos/dispatcher.rs`
- Modify: `src/overlay/macos/mod.rs`
- Modify: `src/state.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: 写失败测试**

在 `dispatcher.rs` 内加入测试，约束请求必须等待主线程闭包的结果：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct InlineDispatcher;
    impl MainThreadDispatcher for InlineDispatcher {
        fn dispatch(&self, task: MainThreadTask) -> crate::Result<()> {
            task();
            Ok(())
        }
    }

    #[tokio::test]
    async fn request_returns_main_thread_result() {
        let value = request(&InlineDispatcher, || Ok::<_, crate::Error>(41 + 1)).await.unwrap();
        assert_eq!(value, 42);
    }
}
```

- [ ] **Step 2: 运行测试并确认 RED**

Run: `cargo test dispatcher::tests::request_returns_main_thread_result`

Expected: FAIL，提示 `MainThreadDispatcher` 与 `request` 未定义。

- [ ] **Step 3: 实现调度器并注入插件状态**

```rust
pub type MainThreadTask = Box<dyn FnOnce() + Send + 'static>;

pub trait MainThreadDispatcher: Send + Sync {
    fn dispatch(&self, task: MainThreadTask) -> crate::Result<()>;
}

pub struct TauriMainThreadDispatcher<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> TauriMainThreadDispatcher<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self { Self { app } }
}

impl<R: tauri::Runtime> MainThreadDispatcher for TauriMainThreadDispatcher<R> {
    fn dispatch(&self, task: MainThreadTask) -> crate::Result<()> {
        self.app.run_on_main_thread(task).map_err(|error| {
            crate::Error::new(
                crate::models::CaptureErrorCode::Internal,
                format!("无法调度 macOS 浮层主线程任务: {error}"),
                true,
            )
        })
    }
}

pub async fn request<T: Send + 'static>(
    dispatcher: &dyn MainThreadDispatcher,
    operation: impl FnOnce() -> crate::Result<T> + Send + 'static,
) -> crate::Result<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    dispatcher.dispatch(Box::new(move || { let _ = tx.send(operation()); }))?;
    rx.await.map_err(|_| crate::Error::new(
        crate::models::CaptureErrorCode::Internal,
        "macOS 浮层主线程任务未返回结果",
        true,
    ))?
}
```

`ScreenCaptureState` 新增 crate 内构造器：

```rust
pub(crate) fn with_overlay_factory_and_publisher_factory(
    overlay_factory: Arc<dyn ShareOverlayFactory>,
    publisher_factory: Option<Arc<dyn CapturePublisherFactory>>,
) -> Self {
    Self::with_backend_overlay_and_publisher_factory(
        default_backend(),
        overlay_factory,
        publisher_factory.unwrap_or_else(|| Arc::new(DefaultPublisherFactory)),
    )
}
```

插件 setup 在 macOS 构造 `MacOsShareOverlayFactory::new(app.clone())`，其他平台继续使用 `DefaultShareOverlayFactory`。

- [ ] **Step 4: 运行格式、单测与类型检查**

Run: `cargo fmt --check && cargo test dispatcher::tests::request_returns_main_thread_result && cargo check --features macos-screencapturekit`

Expected: 全部成功；只允许现有 `is_h264_idr` dead-code 警告。

- [ ] **Step 5: 提交**

```bash
git add Cargo.toml src/overlay/macos/dispatcher.rs src/overlay/macos/mod.rs src/state.rs src/lib.rs
git commit -m "feat: 接入 macOS 浮层主线程调度"
```

### Task 3: 创建四个低开销 NSPanel 与 CALayer 四角

**Files:**
- Create: `src/overlay/macos/panel.rs`
- Modify: `src/overlay/macos/mod.rs`
- Modify: `Cargo.toml`
- Modify: `tests/macos_overlay_policy.rs`

- [ ] **Step 1: 写失败测试**

```rust
use tauri_plugin_screen_capture::overlay::macos::{corner_panel_frames, MacRect};

#[test]
fn four_corner_panels_only_allocate_corner_squares() {
    let frames = corner_panel_frames(MacRect { x: 10.0, y: 20.0, width: 800.0, height: 600.0 }, 32.0);
    assert_eq!(frames.len(), 4);
    assert_eq!(frames[0], MacRect { x: 10.0, y: 588.0, width: 32.0, height: 32.0 });
    assert_eq!(frames[3], MacRect { x: 778.0, y: 20.0, width: 32.0, height: 32.0 });
}
```

- [ ] **Step 2: 运行测试并确认 RED**

Run: `cargo test --test macos_overlay_policy four_corner_panels_only_allocate_corner_squares`

Expected: FAIL，提示 `corner_panel_frames` 未定义。

- [ ] **Step 3: 实现几何和原生面板**

`corner_panel_frames` 必须只返回四个 `32×32` 方块；`CornerPanel::new` 使用以下固定属性：

```rust
panel.setOpaque(false);
panel.setHasShadow(false);
panel.setIgnoresMouseEvents(true);
panel.setBackgroundColor(Some(&NSColor::clearColor()));
panel.setCollectionBehavior(
    NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::Stationary,
);
```

根视图启用 layer，两个子 `CALayer` 分别绘制水平和垂直绿色条；更新 frame 时关闭隐式动画：

```rust
CATransaction::begin();
CATransaction::setDisableActions(true);
horizontal.setFrame(horizontal_frame);
vertical.setFrame(vertical_frame);
CATransaction::commit();
```

四角面板标题使用 `TAURI_SCREEN_CAPTURE_OVERLAY:{session_id}:{corner}`，颜色为 `0x22C55E`，粗细 4pt，面板不创建定时绘制循环。

- [ ] **Step 4: 运行测试和 macOS 编译**

Run: `cargo test --test macos_overlay_policy && cargo check --features macos-screencapturekit`

Expected: 测试通过，objc2 AppKit/Core Animation API 编译成功。

- [ ] **Step 5: 提交**

```bash
git add Cargo.toml src/overlay/macos/mod.rs src/overlay/macos/panel.rs tests/macos_overlay_policy.rs
git commit -m "feat: 添加 macOS 四角原生浮层面板"
```

### Task 4: 读取目标边界并实现同层相对排序

**Files:**
- Create: `src/overlay/macos/window_info.rs`
- Create: `src/overlay/macos/host.rs`
- Modify: `src/overlay/macos/mod.rs`
- Modify: `tests/macos_overlay_policy.rs`

- [ ] **Step 1: 写失败测试**

```rust
use tauri_plugin_screen_capture::overlay::macos::{verify_relative_order, OrderedWindow};

#[test]
fn verification_accepts_four_panels_immediately_above_target() {
    let windows = vec![
        OrderedWindow::other(90),
        OrderedWindow::panel(14), OrderedWindow::panel(13),
        OrderedWindow::panel(12), OrderedWindow::panel(11),
        OrderedWindow::target(42),
        OrderedWindow::other(7),
    ];
    assert!(verify_relative_order(&windows, 42, &[11, 12, 13, 14]));
}

#[test]
fn verification_rejects_a_panel_promoted_above_an_existing_cover_window() {
    let windows = vec![
        OrderedWindow::panel(11), OrderedWindow::other(90),
        OrderedWindow::panel(14), OrderedWindow::panel(13),
        OrderedWindow::panel(12), OrderedWindow::target(42),
    ];
    assert!(!verify_relative_order(&windows, 42, &[11, 12, 13, 14]));
}
```

- [ ] **Step 2: 运行测试并确认 RED**

Run: `cargo test --test macos_overlay_policy verification_`

Expected: FAIL，提示验证函数与有序窗口类型未定义。

- [ ] **Step 3: 实现 CoreGraphics 查询与 fail-closed 排序**

`window_info.rs` 使用 `CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID)`，按返回数组顺序保存前后关系，并读取 `kCGWindowNumber`、`kCGWindowLayer`、`kCGWindowBounds`、`kCGWindowOwnerPID`、`kCGWindowIsOnscreen`。

窗口目标刷新顺序固定为：

```rust
let target = window_snapshot(target_id)?;
let OverlayDecision::Show { level, .. } = decide_window_overlay(&target, true) else {
    return self.hide_panels();
};
for panel in &self.panels {
    panel.set_level(level);
    panel.order_above(target_id);
}
let ordered = ordered_windows()?;
if !verify_relative_order(&ordered, target_id, &self.panel_window_ids()) {
    self.hide_panels();
    return Err(overlay_error("无法维持目标窗口的相对层级，已隐藏浮层"));
}
```

显示器目标使用 `CGDisplayBounds` 和 AppKit 坐标转换；窗口目标使用 CGWindow 边界。`show` 只恢复期望可见状态后刷新，`hide` 执行 `orderOut`，`stop` 从 registry 移除并释放全部监听器和面板。

- [ ] **Step 4: 运行策略测试和 macOS 编译**

Run: `cargo test --test macos_overlay_policy && cargo check --features macos-screencapturekit`

Expected: 全部成功。

- [ ] **Step 5: 提交**

```bash
git add src/overlay/macos/mod.rs src/overlay/macos/window_info.rs src/overlay/macos/host.rs tests/macos_overlay_policy.rs
git commit -m "feat: 实现 macOS 浮层边界与相对层级"
```

### Task 5: 用事件驱动保持拖动、焦点与空间状态同步

**Files:**
- Create: `src/overlay/macos/events.rs`
- Modify: `src/overlay/macos/host.rs`
- Modify: `src/overlay/macos/mod.rs`
- Modify: `tests/macos_overlay_policy.rs`

- [ ] **Step 1: 写失败测试**

```rust
use tauri_plugin_screen_capture::overlay::macos::{event_action, OverlayEvent, RefreshAction};

#[test]
fn drag_hides_immediately_and_mouse_up_refreshes_once() {
    assert_eq!(event_action(OverlayEvent::LeftMouseDragged), RefreshAction::Hide);
    assert_eq!(event_action(OverlayEvent::LeftMouseUp), RefreshAction::Refresh);
}

#[test]
fn workspace_and_display_changes_trigger_refresh() {
    for event in [
        OverlayEvent::ApplicationActivated,
        OverlayEvent::ApplicationHidden,
        OverlayEvent::ApplicationTerminated,
        OverlayEvent::ActiveSpaceChanged,
        OverlayEvent::ScreenParametersChanged,
        OverlayEvent::SystemWoke,
    ] {
        assert_eq!(event_action(event), RefreshAction::Refresh);
    }
}
```

- [ ] **Step 2: 运行测试并确认 RED**

Run: `cargo test --test macos_overlay_policy event_`

Expected: FAIL，提示事件类型未定义。

- [ ] **Step 3: 实现事件监听**

全局与本地 `NSEvent` monitor 监听 `LeftMouseDragged | LeftMouseUp`；拖动先 `orderOut`，抬起统一刷新。`NSWorkspace.notificationCenter` 监听应用激活、隐藏、终止、活动 Space 变化、睡眠与唤醒；`NSNotificationCenter.defaultCenter` 监听 `NSApplicationDidChangeScreenParametersNotification`。

每个回调只调用：

```rust
match event_action(event) {
    RefreshAction::Hide => registry_hide(session_id),
    RefreshAction::Refresh => registry_refresh(session_id),
    RefreshAction::None => {}
}
```

监听 token 保存在 `EventObservers`，Drop/stop 时逐个移除。不得使用 `CADisplayLink`；仅保留一个 2 秒纠偏计时器，且 bounds 和有序窗口 ID 未变化时不执行 frame/order 更新。

- [ ] **Step 4: 运行测试和编译**

Run: `cargo test --test macos_overlay_policy && cargo check --features macos-screencapturekit`

Expected: 全部成功，无新增警告。

- [ ] **Step 5: 提交**

```bash
git add src/overlay/macos/events.rs src/overlay/macos/host.rs src/overlay/macos/mod.rs tests/macos_overlay_policy.rs
git commit -m "feat: 添加 macOS 浮层事件同步"
```

### Task 6: 从显示器捕获中排除浮层

**Files:**
- Modify: `src/platform/macos/screencapturekit.rs`
- Modify: `src/overlay/macos/mod.rs`

- [ ] **Step 1: 写失败单元测试**

在 `screencapturekit.rs` 测试模块中用纯值列表验证排除策略：

```rust
#[test]
fn keeps_current_process_non_overlay_windows_as_exceptions() {
    let windows = vec![
        FilterWindow { id: 1, pid: 123, title: "TAURI_SCREEN_CAPTURE_OVERLAY:7:tl".into() },
        FilterWindow { id: 2, pid: 123, title: "设置".into() },
        FilterWindow { id: 3, pid: 456, title: "文档".into() },
    ];
    assert_eq!(current_app_window_exceptions(123, &windows), vec![2]);
}
```

- [ ] **Step 2: 运行测试并确认 RED**

Run: `cargo test --features macos-screencapturekit keeps_current_process_non_overlay_windows_as_exceptions`

Expected: FAIL，提示策略函数未定义。

- [ ] **Step 3: 实现 ScreenCaptureKit 过滤器**

显示器过滤器找到 `process_id() == std::process::id()` 的 `SCRunningApplication`，并把同 PID、标题不以 `TAURI_SCREEN_CAPTURE_OVERLAY:` 开头的 `SCWindow` 作为 excepting windows：

```rust
SCContentFilter::create()
    .with_display(&display)
    .with_excluding_applications(&[&current_app], &exception_refs)
    .build()
```

找不到当前应用时降级为 `with_excluding_windows(&[])`；单窗口捕获继续使用 `with_window(&window)`。实际捕获和缩略图共用同一个 helper，避免路径行为不一致。

- [ ] **Step 4: 运行 feature 测试和完整测试**

Run: `cargo test --features macos-screencapturekit keeps_current_process_non_overlay_windows_as_exceptions && cargo test --all-targets`

Expected: 全部成功。

- [ ] **Step 5: 提交**

```bash
git add src/platform/macos/screencapturekit.rs src/overlay/macos/mod.rs
git commit -m "fix: 排除 macOS 屏幕共享浮层画面"
```

### Task 7: 补齐真机层级与性能验收文档

**Files:**
- Create: `docs/macos-overlay-acceptance.md`
- Modify: `docs/superpowers/specs/2026-07-14-macos-share-overlay-design.md`

- [ ] **Step 1: 写中文验收矩阵**

文档必须列出以下可复现用例、预期结果和采样命令：

```markdown
| 场景 | 操作 | 预期 |
|---|---|---|
| 同层窗口遮挡 | 共享窗口 A，再把普通窗口 B 放到 A 上方 | B 同时遮住 A 和绿色浮层 |
| 目标重获焦点 | 点击 A | A 与浮层一起回到 B 上方，浮层仍紧邻 A |
| 部分遮挡 | B 只覆盖 A 的一个角 | 对应绿色角被 B 遮挡，其余角仍可见 |
| 最小化/隐藏/关闭 | 最小化、隐藏应用、关闭 A | 浮层立即隐藏且不残留 |
| Space/全屏 | 切换 Space 或让 A 进入全屏 | 非目标 Space 不出现浮层，返回后恢复 |
| Stage Manager | 把 A 收入/移出侧边台 | 浮层可见性跟随 A |
| 显示器捕获 | 共享显示器并录制 10 秒 | 本地看见浮层，编码画面不含浮层 |
| 空闲性能 | Activity Monitor 观察 60 秒 | 浮层空闲 CPU 目标低于 0.5% |
```

层级采样命令使用仓库内的 Swift probe 或 `CGWindowListCopyWindowInfo` 调试日志，确认四个 panel 在目标前连续出现、原本的遮挡窗口仍排在四个 panel 前。

- [ ] **Step 2: 核对设计文档链接**

在设计文档“测试与验收”末尾加入：

```markdown
具体真机步骤见 [macOS 浮层验收清单](../../macos-overlay-acceptance.md)。
```

- [ ] **Step 3: 提交**

```bash
git add docs/macos-overlay-acceptance.md docs/superpowers/specs/2026-07-14-macos-share-overlay-design.md
git commit -m "docs: 添加 macOS 浮层真机验收清单"
```

### Task 8: 最终验证与质量门禁

**Files:**
- Modify only if verification exposes defects.

- [ ] **Step 1: 格式与静态检查**

Run: `cargo fmt --check && cargo clippy --all-targets --features macos-screencapturekit -- -D warnings`

Expected: 成功；若现有 `is_h264_idr` 警告阻断 clippy，则先确认它是基线问题，再用不扩大本功能范围的局部 `#[allow(dead_code)]` 修正并单独提交 `chore: 清理现有静态检查警告`。

- [ ] **Step 2: 全量自动化测试**

Run: `cargo test --all-targets --features macos-screencapturekit`

Expected: 现有测试和新增 macOS 测试全部通过，0 failed。

- [ ] **Step 3: 示例应用构建**

Run: `cargo check --manifest-path examples/tauri-app/src-tauri/Cargo.toml`

Expected: 示例 Tauri 应用成功链接插件与 macOS 原生依赖。

- [ ] **Step 4: 执行真机关键路径**

按照 `docs/macos-overlay-acceptance.md` 至少执行：单窗口遮挡/重获焦点/最小化、显示器捕获排除、60 秒空闲 CPU。记录 macOS 版本、芯片、通过项和未通过项；任一层级校验失败必须保持 fail-closed，不得改成浮层置顶兜底。

- [ ] **Step 5: 最终提交（仅有验证修复时）**

```bash
git add <验证中实际修复的文件>
git commit -m "fix: 修正 macOS 浮层验收问题"
```

## 自检结论

- 设计覆盖：显示器与单窗口、四 `NSPanel`、CALayer 无持续渲染、主线程隔离、拖动隐藏、工作区/Space/睡眠事件、同层相对排序、自然遮挡、最小化/关闭隐藏、ScreenCaptureKit 排除、多会话 registry、性能目标和真机验收均有对应任务。
- 占位符检查：计划不含 TBD/TODO/“稍后实现”等未定义工作；真机步骤明确列入 Task 7/8。
- 类型一致性：统一使用 `WindowSnapshot`、`OverlayDecision`、`MacRect`、`OrderedWindow`、`MainThreadDispatcher`、`MacOsShareOverlayFactory` 和标题前缀 `TAURI_SCREEN_CAPTURE_OVERLAY:`。
