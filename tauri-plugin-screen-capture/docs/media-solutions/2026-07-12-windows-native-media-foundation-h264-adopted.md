# Windows 原生 Media Foundation H264 硬件编码

> 状态：**已采纳（ADOPTED）**
> 采纳标记：`ADOPTED-WINDOWS-MF-H264-2026-07-12`

## 问题背景

旧实现通过启动 FFmpeg `.exe` 尝试 `h264_amf`、`h264_qsv` 或 `h264_nvenc`。这增加了进程部署、版本、签名、日志管道和崩溃恢复成本，也不符合插件只能使用动态库/系统 API 的约束。FFmpeg 不存在时，同步 OpenH264 在 Debug 1080p 输入下只有约 2.84 FPS，直接造成示例 App 视频卡顿。

## 解决目标

- 不启动外部编码进程。
- 通过 Windows 系统 DLL 和显卡驱动注册的硬件 MFT 编码 H264。
- 使用 NV12、低延迟单帧输入输出和 Annex-B 样本，失败时明确降级到 OpenH264。
- 公开实际选择的 `encoderBackend`，不能把软件降级误报成硬件。

## 已落地方案

`MediaFoundationH264Encoder` 调用 `MFStartup`、`MFTEnumEx(MFT_ENUM_FLAG_HARDWARE)`、`IMFTransform` 与异步 `IMFMediaEventGenerator`。输出类型先设为 H264，输入类型为 NV12；硬件 MFT 的 `MF_TRANSFORM_ASYNC_UNLOCK` 在媒体类型协商前设置。资源在所属 MTA 线程创建、使用和释放，MFT 引用释放后才调用 `MFShutdown`/`CoUninitialize`。事件获取采用 non-blocking poll 和 2 秒上限，驱动失去响应时转入软件降级，避免停止共享永久卡住。

WebRTC PLI/FIR 触发关键帧时，通过 `ICodecAPI::SetValue(CODECAPI_AVEncVideoForceKeyFrame)` 请求硬件 MFT 产生 IDR，不再用 flush/start-of-stream 近似代替关键帧请求。

编码器选择顺序为：

1. Windows Media Foundation hardware MFT；
2. OpenH264 软件降级；初始化或运行时降级都会同步更新 `encoderBackend`。

旧 FFmpeg 进程、stdin/stdout 管道与外部编码器探测代码已删除。

## 验证结果

- 当前 AMD Windows 实机，1080p60，Release：180/180 帧，约 79.08 FPS。
- 生命周期修复后的复测：180/180 帧，约 58.89–62.16 FPS，仍满足 55 FPS 编码吞吐门槛。
- 对 Debug 像素转换依赖启用优化后：180/180 帧，约 71.67 FPS；优化前约 13.53 FPS。
- H264 AVCC/Annex-B 归一化、Media Foundation ratio packing 单测通过。
- 硬件关键帧测试确认 `CODECAPI_AVEncVideoForceKeyFrame` 后的访问单元包含 IDR NAL（type 5）。
- 全量自动测试通过；硬件性能测试独立通过。

## 尚存边界

当前输入在进入 MFT 前仍由 CPU 完成 BGRA→NV12。D3D11 texture→NV12 surface→MFT 的完整零读回属于独立 Phase 2 方案，未在本文宣称完成。

## 参考

- [Microsoft H.264 Video Encoder](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder)
- [MFTEnumEx](https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mftenumex)
- [Hardware MFTs](https://learn.microsoft.com/en-us/windows/win32/medfound/hardware-mfts)
- [IMFTransform::ProcessInput](https://learn.microsoft.com/en-us/windows/win32/api/mftransform/nf-mftransform-imftransform-processinput)
- [CODECAPI_AVLowLatencyMode](https://learn.microsoft.com/en-us/windows/win32/medfound/codecapi-avlowlatencymode)
- [CODECAPI_AVEncVideoForceKeyFrame](https://learn.microsoft.com/en-us/windows/win32/medfound/codecapi-avencvideoforcekeyframe-property)
