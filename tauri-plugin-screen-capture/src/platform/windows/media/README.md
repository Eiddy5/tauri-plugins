# Windows media module

该模块承载 Windows 实时媒体路径中必须具有单线程所有权的职责。

- `encoder_worker.rs`：latest-frame 调度、相互独立的编码/传输线程、keyframe/stop 与编码阶段统计。
- `gpu_processor.rs`：D3D11 Video Processor 的 BGRA GPU 缩放；捕获侧配合三槽 staging ring 延后 readback。
- `media_foundation.rs`：系统/驱动动态提供的 H264 hardware MFT，负责 NV12→H264 与 Annex-B 输出。

稳定边界：捕获回调只提交最新帧；编码和 WebRTC 写入不能反向阻塞捕获；硬件失败必须降级并通过 `encoderBackend` 暴露真实状态。当前缩放已在 GPU 完成，但 GPU 结果仍读回 CPU BGRA，BGRA→NV12 仍为 CPU 路径；后续零读回 surface processor 继续放在本模块内，不能把 `ID3D11Texture2D` 暴露到跨平台公共 API。
