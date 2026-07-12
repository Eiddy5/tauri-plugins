# Windows D3D11 GPU 缩放与 staging ring

> 方案状态：**ADOPTED（已采纳）**
>
> 采纳日期：2026-07-12
>
> 采纳标记：`ADOPTED-WINDOWS-D3D11-GPU-SCALING-STAGING-RING`

## 问题背景

Windows Graphics Capture 本身可接近 60 FPS，但原实现把 2560×1440 BGRA 读回 CPU 后，再在捕获回调内缩放为 1920×1080。Debug 实测只有约 1.25–1.3 FPS，Release 也只有约 28.49 FPS；同步创建/映射 staging texture 还会让 CPU 等待 GPU，拖慢整个捕获入口。

## 为解决什么问题

- 把最重的整帧缩放从 CPU 移到 D3D11 GPU。
- 避免每帧创建 staging texture。
- 延后 readback，让 GPU 有时间完成前序 copy，降低立即 Map 造成的流水线停顿。
- 保留 CPU fallback，避免不支持 D3D11 Video Processor 的设备直接失效。

## 已采纳方案

1. 使用 `ID3D11VideoDevice` / `ID3D11VideoContext` / `VideoProcessorBlt` 将捕获 BGRA texture 在 GPU 上缩放到目标分辨率。
2. 复用 3 个 staging texture 形成 ring；本帧 copy 到当前槽，优先读取更早提交且已 ready 的槽。
3. 分辨率变化时重建 scaler 与 ring；GPU scaler 初始化失败时仅记录一次并切回 CPU 缩放。
4. 平台 D3D11 类型封装在 Windows media/capture 模块，不进入跨平台公共 API。

## 参考

- Microsoft D3D11 Video Processor API：`ID3D11VideoDevice`、`ID3D11VideoContext::VideoProcessorBlt`。
- OBS 式设计原则：捕获、缩放、颜色处理优先留在 GPU，并以有界队列隔离实时阶段。
- 项目架构文档：`2026-07-12-cross-platform-gpu-media-pipeline-proposal.md`。

## 验证结果

- 优化前，2560×1440→1920×1080 Debug：约 **1.3 FPS**。
- 优化后，同一 Windows/AMD 测试机：捕获最高约 **57 FPS**、发布约 **50 FPS**。
- GPU scaler 失败时仍保留 CPU fallback，不改变 API。

## 已知边界

当前是“GPU 缩放 + 延后 CPU readback”，不是完整零读回：输出仍读回 CPU BGRA，并在 CPU 完成 BGRA→NV12。完整 D3D11 surface→NV12→Media Foundation 属于后续独立方案，未在本文标记完成。
