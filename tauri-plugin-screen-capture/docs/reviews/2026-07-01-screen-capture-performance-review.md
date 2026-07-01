# 代码评审报告

- 日期：2026-07-01
- 范围：屏幕采集性能专项审查
- 审查文件数：约 18 个热路径相关源码/示例/文档文件

## 🔴 必须修改

1. `src/platform/macos/screencapturekit.rs:480` — macOS 采集每帧先把 `CVPixelBuffer` 拷贝成新的 tightly-packed BGRA buffer，随后编码器又把同一帧再拷贝一次。
   - 当前代码：`tightly_packed_bgra(...).to_vec()` / `Vec::with_capacity(...)` 生成 owned frame；`src/webrtc/h264_encoder.rs:106` 又 `frame.data.to_vec()`。
   - 建议修改：先把 `VideoFrame` 扩展为支持 stride/borrowed/pooled buffer，避免 ScreenCaptureKit 侧强制紧凑化；VideoToolbox 编码侧使用可复用 `CVPixelBufferPool` 或直接消费原始 row stride。
   - 原因：2560x1440 BGRA 单帧约 14 MB，60 fps 下单是这两次拷贝就接近 1.7 GB/s 内存带宽，还没算编码和 WebRTC。这个是 macOS 高分屏共享最先撞到的瓶颈。

2. `src/platform/windows/graphics_capture.rs:301` — Windows 在 capture callback 热路径里做 CPU buffer copy 和最近邻 resize。
   - 当前代码：`frame_to_video_frame` 在 `on_frame_arrived` 中执行；尺寸不匹配时调用 `resize_bgra_nearest`，匹配时也会 `data.to_vec()`。
   - 建议修改：callback 中只保存/移动最新 frame handle 或原始 buffer，缩放和 CPU readback 放到独立 worker；优先走 GPU scaling / encoder scaling / NV12 texture path。
   - 原因：callback 阻塞会反压 Windows Graphics Capture 的 frame arrival。2K/4K 下逐像素 Rust 循环 resize 是 CPU 大头，且会和编码抢核心。

3. `src/publisher/webrtc.rs:81` — WebRTC publisher 在 `push_frame` 内同步执行编码和 `write_sample`，pipeline 没有独立编码队列。
   - 当前代码：`CapturePipeline::push_frame` 直接 `publisher.push_frame(frame).await`；WebRTC publisher 持有 encoder mutex 后同步 encode，再 await `send_sample`。
   - 建议修改：把 capture -> encode/publish 改成 bounded latest-frame queue，采集线程只投递最新帧；编码 worker 按目标 fps 取帧，慢时丢旧帧并计数。
   - 原因：编码或 WebRTC track 写入抖动会直接拖慢采集侧，导致 capture callback/dispatcher 堆积或反复替换帧。当前 stats 也无法准确表达这种背压。

4. `src/webrtc/h264_encoder_windows.rs:216` — Windows FFmpeg 硬编路径每帧写 pipe 后立刻 flush，且通过外部进程 raw BGRA pipe 传整帧。
   - 当前代码：`self.stdin.write_all(&frame.data)` 后 `self.stdin.flush()`。
   - 建议修改：用进程内 Media Foundation / D3D11 texture encoder 替代 FFmpeg pipe；短期至少去掉每帧 flush、批量/异步写入，并给 pipe 写入独立 backpressure 策略。
   - 原因：4K BGRA 每帧约 33 MB，60 fps 通过 pipe 传 rawvideo 接近 2 GB/s。外部进程 + pipe + flush 会带来上下文切换、内核拷贝和不可控延迟。

## 🟡 建议优化

1. `src/platform/macos/screencapturekit.rs:170` — macOS 捕获像素格式固定为 BGRA，后续又编码成 H264。
   - 建议：评估 ScreenCaptureKit 输出 NV12/420 或 texture-backed path；至少在 publisher 是 WebRTC/Agora 时允许选择 encoder-friendly format。
   - 原因：BGRA 对 UI 截图友好，但对视频编码不友好，会把色彩转换成本推给 CPU/VideoToolbox 输入封装。

2. `src/platform/windows/graphics_capture.rs:691` — Windows 同时用 WGC minimum interval 和 tokio interval 做 fps 控制。
   - 建议：只保留一个节流权威，或者明确 WGC 负责采样、worker 负责丢帧；记录 dropped 原因。
   - 原因：双层节流容易出现节奏漂移：callback 先做了昂贵转换，worker 再按 tick 取一帧，前面消耗已经花掉。

3. `src/platform/macos/screencapturekit.rs:124` — macOS `list_sources(include_thumbnails=true)` 串行给每个 source 截图和 PNG/base64 编码。
   - 建议：默认先返回 source metadata，缩略图懒加载或并发限流；缓存短时间内的 thumbnail；只给当前 tab/source viewport 生成。
   - 原因：窗口多时刷新列表会被 N 次截图 + PNG 压缩阻塞，example 默认就传 `includeThumbnails: true`。

4. `src/platform/windows/graphics_capture.rs:347` — Windows 每个窗口 thumbnail 都启动一次 capture session，并同步等待最多 600ms。
   - 建议：改成懒加载、并发限流、缓存；对窗口缩略图使用更便宜的 DWM/窗口预览路径，或只在 hover/选中时生成。
   - 原因：当前虽然限制前 12 个窗口，但最坏仍可能让一次刷新卡数秒，且会短时间创建多个 WGC 会话。

5. `src/pipeline/mod.rs:56` — pipeline 保存 latest frame 时 clone `VideoFrame`，publisher fan-out 也逐个 clone。
   - 建议：这个 clone 目前主要是 `Arc` clone，不是大拷贝，但如果未来支持 texture/buffer pool，需要明确 frame ownership 和 release 时机；建议封装 `FrameRef`。
   - 原因：现在看起来便宜，但会掩盖底层 buffer 生命周期问题，后续接 GPU texture/Agora native 时容易变成隐性同步点。

6. `src/webrtc/h264_encoder.rs:97` — macOS 每帧创建 `CVPixelBuffer::create_with_bytes`，没有 buffer pool。
   - 建议：使用 `CVPixelBufferPool` 或一组复用 buffer，避免每帧分配/释放和 CoreVideo 对象 churn。
   - 原因：高 fps 下对象分配会放大 GC/allocator 压力，表现为周期性卡顿。

7. `src/webrtc/h264_encoder_windows.rs:342` — OpenH264 fallback 每帧 BGRA -> YUV 软件转换。
   - 建议：把 OpenH264 fallback 降级为低分辨率/低 fps 模式，或在 fallback 前明确提示性能风险；优先实现 NV12 input path。
   - 原因：软件色彩转换 + 软件编码不适合作为 2K/4K 屏幕共享的默认兜底。

8. `examples/tauri-app/src/main.js:38` — example 默认 2560 宽、60 fps，对当前 CPU-copy 管线太激进。
   - 建议：默认 30 fps，按源尺寸/平台/编码器能力动态选择 720p/1080p/1440p；给 UI 暴露质量档位。
   - 原因：当前默认设置会把所有热路径问题放大，容易让用户误判插件不可用。

9. `src/models.rs:157` — stats 模型有 fps/bitrate/dropped，但核心实现没有足够的性能观测。
   - 建议：增加 capture callback time、copy/resize time、encode time、publish time、queue depth、drop reason、latest frame age。
   - 原因：没有这些指标，只看 frames captured/published 很难判断瓶颈在采集、拷贝、编码还是 WebRTC。

10. `src/publisher/composite.rs:31` — 多 publisher fan-out 是串行 await。
    - 建议：如果未来 WebRTC + Agora 同时发布，使用独立 bounded queue/worker，慢 publisher 不应拖慢其他 publisher。
    - 原因：当前一个 publisher 卡住会阻塞整个 fan-out 链路。

## 💡 小建议

1. `src/platform/macos/screencapturekit.rs:177` — queue depth 固定为 3，可以根据 fps/分辨率/延迟目标配置。
2. `src/platform/windows/graphics_capture.rs:713` — 手写 nearest resize 应至少替换为 SIMD/图像库/GPU 路径；nearest 对文本边缘质量也差。
3. `examples/tauri-app/src/main.js:267` — stats 每秒 `console.info` 会污染性能观察；调试开关打开时再打印。
4. `src/webrtc/h264_encoder_windows.rs:598` — stdout reader 每个 chunk `to_vec()`，可改成复用 buffer 或 bytes pool。

## 🟢 做得好的地方

1. 没有把原始视频帧走 Tauri IPC，这是性能上最正确的方向。
2. macOS dispatcher 使用 latest-frame replacement，已经有“慢了就保留最新帧”的雏形。
3. `VideoFrame.data` 用 `Arc<[u8]>`，fan-out 时避免了额外大拷贝。
4. Windows 已经有 thumbnail 数量上限，至少避免了对所有窗口无脑开 capture。

## 优先级建议

1. 先做 bounded latest-frame encode queue，把采集和编码解耦。
2. 再消掉 macOS 双重 BGRA 拷贝：stride-aware frame + VideoToolbox buffer pool。
3. Windows 把 resize/readback 移出 callback，并规划 D3D11/NV12/Media Foundation path。
4. 缩略图改成懒加载/缓存，example 默认改 30 fps + 质量档位。
5. 加性能 metrics，否则后面优化只能靠感觉。

## 验证说明

本轮是静态性能审查，没有运行端到端性能基准。上一轮已验证 `cargo test`、`cargo test --features macos-screencapturekit`、根 `npm run build`、example `npm run build` 均通过；这些只能证明编译/单测通过，不能证明采集性能达标。

## 📝 建议追加到 LESSONS.md

1. 屏幕采集热路径中每增加一次全帧 BGRA copy，都要按目标分辨率/fps 估算内存带宽成本。
2. capture callback 里只做 O(1) 投递，不做 resize、编码、PNG、I/O 或同步等待。
3. 截图缩略图和实时视频采集是两条不同性能路径，source list 不应默认阻塞在缩略图生成上。
