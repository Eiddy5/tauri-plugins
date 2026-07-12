# Windows 静态帧续发与场景刷新方案（已采纳）

> 状态：**ADOPTED**
>
> 采纳标记：`ADOPTED-WINDOWS-STATIC-FRAME-CADENCE-SCENE-REFRESH`
>
> 日期：2026-07-13
>
> 平台：Windows 10/11
>
> 实现：Windows Graphics Capture + D3D11 surface + Media Foundation H.264 + WebRTC

## 问题背景

窗口共享使用 Windows Graphics Capture 时，静态窗口只在内容发生变化时交付新帧。原编码 worker 也只在收到新捕获帧时编码，导致窗口静止后出现以下连锁问题：

1. 捕获帧率降到接近 0，同时 RTP 包停止前进；
2. 接收端因长时间没有视频包发送 PLI；
3. 窗口重新发生大面积变化时，编码器只收到少量 dirty frame，随后再次停帧；
4. 接收端可能长时间保留变化瞬间的低质量预测画面，表现为文字模糊、细节慢慢补齐，快速拖动时还可能出现局部花屏。

这不是 GPU 捕获或浏览器渲染单点卡顿，而是“按变化交付的捕获源”和“需要连续时钟/连续样本的实时视频传输”之间缺少节拍适配。

## 方案目标

- 静态窗口仍以目标帧率连续输出 RTP，保持解码器、抖动缓冲和反馈链路活跃；
- 静止后的第一次真实内容变化立即刷新完整场景，不依赖数秒渐进恢复；
- 继续使用 D3D11 surface 输入 Media Foundation，不增加 GPU→CPU readback；
- 1080p60 屏幕文字在变化后快速达到可读质量；
- 编码耗时计入节拍，不因固定 sleep 把实际输出帧率压低。

## 已采纳设计

### 1. 最新 GPU surface 按目标节拍续发

编码 worker 保存最后一个 `EncoderInput`。当捕获队列在一个帧周期内没有新帧时，复用该输入并生成递增时间戳，再交给编码器。GPU 路径只克隆 D3D11 surface 句柄及元数据，不进行 BGRA CPU readback。

### 2. 静止后的真实变化触发场景刷新

连续 6 个续发 tick（1080p60 约 100 ms）视为进入静止状态。静止后的第一帧真实捕获帧请求 H.264 keyframe，使大面积窗口变化一次性刷新，而不是等待普通预测帧逐步恢复细节。

### 3. 采用 deadline pacing

下一次续发时间基于 deadline 推进，并扣除本次编码已消耗的时间。若编码已经错过 deadline，则从当前时刻重新安排一个帧周期，避免突发追帧。最初“编码后固定等待 16.7 ms”的实现只能得到约 43–49 FPS，改为 deadline pacing 后实测稳定在 59–60 FPS。

### 4. 屏幕质量码率下限

配套 REMB 策略将 1080p60 的最低编码目标由 1 Mbps 提高到 4 Mbps（`max(maxBitrate / 2, 1 Mbps)`）。网络预算不足时，正确的后续策略是降低分辨率或帧率，而不是继续把 1080p60 压缩到文字不可读。该策略的完整背景和边界见独立文档 [Windows REMB 低延迟码率控制方案](./2026-07-13-windows-remb-low-latency-bitrate-control-adopted.md)。

### 5. 实际码率可观测

publisher 将编码 worker 已应用的 `bitrateKbps` 写入统计状态，示例应用同时显示 FPS、实际目标码率、CPU readback 次数和编码后端，避免只凭主观画面判断链路状态。

## 未采纳方案

### 仅缩短 GOP

缩短 GOP 能限制最坏关键帧等待时间，但不能解决静态 WGC 不交付新帧、RTP 停止的问题；持续增加关键帧还会造成周期性码率峰值。因此保留按需关键帧，以续发节拍解决连续性，以静止后的真实变化解决场景恢复。

### 使用 `CODECAPI_VideoEncoderDisplayContentType`

该属性不适合作为 Windows 10/11 的屏幕内容优化依据，未纳入实现。

## 自动测试

- `first_captured_frame_after_idle_requests_scene_refresh`：连续 6 个续发 tick 后，第一帧真实捕获帧必须请求场景刷新，后续连续真实帧不得重复请求；
- `latest_slot_timeout_keeps_slot_running_for_frame_repeats`：捕获槽超时与停止必须区分，超时用于续发而不是退出 worker；
- `repeat_deadline_accounts_for_encoding_time_without_bursting`：deadline 必须扣除编码耗时，过期时不得突发追帧；
- REMB 策略测试：8 Mbps 上限下的最低质量目标为 4 Mbps。

## Windows 实机验证

测试对象为 1727 × 1197 的 Notepad 窗口，示例应用本地 WebRTC 回环播放。

| 项目 | 结果 |
| --- | --- |
| 小内容切换为 45 行密集文本后 300 ms | 文字已经清晰，无花屏 |
| 同一画面 2.3 s 后 | 与 300 ms 画面一致，无渐进式“慢慢变清晰” |
| 发布帧率 | 59–60 FPS |
| 实际编码目标 | 4000 kbps |
| CPU readback | 0 |
| 编码后端 | `media-foundation-d3d11-surface` |
| 内容切换前后 PLI | 3 → 3，未新增 |
| Receiver Report | RTP sequence 持续推进，`total_lost=0` |
| 退出 | worker 正常停止，无 panic |

WGC 的 `Captured` 指标在静态窗口下仍可能接近 0，这是捕获源只报告变化的正常表现；验收应同时观察 `Published`，其必须保持目标帧率。

## 参考

- Microsoft Learn：[H.264 Video Encoder](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder)
- Microsoft Learn：[AVEncMPVGOPSize property](https://learn.microsoft.com/en-us/windows/win32/codecapi/avencmpvgopsize-property)
- Microsoft Learn：[CODECAPI_AVEncVideoContentType](https://learn.microsoft.com/en-us/windows/win32/medfound/codecapi-avencvideocontenttype)
- Microsoft Learn：[CODECAPI_VideoEncoderDisplayContentType](https://learn.microsoft.com/en-us/windows/win32/medfound/codecapi-videoencoderdisplaycontenttype)

## 决策

本方案已完成单元测试和 Windows 实机回环验证，正式采纳为 Windows 窗口共享的默认编码节拍与场景刷新策略。

**ADOPTED-WINDOWS-STATIC-FRAME-CADENCE-SCENE-REFRESH**
