# 代码评审报告

- 日期：2026-05-21
- 范围：Windows 屏幕采集实现与接入路径
- 审查文件数：8

## 🔴 必须修改

1. `src/platform/windows/graphics_capture.rs:93` — capture source 关闭后只记录错误，不停止会话，后续 tick 仍会继续发布最后一帧。
   - 当前代码：`Err(error) => { eprintln!(...) }`
   - 建议修改：收到 `CaptureErrorCode::SourceUnavailable` 或 frame channel error 后，清空 `latest_frame` 并退出分发 loop，或通过一个 stopped/error 状态让 `RunningCapture`/session 进入 Failed/Stopped。
   - 原因：`on_closed()` 已经发送了 source unavailable，但 `start_capture` 的分发任务只打印日志。由于 `latest_frame` 没清空，`tick.tick()` 分支会继续 clone 并 publish 旧画面，用户看到的是“还在共享”的冻结画面，统计也会继续增长。

2. `src/webrtc/h264_encoder_windows.rs:226` — FFmpeg stdout 按任意 pipe chunk 读取后直接当作一个 H264 sample 发送，可能切断 NAL 或合并多帧，WebRTC 端会出现花屏、卡顿或解码失败。
   - 当前代码：`while let Ok(chunk) = self.stdout_rx.try_recv() { data.extend_from_slice(&chunk); }` 后直接 `h264_to_annex_b(&data)`
   - 建议修改：维护一个持久 bitstream buffer，按 Annex-B start code 或 AVCC length-delimited framing 解析完整 access unit；只有拿到完整 frame/sample 时才返回 `EncodedVideoSample`。更保守的选择是先禁用 FFmpeg pipe backend，优先使用 OpenH264 或可控的 Media Foundation backend。
   - 原因：pipe read 边界不是编码帧边界。当前实现把“当前 drain 到的字节”当成完整 sample，这在实时流中不成立。

3. `src/webrtc/h264_encoder_windows.rs:89` — OpenH264 fallback 没有校验后续帧尺寸是否仍等于 encoder 初始化尺寸。
   - 当前代码：`EncoderBackend::OpenH264(encoder) => encoder.encode_frame(frame)`
   - 建议修改：像 FFmpeg/Media Foundation 一样，在 `SoftwareH264Encoder` 中保存 width/height，并在 `encode_frame` 时拒绝尺寸变化；或者在 Windows capture 层固定输出尺寸，窗口尺寸变化时重建 encoder。
   - 原因：Windows window capture 的 frame 尺寸可能随窗口变化而变化。OpenH264 encoder 是按初始尺寸配置的，直接喂不同尺寸 BGRA 可能导致错误码流或内部失败。

4. `src/platform/windows/graphics_capture.rs:343` — `list_sources(includeThumbnails: true)` 会对每个窗口同步启动一次捕获并最多阻塞 600ms。
   - 当前代码：`window_source()` 内同步调用 `capture_thumbnail(window)`；display 也会同步走 GDI/DXGI thumbnail。
   - 建议修改：缩略图应并发且有总预算，或改成首屏/当前 tab 懒加载；至少限制窗口缩略图数量并避免在源枚举主路径上逐个阻塞。
   - 原因：几十个窗口时，列表刷新可能阻塞数秒到十几秒，Tauri command 会长时间无响应，用户会认为源列表卡死。

## 🟡 建议优化

1. `src/platform/windows/graphics_capture.rs:74` — frame channel 使用 `unbounded_channel`，但 frame 转换在 capture callback 中已经完成。
   - 建议：改成容量为 1 的 channel 或 `Arc<Mutex<Option<Result<VideoFrame>>>>` latest-frame slot；在 callback 中如果下游忙，直接替换旧帧。
   - 原因：当前 callback 对每个 WGC frame 都做 BGRA 拷贝/resize，然后才在 async task 中丢旧帧。快速变化的 2K/4K 屏幕会制造大量无用拷贝和内存压力。

2. `src/platform/windows/graphics_capture.rs:100` — 静态画面会按 tick 重复发布最后一帧。
   - 建议：明确这是为了维持视频流，还是只在新帧到达时发布。如果要重复发布，建议加注释并把 captured/published 统计语义区分清楚。
   - 原因：当前行为会让 Published 持续增长，但实际平台没有新 captured frame，排查卡顿/丢帧时容易误判。

3. `src/platform/windows/permissions.rs:4` — `check_permission()` 把 `GraphicsCaptureSession::IsSupported()` 直接映射为 `Granted`。
   - 建议：区分“API 支持”和“程序化捕获权限/启动会话是否可用”。如果使用 programmatic item creation，考虑接入 `GraphicsCaptureAccess::RequestAccessAsync` 或在 capability 中只表达 supported。
   - 原因：用户看到 Granted，但 `start_capture` 仍可能因为系统策略、受保护窗口或权限限制失败。

4. `src/webrtc/h264_encoder_windows.rs:254` — Media Foundation encoder 实现完整但当前没有被 `H264Encoder::new()` 使用。
   - 建议：如果计划保留，纳入选择顺序，例如 FFmpeg hardware -> Media Foundation -> OpenH264；如果不计划使用，删掉或 feature-gate，避免维护死代码。
   - 原因：这段代码复杂度高，未接入时不会被实际验证，后续容易产生“看起来支持硬编但运行不走”的误解。

5. `src/platform/windows/graphics_capture.rs:657` — `window_from_source_id` 只从 source id 还原 HWND，没有重新校验窗口仍存在、可见、未被过滤。
   - 建议：start 前重新查询 HWND 状态、pid/title/size，并复用过滤规则。
   - 原因：窗口列表和开始共享之间存在时间差，窗口关闭或变成系统 UI/不可见时应返回 `SourceUnavailable`，而不是交给底层捕获失败。

## 💡 小建议

1. `src/platform/windows/graphics_capture.rs` — 文件已经同时包含 source enumeration、thumbnail、capture runtime、resize、GDI、DXGI 转换。
   - 建议：后续按 `sources.rs`、`thumbnails.rs`、`frame.rs`、`graphics_capture.rs` 拆分，和 `docs/windows-capture-implementation.md` 中的文件计划保持一致。

2. `src/platform/windows/window_filter.rs` — 目前没有被直接调用，实际过滤逻辑在 `src/sources/filter.rs`。
   - 建议：要么把 Windows 专属规则合并到通用 filter，要么让 `is_system_ui` 调用平台函数，避免两份规则漂移。

3. `src/webrtc/h264_encoder_windows.rs` — FFmpeg 依赖外部可执行文件。
   - 建议：如果这是产品路径，需要明确打包策略；如果只是开发加速路径，建议 feature-gate 或通过配置显式开启。

## 🟢 做得好的地方

1. Windows backend 已经接入统一 `CaptureBackend`，没有把平台差异泄漏到 JS API。
2. 输出仍是统一 `VideoFrame`，能继续复用 `CapturePipeline`、`WebRtcPublisher` 和后续 `CompositePublisher`。
3. `ScreenCaptureState::default()` 已按 target_os 选择 Windows backend。
4. 缩略图、窗口列表和显示器列表已经复用现有 `CaptureSource` 模型，方向是对的。

## 📝 建议追加到 LESSONS.md

1. 对实时媒体流，pipe/socket/read channel 的字节边界不能当作编码帧边界；H264 必须按 NAL/access unit 解析后再交给 WebRTC。
2. Source closed/error 事件不能只 log；必须改变 session 状态或停止发布，否则用户会看到冻结画面但统计继续增长。
3. 源枚举接口不应同步为每个窗口做昂贵缩略图捕获；缩略图需要并发预算、懒加载或分页。

## 验证记录

- `cargo test`：通过，25 个测试通过。
- `cargo check --features macos-screencapturekit`：通过。
- `examples/tauri-app/src-tauri` 下 `cargo check`：通过。
- `cargo check --target x86_64-pc-windows-msvc`：未完成，当前 macOS 环境缺 Windows MSVC C 标准库/交叉编译工具链，`ring` 构建失败，报 `fatal error: 'assert.h' file not found`。这不是 Windows 代码本身的编译结论，需要在 Windows 机器或完整交叉编译环境重新跑。
