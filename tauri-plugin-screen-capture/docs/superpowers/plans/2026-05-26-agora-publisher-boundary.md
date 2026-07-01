# Agora Publisher Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a configurable Agora publisher path without changing the macOS or Windows capture backends.

**Architecture:** Keep `CapturePipeline` unchanged: it receives normalized `VideoFrame` values and forwards them to a selected `CapturePublisher`. `StartCaptureOptions` chooses the publisher, defaulting to WebRTC loopback for backward compatibility. `AgoraPublisher` is introduced as the SDK boundary and validates required Agora session options before native SDK binding work.

**Tech Stack:** Rust 2021, Tauri 2 commands, serde models, existing `CapturePublisher` trait, Tokio tests.

---

### Task 1: Publisher Configuration Model

**Files:**
- Modify: `src/models.rs`
- Modify: `guest-js/index.ts`
- Modify: `dist-js/index.d.ts`
- Test: `tests/model_contract.rs`

- [ ] Write failing tests proving `StartCaptureOptions` defaults to WebRTC and accepts Agora options.
- [ ] Add `PublisherKind`, `PublisherOptions`, and `AgoraPublisherOptions`.
- [ ] Add `publisher: Option<PublisherOptions>` to `StartCaptureOptions`.
- [ ] Update TypeScript declarations.
- [ ] Run `cargo test --test model_contract`.

### Task 2: Publisher Factory And Agora Boundary

**Files:**
- Create: `src/publisher/agora.rs`
- Modify: `src/publisher/mod.rs`
- Modify: `src/state.rs`
- Test: `tests/session_state.rs`

- [ ] Write failing tests proving WebRTC remains default and Agora selection validates required config.
- [ ] Add `AgoraPublisher` implementing `CapturePublisher`.
- [ ] Add a small state-side publisher factory.
- [ ] Return structured `CaptureErrorCode::PublisherUnsupported` for missing SDK-backed publishing at this stage.
- [ ] Run `cargo test --test session_state`.

### Task 3: Verification

**Files:**
- Modify only if tests expose compile or API gaps.

- [ ] Run `cargo test --test model_contract --test session_state --test publisher_composite`.
- [ ] Run `cargo check`.
- [ ] Report remaining SDK integration work explicitly.
