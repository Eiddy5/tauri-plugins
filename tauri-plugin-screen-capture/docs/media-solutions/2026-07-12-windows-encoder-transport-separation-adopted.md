# Windows H264 编码与 WebRTC 传输解耦

> 方案状态：**ADOPTED（已采纳）**
>
> 采纳日期：2026-07-12
>
> 采纳标记：`ADOPTED-WINDOWS-ENCODER-TRANSPORT-SEPARATION`

## 问题背景

仅把编码移出捕获回调仍不够：若编码线程同步等待 WebRTC `send_sample`，网络/track 背压会占住同一线程，后续新帧无法及时编码，表现为视频间歇停顿和过期帧堆积风险。

## 为解决什么问题

- WebRTC 写入变慢时不阻塞捕获和硬件编码。
- 实时视频始终偏向最新帧，不积累需要追赶的历史帧。
- 明确线程所有权，让 Media Foundation COM/MFT 在固定 MTA 线程创建、使用和销毁。

## 已采纳方案

1. 捕获/管线只把最新原始帧放入容量一 latest slot；背压时只允许在编码前替换旧原始帧。
2. 专用编码线程拥有 H264 encoder；编码后的 `EncodedVideoSample` 进入容量一、有序、阻塞式 handoff，禁止覆盖已经编码的样本。
3. 专用传输线程独立等待 WebRTC `send_sample`。传输变慢时编码线程在 handoff 边界等待，捕获侧 latest slot 继续淘汰尚未编码的旧原始帧，因此不会累积延迟，也不会破坏 H264 参考链。
4. 停止顺序固定为：停止 frame slot → 停止 sample handoff 并唤醒两端 → join 编码线程 → join 传输线程。
5. 任一线程创建/初始化失败时停止已创建的 slot/thread，避免泄漏后台线程。

## 参考

- OBS 式实时媒体管线：捕获、处理、编码、输出阶段隔离，使用有界/丢旧帧策略保持实时性。
- Windows Media Foundation COM apartment 与 MFT 所有权约束。
- 项目架构文档：`2026-07-12-cross-platform-gpu-media-pipeline-proposal.md`。

## 验证

- latest slot 单元测试验证新帧替换旧帧。
- ordered handoff 单元测试验证编码样本在背压下保持顺序且不会被覆盖。
- worker 初始化失败路径保证已启动 transport 正常停止。
- Windows 示例运行时通过 `framesEncoderDropped`、`publishFps` 和 `encoderBackend` 暴露行为。

## 已知边界

WebRTC 端到端流畅度仍受接收端渲染、网络和系统桌面 dirty-frame 产生速率影响。`latest-frame` 只适用于进入编码器之前的独立原始帧；编码后的 H264 P 帧存在参考依赖，不能使用同一覆盖策略。
