# Windows OpenH264 SIMD 色彩转换与实时码率方案

> 方案状态：**已采纳（ADOPTED）**
> 采纳日期：2026-07-12

## 问题背景

插件优先探测 FFmpeg 的 NVENC/QSV/AMF 硬件编码器，但测试机没有 FFmpeg，实际路径回退为 OpenH264。旧路径用 OpenH264 Rust wrapper 的通用 BGRA→YUV 转换，再以约 39.8 Mbps 编码 1080p60；自动基准只有 39.57 FPS，无法实时消费捕获流。

## 要解决的问题

- 即使宿主机未安装 FFmpeg，也要让软件回退路径实时处理 1080p60。
- 使用适合 H.264 的 BT.709 limited-range I420，避免不必要的中间 RGB 拷贝。
- 把码率控制在实时屏幕共享可持续范围，同时保留文字可读性。

## 采用方案

固定引入 `yuvutils-rs = 0.3.2`，直接使用其运行时检测的 AVX2/SSE BGRA→YUV420 转换，构造 I420 `YUVBuffer` 后交给 OpenH264。1080p60 目标码率由约 39.8 Mbps 调整为约 10 Mbps，范围限制为 6–24 Mbps；硬件 FFmpeg 路径也复用同一目标，减少管道背压和网络突发。

选择 0.3.2 是为了维持 Rust 1.77.2 兼容性；当前新版本声明 Rust 1.82。

## 自动验证

命令：

```powershell
cargo test --release --test windows_encoder_performance -- --ignored --nocapture
```

结果：在明确日志显示“FFmpeg not found，fallback OpenH264”的同一机器上，1080p60 输入吞吐从 **39.57 FPS 提升到 76.48 FPS**，通过 ≥55 FPS 阈值；180 帧中产出 178 个编码 access units。

测试同时断言至少 95% 输入帧必须产生 H.264 access unit，防止仅靠 `enable_skip_frame` 跳过大量帧而出现虚假吞吐通过。

## 参考

- API 文档：[yuvutils-rs](https://docs.rs/yuvutils-rs)，提供 BGRA→YUV420 planar 转换，并包含 x86 SIMD 实现。
- 源码导出表：[bgra_to_yuv420](https://docs.rs/yuvutils-rs/latest/src/yuvutils_rs/lib.rs.html)，确认该转换是公开 API。

## 未采用的替代方案

- OpenH264 `ScreenContentRealTime` usage：实测下降到 18.22 FPS，并产生 screen-content 特性告警，已撤回。
- 先 BGRA→RGB 再走 OpenH264 `RGB8` 快速路径：多一次大内存重排，实测仅 33.97 FPS，已撤回。
- 强制要求用户安装 FFmpeg：部署不可控，不能作为可靠的默认回退方案。
