# 代码评审报告

- 日期：2026-07-01
- 范围：插件级别全项目审查
- 审查文件数：约 57 个源码/测试/示例/权限/文档文件

## 🔴 必须修改

1. `src/state.rs:80` — `get_capabilities` 在 macOS 默认构建下会错误上报支持采集。
   - 当前代码：`supports_display_capture: cfg!(any(target_os = "macos", target_os = "windows"))`
   - 相关代码：`Cargo.toml:11` 默认 feature 为空；`src/platform/macos/screencapturekit.rs:595` 在未启用 `macos-screencapturekit` 时 `list_sources` 返回空，`start_capture` 返回失败。
   - 建议修改：能力上报应跟实际编译能力一致，例如 macOS 只在 `feature = "macos-screencapturekit"` 时上报 true，或者将该 feature 纳入默认/平台默认构建。
   - 原因：前端会看到 `supportsDisplayCapture=true`，但同一构建无法列源或启动采集，属于公开 API 误导。

2. `src/platform/macos/permissions.rs:3` — macOS 已拒绝权限时 `check_permission` 仍会返回 `notDetermined`，导致前端一直走 request 流程。
   - 当前代码：`CGPreflightScreenCaptureAccess() == false` 一律映射为 `PermissionStatus::NotDetermined`。
   - 相关前端：`examples/tauri-app/src/main.js:223` 每次 start 都先 check 再 request，失败后只显示 `Screen recording permission is denied`。
   - 建议修改：增加可区分 denied/restricted/not-determined 的检测或约定；至少在文档/API 中明确 `check_permission` 无法区分 denied，并提供打开系统设置的引导错误。
   - 原因：macOS 屏幕录制权限被拒后，应用通常不能再次弹系统授权框；当前状态模型会让 UI 提示不准确。

3. `src/state.rs:128` — 运行时采集错误不会进入 session 状态、`last_error` 或事件通道。
   - 当前代码：session 创建时 `status: Publishing, last_error: None`，之后没有任何路径更新 `last_error` 或 `CaptureStatus::Failed`。
   - 相关代码：macOS stream `on_error` 只 log；Windows `on_closed` 写入内部 frame slot 后 worker break，也只 log。
   - 建议修改：让 capture backend 或 pipeline 能回调状态机，更新 session 为 `failed/stopped`，填充 `last_error`，并按设计发出 `screen-capture://error`。
   - 原因：窗口关闭、ScreenCaptureKit runtime error、publisher 失败时，前端只会继续看到旧 session/poll 状态，无法可靠恢复。

4. `src/platform/windows/graphics_capture.rs:217` — Windows 被捕获窗口关闭后状态不会被上层感知。
   - 当前代码：`on_closed` 把 `SourceUnavailable` 放进 `frame_slot`，worker 收到后打印日志并退出。
   - 建议修改：向 `ScreenCaptureState`/pipeline 上报该错误，停止 publisher，移除或标记 session，并使 `get_capture_session/get_capture_stats` 能体现失败。
   - 原因：这是设计文档里要求的 UC-16；目前只能在 stderr 看到错误，前端无结构化信号。

## 🟡 建议优化

1. `src/webrtc/h264_encoder.rs:153` — macOS `force_keyframe` 是空实现。
   - 建议：在 VideoToolbox encode 时支持强制 IDR/keyframe，或在 `accept_webrtc_answer` 的 replay 路径上等待下一个 IDR。
   - 原因：`accept_webrtc_answer` 会主动 `request_keyframe` 并 replay latest frame；但 macOS 不真正产生 keyframe，接收端可能等到周期性关键帧才恢复画面。

2. `src/pipeline/mod.rs:45` — `CaptureStats.fps` 和 `bitrate_kbps` 基本没有被核心 pipeline 计算。
   - 建议：在 pipeline 记录时间窗口，计算实际 capture fps；WebRTC publisher 估算 encoded bitrate 或至少明确返回 0 的含义。
   - 原因：模型和 example UI 暴露 fps/bitrate，但现在主要只有 frame counts 有价值。

3. `src/sources/filter.rs:67` — Windows 系统 UI 过滤规则与平台专用规则不一致，且容易误伤正常 Explorer。
   - 当前核心过滤没有把 `explorer` 当系统 UI；`src/platform/windows/window_filter.rs:1` 会把 `explorer` 整体判为 system UI。
   - 建议：删除未使用/不一致的平台 filter，或统一通过 class/title/bounds 判断 taskbar/start menu，避免按进程名粗暴过滤。
   - 原因：Windows 上大量正常文件管理器窗口也是 Explorer，过滤规则需要可解释且可测试。

4. `examples/tauri-app/src/lib/agoraPublisher.js:19` — example 的 Agora 发布走 Web SDK 自定义 track，而插件原生 Agora publisher 仍是 unsupported。
   - 建议：在 README/example UI 中明确这是“WebRTC preview track -> Agora Web SDK”的示例路径，不是插件 native Agora。
   - 原因：否则容易误以为插件已经完成 Agora Native SDK 集成。

## ✅ 已完成的功能

1. Tauri 插件注册、权限清单、TS wrapper 基本完整：`getCapabilities/checkPermission/requestPermission/listSources/start/pause/resume/stop/stats/WebRTC signaling` 都已导出。
2. 数据模型和结构化错误已建立，含 source、session、stats、WebRTC offer/answer/ICE、publisher options。
3. macOS ScreenCaptureKit 真采集实现存在：列显示器/窗口、缩略图、start/pause/resume/stop、BGRA frame 输出。
4. Windows Graphics Capture 实现已经不是占位：列显示器/窗口、缩略图、start/pause/resume/stop、BGRA frame 输出、fps tick。
5. WebRTC loopback publisher 已实现：PeerConnection、H264 track、offer/answer/ICE、H264 sample sender。
6. Windows H264 encoder 有 FFmpeg hardware 优先 + OpenH264 fallback；macOS 有 VideoToolbox encoder。
7. source filtering 有单元测试覆盖：当前 app、system UI、小窗口、debug reason。
8. CompositePublisher 和 AgoraPublisher 的抽象边界已存在；Agora frame mapping/sink 注入已有测试。
9. example app 已可做 source list、WebRTC preview、stats polling，并提供 Agora Web SDK 发布尝试。

## ⛔ 尚未完成 / 只完成骨架

1. Native Agora SDK adapter 未完成；默认 `kind: "agora"` 会返回 `publisherUnsupported`。
2. 设计文档要求的事件通道未实现：`screen-capture://session-state`、`stats`、`error`、`webrtc-ice-candidate`。
3. runtime error propagation 未完成：采集源关闭/stream error/publisher failure 不会稳定更新 session 状态。
4. `CaptureStatus` 中 `idle/starting/capturing/stopping/stopped/failed` 大多没有真实状态流转，目前主要用 `publishing/paused`。
5. stats 未完整：fps、bitrate、实时 dropped/backpressure 仍不充分。
6. macOS 默认 crate feature 不启用真实采集，能力上报却按平台上报支持。
7. 移动端仍是 scaffold 风格，占位意义大于可用功能。
8. 没有真正的端到端自动化测试覆盖 ScreenCaptureKit/WGC + WebRTC video rendering。

## 🟢 做得好的地方

1. 命令层很薄，平台采集、pipeline、publisher 分层清楚。
2. 没有通过 Tauri IPC 传高频视频帧，方向正确。
3. publisher factory 允许外部注入发布器，适合后续接 Agora Native SDK。
4. 单元测试覆盖了模型契约、状态生命周期、source filter、WebRTC signaling 和 publisher fan-out。

## 验证结果

- `cargo test`：通过。
- `cargo test --features macos-screencapturekit`：通过。
- `npm run build`：通过。
- `examples/tauri-app npm run build`：通过，但 Agora SDK 让 bundle 超过 500 KB，Vite 给出 chunk size warning。
- `npm test -- --help`：失败，原因是根 `package.json` 没有 `test` script。

## 📝 建议追加到 LESSONS.md

1. 能力上报必须基于“实际编译/运行能力”，不能只基于 `target_os`。
2. 带 session/status/error 字段的 API 必须有运行时错误回写路径，否则 UI 会拿到过期状态。
3. feature-gated 平台能力需要同时测试默认构建和 feature 构建。
