# macOS Overlay Deferred Order Verification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent macOS window sharing from failing because AppKit window ordering is verified before the WindowServer commits it.

**Architecture:** Split window-overlay placement and verification into two main-run-loop phases. `OverlayHost` places panels synchronously, coalesces a pending verification generation, and an `NSTimer` callback on the next main RunLoop validates the latest order; invalid order hides only the overlay and never retroactively fails capture startup.

**Tech Stack:** Rust 2021, Tauri 2, objc2 AppKit/Foundation bindings, CoreGraphics window list, Cargo tests.

---

## File structure

- Modify `src/overlay/macos/model.rs`: add the pure pending-verification state machine used by production code and tests.
- Modify `src/overlay/macos/mod.rs`: export the new model type for the existing macOS policy integration test.
- Modify `tests/macos_overlay_policy.rs`: lock down deferral, coalescing, and cancellation behavior.
- Modify `src/overlay/macos/events.rs`: schedule a one-shot callback on the next main RunLoop.
- Modify `src/overlay/macos/host.rs`: separate native placement from post-RunLoop verification and ignore callbacks for hidden/stopped hosts.

### Task 1: Add the pending-verification state model

**Files:**
- Modify: `tests/macos_overlay_policy.rs`
- Modify: `src/overlay/macos/model.rs`
- Modify: `src/overlay/macos/mod.rs`

- [ ] **Step 1: Write failing state-model tests**

Add the import and tests below to `tests/macos_overlay_policy.rs`:

```rust
use tauri_plugin_screen_capture::overlay::macos::OrderVerificationState;

#[test]
fn order_verification_is_deferred_and_coalesces_latest_generation() {
    let mut state = OrderVerificationState::default();

    assert!(state.request());
    assert_eq!(state.pending_generation(), Some(1));
    assert!(!state.request());
    assert_eq!(state.pending_generation(), Some(2));
    assert_eq!(state.take_pending(), Some(2));
    assert_eq!(state.pending_generation(), None);
}

#[test]
fn hidden_overlay_cancels_pending_order_verification() {
    let mut state = OrderVerificationState::default();
    state.request();

    state.cancel();

    assert_eq!(state.take_pending(), None);
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test --test macos_overlay_policy order_verification
```

Expected: compilation fails because `OrderVerificationState` does not exist.

- [ ] **Step 3: Implement the minimal state model**

Add to `src/overlay/macos/model.rs`:

```rust
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OrderVerificationState {
    generation: u64,
    pending_generation: Option<u64>,
}

impl OrderVerificationState {
    pub fn request(&mut self) -> bool {
        self.generation = self.generation.wrapping_add(1);
        let should_schedule = self.pending_generation.is_none();
        self.pending_generation = Some(self.generation);
        should_schedule
    }

    pub const fn pending_generation(&self) -> Option<u64> {
        self.pending_generation
    }

    pub fn take_pending(&mut self) -> Option<u64> {
        self.pending_generation.take()
    }

    pub fn cancel(&mut self) {
        self.pending_generation = None;
    }
}
```

Export `OrderVerificationState` from `src/overlay/macos/mod.rs` beside the existing model exports.

- [ ] **Step 4: Run the focused test and verify GREEN**

Run:

```bash
cargo test --test macos_overlay_policy order_verification
```

Expected: both focused tests pass.

- [ ] **Step 5: Commit the model cycle**

```bash
git add src/overlay/macos/model.rs src/overlay/macos/mod.rs tests/macos_overlay_policy.rs
git commit -m "test: 覆盖 macOS 浮层延迟校验状态"
```

### Task 2: Defer WindowServer order verification to the next RunLoop

**Files:**
- Modify: `src/overlay/macos/events.rs`
- Modify: `src/overlay/macos/host.rs`

- [ ] **Step 1: Add the next-RunLoop scheduler**

Add this function to `src/overlay/macos/events.rs`:

```rust
pub(crate) fn schedule_order_verification(session_id: u64) {
    let block = RcBlock::new(move |_: NonNull<NSTimer>| {
        super::host::verify_pending_order(session_id);
    });
    // SAFETY: The block captures only a numeric ID and the scheduled timer fires on
    // the current main RunLoop. The RunLoop retains the one-shot timer until it fires.
    let _timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &block)
    };
}
```

- [ ] **Step 2: Split placement from verification in the host**

Add `session_id: u64` and `order_verification: OrderVerificationState` to `OverlayHost`, initialize them in `OverlayHost::new`, and import `events::schedule_order_verification` plus `OrderVerificationState`.

After applying panel frames and calling `order_above`, replace the immediate `ordered_windows` verification block with:

```rust
self.last_frame = Some(geometry.frame);
self.last_level = Some(level);
self.panels_visible = true;
if self.order_verification.request() {
    schedule_order_verification(self.session_id);
}
Ok(())
```

Make `hide_panels` call `self.order_verification.cancel()` before ordering panels out.

- [ ] **Step 3: Add the deferred verification callback**

Add the registry entrypoint:

```rust
pub(crate) fn verify_pending_order(session_id: u64) {
    let result = with_host(session_id, OverlayHost::verify_pending_order);
    if let Err(error) = result {
        tracing::debug!(%error, session_id, "macOS 浮层延迟层级校验失败");
    }
}
```

Add the host method:

```rust
fn verify_pending_order(&mut self) -> Result<()> {
    let Some(generation) = self.order_verification.take_pending() else {
        return Ok(());
    };
    if !self.requested_visible || self.dragging || !self.panels_visible {
        return Ok(());
    }

    let target_id = parse_source_id(&self.target.source_id, "window")?;
    let panel_ids = self.panel_ids();
    let windows = ordered_windows(target_id, &panel_ids)?;
    if verify_relative_order(&windows, target_id, &panel_ids) {
        return Ok(());
    }

    self.hide_panels();
    Err(overlay_error(format!(
        "无法维持目标窗口的相对层级，已隐藏浮层（校验代次 {generation}）"
    )))
}
```

This callback logs a real post-commit failure but cannot propagate into the already completed `start` request.

- [ ] **Step 4: Format and run the focused policy suite**

Run:

```bash
cargo fmt --check
cargo test --test macos_overlay_policy
```

Expected: formatting is clean and every macOS overlay policy test passes.

- [ ] **Step 5: Re-run the original AppKit feedback loop**

Run the Swift WindowServer probe used during diagnosis. Expected: the immediate list lacks the new panels while the post-RunLoop list contains all four immediately above the target, proving the implementation now validates at the meaningful boundary.

- [ ] **Step 6: Commit the runtime fix**

```bash
git add src/overlay/macos/events.rs src/overlay/macos/host.rs
git commit -m "fix: 延迟校验 macOS 浮层窗口层级"
```

### Task 3: Full regression verification

**Files:**
- Verify only; no source changes expected.

- [ ] **Step 1: Run all feature-enabled tests**

```bash
cargo test --all-targets --features macos-screencapturekit
```

Expected: all targets pass with zero failures.

- [ ] **Step 2: Run clippy with warnings denied**

```bash
cargo clippy --all-targets --features macos-screencapturekit -- -D warnings
```

Expected: exit code 0 with no warnings.

- [ ] **Step 3: Verify formatting and worktree scope**

```bash
cargo fmt --check
git diff --check
git status --short
```

Expected: formatting and whitespace checks pass; status contains no uncommitted implementation files.
