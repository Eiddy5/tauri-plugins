# Windows SIMD Hamming 高清缩放方案

> 方案状态：**已采纳（ADOPTED）**
> 采纳日期：2026-07-12

## 问题背景

旧实现对每个 BGRA 输出像素执行 Rust 标量最近邻采样。2560×1440 缩放到 1920×1080 时，Debug 仅 7.48 FPS、P95 173.85 ms；Release 也只有 42.61 FPS、P95 46.15 ms。最近邻对文字斜线、细边框和缩放后的 UI 文字会产生明显锯齿，不满足清晰度要求。

## 要解决的问题

- 降低每帧缩放 CPU 时间，避免 capture callback 被长时间占用。
- 改善窗口文字、图标与细线条的缩小质量。
- 保持 BGRA 输出、偶数尺寸与现有编码接口不变。

## 采用方案

Windows 目标引入精确固定的 `fast_image_resize = 5.1.2`，使用其 U8x4 SIMD 路径和 Hamming convolution filter。Hamming 在缩小时比最近邻平滑，并针对屏幕内容保留较锐利边缘；关闭 alpha 预乘除，因为捕获帧为不透明桌面 BGRA。

固定 5.1.2 是为了不抬高本插件声明的 Rust 1.77.2 最低版本；6.0.0 需要 Rust 1.87。

## 自动验证

命令：

```powershell
cargo test --release --test windows_capture_performance -- --ignored --nocapture
```

结果：2560×1440→1920×1080 在事件驱动交付同时启用后达到 **58.45 FPS / P95 21.14 ms**，通过 ≥50 FPS 与 P95 ≤34 ms 阈值。尺寸断言保证所有帧均为请求的 1920×1080。

单元测试 `hamming_downscale_antialiases_high_frequency_screen_edges` 使用黑白高频边缘，断言下采样输出含抗锯齿的中间灰度而不是最近邻的纯黑/纯白，并保持不透明 alpha。

## 参考

- 项目源码：[fast_image_resize](https://github.com/Cykooz/fast_image_resize)，列出 U8x4 的 SSE4.1、AVX2、NEON 与 WASM SIMD128 优化。
- API 文档：[FilterType](https://docs.rs/fast_image_resize/latest/fast_image_resize/enum.FilterType.html)，说明 Hamming 与 Bilinear 性能相当，同时下采样质量接近 bicubic 且更锐利。

## 未采用的替代方案

- 保留最近邻：速度与画质基线均未达标。
- Lanczos3：质量高，但 6×6 kernel 对实时 60 FPS 回调成本更高；当前屏幕共享优先选择 Hamming 的质量/延迟平衡。
- `fast_image_resize` 6.x：会把 MSRV 提升到 Rust 1.87，超出插件契约。
