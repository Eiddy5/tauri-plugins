# macOS 同应用多窗口浮层遮挡 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 ToDesk 等同进程多窗口目标在保持真实遮挡语义的前提下稳定显示共享浮层，并消除拖动后的闪烁和自动隐藏。

**Architecture:** 在纯模型层为窗口栈补充 owner PID 与边界，按目标同 owner、同 layer sibling 计算四角可见掩码，再逐 panel 验证排序。运行时只显示安全角；拖动期间全隐藏，松手后基于稳定窗口栈一次性重算。

**Tech Stack:** Rust 2021、Tauri 2、objc2 AppKit/Foundation、CoreGraphics Window List、Swift AppKit 真机探针。

---

## 文件结构

- 修改 `src/overlay/macos/model.rs`：保存窗口 owner/边界，提供四角可见掩码和逐 panel 层级验证。
- 修改 `src/overlay/macos/window_info.rs`：提取 owner PID 与 AppKit 坐标边界。
- 修改 `src/overlay/macos/host.rs`：维护四角可见状态，只排序安全 panel。
- 修改 `src/overlay/macos/mod.rs`：导出测试所需纯函数。
- 修改 `tests/macos_overlay_policy.rs`：覆盖 ToDesk sibling 与遮挡边界。
- 修改 `scripts/macos-overlay-order-probe.swift`：支持指定真实目标窗口。

### Task 1：建立 sibling 遮挡纯逻辑模型

**Files:**
- Modify: `tests/macos_overlay_policy.rs`
- Modify: `src/overlay/macos/model.rs`
- Modify: `src/overlay/macos/mod.rs`

- [ ] **Step 1：先写 ToDesk 模式的失败测试**

新增 `non_overlapping_same_owner_siblings_do_not_invalidate_panels`：窗口栈从上到下包含四个 panel、ToDesk `16818`、ToDesk `18571`、目标 `16303`。目标为 2560×1440，sibling 留出四周边距，断言四角全部可见且逐 panel 校验通过。

```rust
let corners = corner_panel_frames(rect(0.0, 0.0, 2560.0, 1440.0), 32.0);
let windows = vec![
    detailed_panel(11, corners[0]),
    detailed_panel(12, corners[1]),
    detailed_panel(13, corners[2]),
    detailed_panel(14, corners[3]),
    detailed_other(16818, 0, 36957, rect(48.0, 52.0, 2464.0, 1386.0)),
    detailed_other(18571, 0, 36957, rect(49.0, 918.0, 11.0, 64.0)),
    detailed_target(16303, 0, 36957, rect(0.0, 0.0, 2560.0, 1440.0)),
];
assert_eq!(visible_corner_panels(&windows, 16303, &corners), [true; 4]);
assert!(verify_panel_placements(&windows, 16303, &panel_pairs));
```

再增加三个用例：同 owner sibling 覆盖左上角时只隐藏该角；不同 owner 窗口不参与 sibling 掩码；矩形只接触边界时不算遮挡。

- [ ] **Step 2：运行聚焦测试并确认 RED**

```bash
cargo test --offline --test macos_overlay_policy sibling -- --nocapture
```

Expected: 编译失败，提示 `visible_corner_panels`、`verify_panel_placements` 或详细窗口构造器不存在。

- [ ] **Step 3：实现最小窗口栈模型**

给 `OrderedWindow` 增加 `owner_pid: Option<u32>`、`frame: Option<MacRect>` 以及 `with_owner_pid`、`with_frame` 链式构造器。实现严格正面积矩形相交，并增加：

```rust
pub fn visible_corner_panels(
    windows: &[OrderedWindow],
    target_id: u32,
    panel_frames: &[MacRect; 4],
) -> [bool; 4];

pub fn verify_panel_placements(
    windows: &[OrderedWindow],
    target_id: u32,
    panels: &[(u32, MacRect)],
) -> bool;
```

可见性只考虑目标上方、同 owner PID、同 layer、类型为 `Other` 的 sibling。逐 panel 校验允许不与该角相交的 sibling 位于 panel 与 target 之间；跨 owner 窗口、不同 layer、重叠 sibling 或缺失 panel 均返回 `false`。

- [ ] **Step 4：导出模型 API 并确认 GREEN**

从 `src/overlay/macos/mod.rs` 导出两个函数，运行：

```bash
cargo test --offline --test macos_overlay_policy -- --nocapture
```

Expected: 新增用例和原有严格层级测试全部通过。

- [ ] **Step 5：提交模型循环**

```bash
git add src/overlay/macos/model.rs src/overlay/macos/mod.rs tests/macos_overlay_policy.rs
git commit -m "test: 覆盖 macOS 同应用窗口遮挡"
```

### Task 2：接入真实窗口元数据与运行时

**Files:**
- Modify: `src/overlay/macos/window_info.rs`
- Modify: `src/overlay/macos/host.rs`

- [ ] **Step 1：增加窗口元数据的失败测试**

在 `window_info.rs` 测试模块为行转换辅助函数输入 `(id, layer, owner_pid, frame)`，断言生成的 `OrderedWindow` 保留 owner 与 AppKit 坐标边界。

- [ ] **Step 2：运行测试并确认 RED**

```bash
cargo test --offline --features macos-screencapturekit window_info -- --nocapture
```

Expected: 断言失败，因为当前 `ordered_windows` 只保存 ID、layer 和 kind。

- [ ] **Step 3：填充 CoreGraphics 元数据**

在 `ordered_windows` 中读取 `kCGWindowOwnerPID` 与 `kCGWindowBounds`，使用现有 `appkit_rect` 转换坐标，为 target、panel、other 调用 `with_owner_pid` 与 `with_frame`。缺少 owner 或有效边界时保留 `None`，模型按 fail-closed 处理。

- [ ] **Step 4：让 OverlayHost 维护每角可见掩码**

给 `OverlayHost` 增加 `panel_visibility: [bool; 4]`。`refresh_window` 先计算 `panel_frames` 和 `visible_corner_panels`；把掩码变化纳入 native update 条件；只对可见 panel 更新 frame、level 并 `order_above`，其余调用 `hide()`；保存掩码并调度延迟校验。

延迟校验从 `panel_visibility` 和 `last_frame` 生成 `(panel_id, frame)` 列表并调用 `verify_panel_placements`。失败时 fail-closed 隐藏全部 panel，但不影响采集会话。

- [ ] **Step 5：运行策略与运行时测试并确认 GREEN**

```bash
cargo test --offline --test macos_overlay_policy -- --nocapture
cargo test --offline --features macos-screencapturekit overlay::macos -- --nocapture
```

Expected: ToDesk sibling、四角几何、拖动和校验代次测试全部通过。

- [ ] **Step 6：提交运行时修复**

```bash
git add src/overlay/macos/window_info.rs src/overlay/macos/host.rs
git commit -m "fix: 适配 macOS 同应用多窗口浮层"
```

### Task 3：修正真机探针并完整验证

**Files:**
- Modify: `scripts/macos-overlay-order-probe.swift`

- [ ] **Step 1：让探针选择真实用户窗口**

增加 `TARGET_WINDOW_ID` 环境变量。未指定时排除 owner 为 `Window Server`、无有效边界、非 layer-0 和当前进程窗口。输出 target owner、同 owner sibling、panel 顺序及各角遮挡结果，不再假定四个 panel 必须紧邻 target。

- [ ] **Step 2：运行普通窗口和 ToDesk 反馈环**

```bash
xcrun swift scripts/macos-overlay-order-probe.swift
TARGET_WINDOW_ID=16303 xcrun swift scripts/macos-overlay-order-probe.swift
TARGET_WINDOW_ID=16818 xcrun swift scripts/macos-overlay-order-probe.swift
```

Expected: 普通窗口完整四角有效；ToDesk 报告应用分组存在，但未遮挡角有效、遮挡角隐藏；退出码均为 0。

- [ ] **Step 3：运行完整验证**

```bash
cargo fmt --all -- --check
cargo test --offline --all-targets --features macos-screencapturekit
cargo clippy --offline --all-targets --features macos-screencapturekit -- -D warnings -A dead-code -A clippy::duplicated-attributes -A clippy::items-after-test-module
cargo build --offline --manifest-path examples/tauri-app/src-tauri/Cargo.toml
git diff --check
```

Expected: 全部命令退出码为 0，仅保留项目已知的 `is_h264_idr` dead-code 构建警告。

- [ ] **Step 4：提交探针并确认工作树**

```bash
git add scripts/macos-overlay-order-probe.swift
git commit -m "test: 增强 macOS 多窗口浮层探针"
git status --short
```

Expected: 提交成功且工作树为空。
