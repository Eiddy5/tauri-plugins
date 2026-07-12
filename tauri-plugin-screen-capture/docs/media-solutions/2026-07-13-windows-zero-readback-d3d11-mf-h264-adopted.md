# Windows D3D11 surface 到 Media Foundation H264 零读回方案

> 方案状态：**ADOPTED（已采纳）**
>
> 采纳日期：2026-07-13
>
> 采纳标记：`ADOPTED-WINDOWS-ZERO-READBACK-D3D11-MF-H264`

## 问题背景

此前 Windows 路径虽然使用 D3D11 完成缩放，但仍把完整 BGRA 帧读回 CPU，再由 CPU 转换为 NV12 后送入编码器。1080p60 下反复跨 PCIe/共享内存边界复制和颜色转换会占用 CPU、增加同步等待，也使快速画面变化时更容易产生处理积压。

## 为解决什么问题

- 正常路径不再执行整帧 CPU readback。
- 缩放和 BGRA→NV12 都由 D3D11 Video Processor 完成。
- 原生动态库/系统组件完成硬件 H264 编码，不启动外部 `.exe`。
- 保留明确、可观测的软件降级路径，避免硬件能力不足时直接中断共享。

## 已采纳方案

1. Windows Graphics Capture 提供 D3D11 BGRA texture。
2. `WindowsGpuProcessor` 使用 D3D11 Video Processor 缩放并转换为 NV12，输出来自四槽有界 GPU surface 池。
3. `WindowsGpuSurface` 只在 Windows 私有模块间传递，不进入跨平台公共 API。
4. Media Foundation encoder 创建 `IMFDXGIDeviceManager`，通过 `MFT_MESSAGE_SET_D3D_MANAGER` 绑定同一 D3D11 device。
5. 每个 NV12 texture 通过 `MFCreateDXGISurfaceBuffer` 包装为 `IMFMediaBuffer`，直接作为 MFT 输入，输出 Annex-B H264。
6. 只有 D3D11/MFT 初始化或运行失败时才调用 `readback_nv12`，切换 CPU Media Foundation/OpenH264 fallback，并累计 `framesCpuReadback`。

## 验证

- GPU surface 捕获测试确认输出格式为 NV12、正常路径 `framesCpuReadback == 0`。
- 直接 GPU surface 编码测试确认 `media-foundation-d3d11-surface` 后端可以持续产生 H264 样本且 readback 为 0。
- 示例 App 实测正常路径约捕获 57 FPS、发布 52 FPS，UI 显示 readback 0；无并发高负载时发布可稳定接近 59–60 FPS。
- fallback 保留 NV12 readback，硬件失败不会静默伪装成零读回。

## 参考

- [MFCreateDXGISurfaceBuffer](https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mfcreatedxgisurfacebuffer)
- [MFCreateDXGIDeviceManager](https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mfcreatedxgidevicemanager)
- [IMFDXGIDeviceManager::ResetDevice](https://learn.microsoft.com/en-us/windows/win32/api/mfobjects/nf-mfobjects-imfdxgidevicemanager-resetdevice)
- [MFT_MESSAGE_SET_D3D_MANAGER](https://learn.microsoft.com/en-us/windows/win32/api/mftransform/ne-mftransform-mft_message_type)
- [Supporting Direct3D 11 Video Decoding in Media Foundation](https://learn.microsoft.com/en-us/windows/win32/medfound/supporting-direct3d-11-video-decoding-in-media-foundation)

## 已知边界

零读回取决于显卡驱动是否提供兼容的 D3D11 Video Processor 和 D3D11-aware H264 hardware MFT。`framesCpuReadback > 0` 或后端名称不再是 `media-foundation-d3d11-surface` 表示已进入降级路径，不应仍宣称当前会话为全 GPU。
