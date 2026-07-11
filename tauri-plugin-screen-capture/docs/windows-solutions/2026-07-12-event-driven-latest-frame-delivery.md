# Windows 事件驱动 latest-frame 交付方案

> 方案状态：**已采纳（ADOPTED）**
> 采纳日期：2026-07-12

## 问题背景

Windows Graphics Capture 已按目标 FPS 产生帧，但旧实现先把帧写入单帧槽，再由独立的 Tokio 定时器按 60 Hz 轮询。捕获回调与定时器相位不一致时，一帧会在槽内等待，后续 WGC 帧又会因为槽已满被跳过。这使原生 2560×1440 捕获在 Release 下只有 43.90 FPS，P95 帧间隔 32.64 ms。

## 要解决的问题

- 消除 WGC 回调之后的第二套时钟和额外排队延迟。
- 维持容量为 1 的 latest-frame 背压，发布慢时丢旧帧，不反向阻塞捕获线程。
- 捕获源关闭或帧转换失败时，仍可靠通知 session 结束。

## 采用方案

用容量为 1 的 Tokio `mpsc` 通道替代“Mutex 单帧槽 + interval 轮询”。WGC 的 `MinimumUpdateIntervalSettings` 是唯一 FPS 节流来源；捕获线程仅在通道有容量时做 GPU→CPU 回读/转换，然后 `try_send`。异步 worker 在帧到达时立即被唤醒。

停止顺序改为先停止 WGC control，再终止接收 worker，避免关闭 receiver 后 capture callback 报伪错误。捕获终止错误通过独立 `watch` 通道传递，不与视频帧争抢容量。

## 自动验证

命令：

```powershell
$env:SCREEN_CAPTURE_BENCH_NATIVE='1'
cargo test --release --test windows_capture_performance -- --ignored --nocapture
```

结果：2560×1440 原生输出从 43.90 FPS / P95 32.64 ms 提升到 **58.09 FPS / P95 20.08 ms**，达到测试阈值（≥50 FPS、P95 ≤34 ms）。

另有端到端命令 `cargo test --release --test windows_capture_performance display_capture_resize_and_h264_encode_stay_realtime_end_to_end -- --ignored --nocapture`，真实执行 display capture → 1080p resize → OpenH264，在无 FFmpeg 环境达到 **55.26 FPS**。

## 参考

- Microsoft Learn：[Windows.Graphics.Capture 屏幕捕获](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)。文档说明 `FrameArrived` 每次新帧可用时触发，并建议不要在回调线程执行重工作。
- Microsoft Learn：[Direct3D11CaptureFramePool.CreateFreeThreaded](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.createfreethreaded)。Free-threaded frame pool 在内部 worker 线程触发 `FrameArrived`，不依赖 UI DispatcherQueue。

## 未采用的替代方案

- 提高轮询频率：仍保留双时钟和相位抖动，而且会增加无效唤醒。
- 无界通道：发布慢时会累积过期帧，增大共享延迟和内存占用。
