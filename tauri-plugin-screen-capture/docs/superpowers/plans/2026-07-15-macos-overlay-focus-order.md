# macOS Overlay Focus Order Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Detect a shared macOS window being reordered above its four corner panels and restore the overlay within the existing 100 ms tracking interval.

**Architecture:** Add a pure order validator in `model.rs`, expose a lightweight CoreGraphics Window ID list in `window_info.rs`, then connect both to the `WindowPositionTimer` fast path in `host.rs`. Full window descriptions and native panel reordering remain in the existing correction path and run only when geometry or order is invalid.

**Post-review correction:** The original contiguous-panel contract below was superseded during implementation. A fully validated order may legally contain same-owner sibling windows between panels and the target. The final implementation caches the exact validated `panel/sibling/target` span, compares that span in the 100 ms path, and rebuilds it after a full validation. This also provides a sibling anchor when all panels are hidden. The final regression tests and implementation, rather than the earlier contiguous-only snippets, are authoritative.

**Tech Stack:** Rust, objc2 0.6, objc2-core-foundation 0.3, objc2-core-graphics 0.3, AppKit `NSPanel`, CoreGraphics Window Services, Cargo tests.

Run every command below from `/Users/mac18/workspace/base_rust/tauri/tauri-plugins/tauri-plugin-screen-capture`.

---

## File structure

- Modify `src/overlay/macos/model.rs`: pure lightweight relative-order policy and `WindowFrameAction` escalation.
- Modify `src/overlay/macos/mod.rs`: export the pure policy for integration tests.
- Modify `src/overlay/macos/window_info.rs`: read pointer-encoded `CGWindowID` values from `CGWindowListCreate` without materializing description dictionaries.
- Modify `src/overlay/macos/host.rs`: check lightweight order only on stable visible frames, then reuse `refresh_window_geometry` on failure.
- Modify `tests/macos_overlay_policy.rs`: cross-platform policy regression tests.

### Task 1: Define the lightweight order contract

**Files:**
- Modify: `tests/macos_overlay_policy.rs`
- Modify: `src/overlay/macos/model.rs`
- Modify: `src/overlay/macos/mod.rs`

- [ ] **Step 1: Write failing policy tests**

Add `verify_lightweight_order` to the imports in `tests/macos_overlay_policy.rs`, then add:

```rust
#[test]
fn lightweight_order_accepts_a_contiguous_panel_group_immediately_above_target() {
    assert!(verify_lightweight_order(
        &[90, 13, 12, 11, 42, 7],
        42,
        &[11, 12, 13]
    ));
}

#[test]
fn lightweight_order_rejects_target_reordered_above_panels() {
    assert!(!verify_lightweight_order(
        &[90, 42, 13, 12, 11, 7],
        42,
        &[11, 12, 13]
    ));
}

#[test]
fn lightweight_order_rejects_an_intervening_window_or_missing_visible_panel() {
    assert!(!verify_lightweight_order(
        &[90, 13, 77, 12, 11, 42, 7],
        42,
        &[11, 12, 13]
    ));
    assert!(!verify_lightweight_order(
        &[90, 13, 11, 42, 7],
        42,
        &[11, 12, 13]
    ));
}

#[test]
fn lightweight_order_allows_panels_hidden_by_visibility_policy() {
    assert!(verify_lightweight_order(&[90, 42, 7], 42, &[]));
    assert!(verify_lightweight_order(&[90, 13, 42, 7], 42, &[13]));
}
```

- [ ] **Step 2: Run the focused tests and confirm RED**

Run:

```bash
cargo test --test macos_overlay_policy lightweight_order --features macos-screencapturekit
```

Expected: compilation fails because `verify_lightweight_order` does not exist.

- [ ] **Step 3: Implement the minimal pure validator**

Add to `src/overlay/macos/model.rs`:

```rust
pub fn verify_lightweight_order(
    window_ids: &[u32],
    target_id: u32,
    visible_panel_ids: &[u32],
) -> bool {
    if visible_panel_ids.is_empty() {
        return window_ids.contains(&target_id);
    }
    let Some(target_index) = window_ids.iter().position(|id| *id == target_id) else {
        return false;
    };
    let Some(panel_start) = target_index.checked_sub(visible_panel_ids.len()) else {
        return false;
    };
    let panel_group = &window_ids[panel_start..target_index];
    visible_panel_ids
        .iter()
        .all(|panel_id| panel_group.iter().filter(|id| *id == panel_id).count() == 1)
}
```

Export it from `src/overlay/macos/mod.rs` alongside `verify_panel_placements`.

- [ ] **Step 4: Run the focused tests and confirm GREEN**

Run the command from Step 2. Expected: four `lightweight_order_*` tests pass.

- [ ] **Step 5: Commit the policy slice**

```bash
git add src/overlay/macos/model.rs \
  src/overlay/macos/mod.rs \
  tests/macos_overlay_policy.rs
git commit -m "fix: 增加 macOS 浮层轻量层级校验"
```

### Task 2: Read the public CoreGraphics Window ID order

**Files:**
- Modify: `src/overlay/macos/window_info.rs`

- [ ] **Step 1: Write a failing real-API regression test**

Inside the existing `#[cfg(test)]` module in `src/overlay/macos/window_info.rs`, add:

```rust
#[test]
fn lightweight_window_ids_include_a_valid_on_screen_window() {
    let rows = window_rows(CGWindowListOption::OptionOnScreenOnly).unwrap();
    let window_id = rows
        .iter()
        .find_map(|row| number(row, unsafe { kCGWindowNumber })?.as_i64())
        .expect("当前 GUI 会话应至少包含一个可见窗口") as u32;

    assert!(visible_window_ids().unwrap().contains(&window_id));
}
```

- [ ] **Step 2: Run the test and confirm RED**

Run:

```bash
cargo test --lib lightweight_window_ids_include_a_valid_on_screen_window \
  --features macos-screencapturekit
```

Expected: compilation fails because `visible_window_ids` does not exist.

- [ ] **Step 3: Implement pointer-encoded ID extraction**

Add this helper near `window_row` in `src/overlay/macos/window_info.rs`:

```rust
pub(crate) fn visible_window_ids() -> Result<Vec<u32>> {
    let windows = CGWindowListCreate(
        CGWindowListOption::OptionOnScreenOnly,
        kCGNullWindowID,
    )
    .ok_or_else(|| {
        Error::new(
            CaptureErrorCode::Internal,
            "无法读取 macOS 可见窗口顺序",
            true,
        )
    })?;

    Ok((0..windows.count())
        .map(|index| {
            // SAFETY: CGWindowListCreate returns an array whose values are pointer-encoded
            // CGWindowID entries, and index is within the array's reported count.
            unsafe { windows.value_at_index(index) } as usize as u32
        })
        .collect())
}
```

Do not cast the array to `CFArray<CFNumber>` or iterate it as CoreFoundation objects; these entries are raw pointer-sized ID values.

- [ ] **Step 4: Run focused and existing targeted-query tests**

Run:

```bash
cargo test --lib window_ids --features macos-screencapturekit
cargo test --lib targeted_window_query --features macos-screencapturekit
```

Expected: both command groups pass, including valid and invalid targeted window queries.

- [ ] **Step 5: Commit the API slice**

```bash
git add src/overlay/macos/window_info.rs
git commit -m "fix: 读取 macOS 轻量窗口顺序"
```

### Task 3: Escalate stable-frame order failures into the correction path

**Files:**
- Modify: `tests/macos_overlay_policy.rs`
- Modify: `src/overlay/macos/model.rs`
- Modify: `src/overlay/macos/host.rs`

- [ ] **Step 1: Write a failing action-policy test**

Add to `tests/macos_overlay_policy.rs`:

```rust
#[test]
fn stable_frame_escalates_to_refresh_when_lightweight_order_is_invalid() {
    assert_eq!(
        WindowFrameAction::Keep.refresh_if_order_invalid(false),
        WindowFrameAction::Refresh
    );
    assert_eq!(
        WindowFrameAction::Keep.refresh_if_order_invalid(true),
        WindowFrameAction::Keep
    );
    assert_eq!(
        WindowFrameAction::Hide.refresh_if_order_invalid(false),
        WindowFrameAction::Hide
    );
}
```

- [ ] **Step 2: Run the test and confirm RED**

Run:

```bash
cargo test --test macos_overlay_policy stable_frame_escalates \
  --features macos-screencapturekit
```

Expected: compilation fails because `refresh_if_order_invalid` does not exist.

- [ ] **Step 3: Add the minimal action transition**

Add to `impl WindowFrameAction` in `src/overlay/macos/model.rs`:

```rust
impl WindowFrameAction {
    pub const fn refresh_if_order_invalid(self, relative_order_valid: bool) -> Self {
        match self {
            Self::Keep if !relative_order_valid => Self::Refresh,
            action => action,
        }
    }
}
```

- [ ] **Step 4: Wire the stable-frame fast path**

Import `verify_lightweight_order` and `visible_window_ids` in `src/overlay/macos/host.rs`. In `track_window_frame`, first compute the existing frame action. Only when it is `Keep` and panels are visible, build the visible panel ID subset and validate the lightweight list:

```rust
let mut action = self
    .window_frame_tracker
    .observe_for_visibility(geometry.frame, self.panels_visible);
if action == WindowFrameAction::Keep && self.panels_visible {
    let visible_panel_ids = self
        .panels
        .iter()
        .zip(self.panel_visibility)
        .filter_map(|(panel, visible)| visible.then_some(panel.window_id()))
        .collect::<Vec<_>>();
    let order_valid = visible_window_ids()
        .map(|window_ids| verify_lightweight_order(&window_ids, target_id, &visible_panel_ids))
        .unwrap_or(false);
    action = action.refresh_if_order_invalid(order_valid);
}

match action {
    WindowFrameAction::Keep => Ok(()),
    WindowFrameAction::Hide => {
        self.hide_panels();
        Ok(())
    }
    WindowFrameAction::Refresh => self.refresh_window_geometry(target_id, geometry),
}
```

An ID-list read error intentionally maps to `false`, so the code falls back to the existing full refresh instead of hiding immediately or stopping capture.

- [ ] **Step 5: Run policy and library tests and confirm GREEN**

```bash
cargo test --test macos_overlay_policy --features macos-screencapturekit
cargo test --lib --features macos-screencapturekit
```

Expected: all policy and library tests pass.

- [ ] **Step 6: Commit the host integration**

```bash
git add src/overlay/macos/host.rs \
  src/overlay/macos/model.rs \
  tests/macos_overlay_policy.rs
git commit -m "fix: 快速纠正 macOS 共享窗口聚焦层级"
```

### Task 4: Add a real AppKit focus-order probe

**Files:**
- Create: `tests/macos_overlay_focus_probe.swift`

- [ ] **Step 1: Write the real-window probe before host integration is considered complete**

Create `tests/macos_overlay_focus_probe.swift`:

```swift
import AppKit
import CoreGraphics
import Foundation

func visibleWindowIDs() -> [CGWindowID] {
    guard let list = CGWindowListCreate(.optionOnScreenOnly, kCGNullWindowID) else {
        return []
    }
    return (0..<CFArrayGetCount(list)).compactMap { index in
        guard let raw = CFArrayGetValueAtIndex(list, index) else {
            return nil
        }
        return CGWindowID(UInt(bitPattern: raw))
    }
}

func orderIsValid(_ ids: [CGWindowID], target: CGWindowID, panels: [CGWindowID]) -> Bool {
    guard let targetIndex = ids.firstIndex(of: target), targetIndex >= panels.count else {
        return false
    }
    let group = ids[(targetIndex - panels.count)..<targetIndex]
    return panels.allSatisfy { panel in group.filter { $0 == panel }.count == 1 }
}

func advanceWindowServer() {
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

let target = NSWindow(
    contentRect: NSRect(x: 120, y: 120, width: 480, height: 320),
    styleMask: [.titled, .closable, .resizable],
    backing: .buffered,
    defer: false
)
target.title = "macos-overlay-focus-probe-target"
target.level = .normal
target.orderFrontRegardless()

let panelFrames = [
    NSRect(x: 120, y: 408, width: 32, height: 32),
    NSRect(x: 568, y: 408, width: 32, height: 32),
    NSRect(x: 120, y: 120, width: 32, height: 32),
    NSRect(x: 568, y: 120, width: 32, height: 32),
]
let panels = panelFrames.map { frame -> NSPanel in
    let panel = NSPanel(
        contentRect: frame,
        styleMask: [.borderless, .nonactivatingPanel],
        backing: .buffered,
        defer: false
    )
    panel.level = target.level
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.ignoresMouseEvents = true
    panel.order(.above, relativeTo: target.windowNumber)
    return panel
}

advanceWindowServer()
let targetID = CGWindowID(target.windowNumber)
let panelIDs = panels.map { CGWindowID($0.windowNumber) }
precondition(
    orderIsValid(visibleWindowIDs(), target: targetID, panels: panelIDs),
    "initial panel order was not established"
)

target.orderFrontRegardless()
advanceWindowServer()
precondition(
    !orderIsValid(visibleWindowIDs(), target: targetID, panels: panelIDs),
    "bringing the target forward did not reproduce panel order loss"
)

for panel in panels {
    panel.order(.above, relativeTo: target.windowNumber)
}
advanceWindowServer()
precondition(
    orderIsValid(visibleWindowIDs(), target: targetID, panels: panelIDs),
    "relative panel reorder did not restore the invariant"
)

for panel in panels {
    panel.close()
}
target.close()
print("PASS: focus reorder reproduced and repaired")
```

- [ ] **Step 2: Run the probe in the logged-in GUI session**

```bash
swift tests/macos_overlay_focus_probe.swift
```

Expected: `PASS: focus reorder reproduced and repaired`. A precondition failure means the assumed Window Server transition is not reproduced and Task 3 must not be reported as runtime-verified.

- [ ] **Step 3: Commit the probe**

```bash
git add tests/macos_overlay_focus_probe.swift
git commit -m "test: 添加 macOS 聚焦层级真机探针"
```

### Task 5: Full verification and runtime handoff

**Files:**
- Verify: all modified Rust files
- Verify: `docs/macos-overlay-acceptance.md`

- [ ] **Step 1: Format and reject whitespace errors**

```bash
cargo fmt --all
cargo fmt --all --check
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 2: Run the complete feature test suite**

```bash
cargo test --all-targets --features macos-screencapturekit
```

Expected: every non-ignored test passes with zero failures.

- [ ] **Step 3: Run Clippy for production paths**

```bash
cargo clippy --all-targets --features macos-screencapturekit -- -D warnings
```

Expected: exit 0. If the repository's pre-existing `is_h264_idr` dead-code warning still fails `-D warnings`, record that exact pre-existing warning and rerun Clippy scoped to the modified macOS overlay target before reporting status.

- [ ] **Step 4: Review the actual diff and repository state**

```bash
git diff HEAD~4 -- src/overlay/macos tests/macos_overlay_policy.rs \
  tests/macos_overlay_focus_probe.swift
git status --short --branch
```

Expected: only the planned macOS overlay files and any explicitly committed design/plan documents differ; `.superpowers/brainstorm` remains untracked and must not be staged.

- [ ] **Step 5: Runtime acceptance on the user's shared-window flow**

Start window sharing, click inside the shared window at least 20 times, and verify:

1. no corner remains beneath the target beyond 100 ms plus one main RunLoop turn;
2. clicking a different covering window does not pull overlay above it;
3. moving and resizing still hide the overlay until geometry stabilizes;
4. no Accessibility or Input Monitoring permission prompt appears.

Record the automated test results separately from this user-visible runtime check; do not claim the visual symptom is fixed until this check is observed.
