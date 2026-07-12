# Windows media module

该模块承载 Windows 实时媒体路径中必须具有单线程所有权的职责。

- `encoder_worker.rs`：编码前 latest-frame 调度、编码后有序样本交接、相互独立的编码/传输线程、keyframe/stop 与编码阶段统计。
- `gpu_processor.rs`：D3D11 Video Processor 在 GPU 上完成 BGRA→NV12、缩放，并管理有界输出 surface 池。
- `gpu_surface.rs`：模块私有的 D3D11 NV12 surface 所有权与显式 CPU fallback readback。
- `media_foundation.rs`：通过 DXGI device manager 和 `MFCreateDXGISurfaceBuffer` 把 NV12 D3D11 surface 交给低延迟硬件 H264 MFT，支持运行时码率更新并输出 Annex-B。

稳定边界：捕获回调只提交最新 GPU surface；背压只允许淘汰尚未编码的原始帧；编码后的 H264 样本必须严格有序且不可覆盖。WebRTC REMB 以 85% headroom 驱动硬件编码动态码率，最低保留目标最大码率的 1/8，避免高运动帧持续压垮 transport。正常硬件路径不做整帧 CPU readback，`framesCpuReadback` 必须保持为 0；硬件或驱动不支持时才显式读回 NV12 并降级，同时通过 `encoderBackend` 和统计暴露真实状态。`ID3D11Texture2D` 不暴露到跨平台公共 API。
