# Windows WebRTC REMB 与低延迟硬件编码码率控制

> 方案状态：**ADOPTED（已采纳）**
>
> 采纳日期：2026-07-13
>
> 采纳标记：`ADOPTED-WINDOWS-REMB-LOW-LATENCY-BITRATE-CONTROL`

## 问题背景

编码后的 H264 样本改为有序交接后，常规快速窗口缩放不再产生新增 PLI；但极端快速的全屏内容交替仍可让接收端请求关键帧。日志显示 PLI 前 RTP sequence number 会冻结数秒，`total_lost` 始终为 0，同时 REMB 从约 4.8 Mbps 下跌到 0.47 Mbps、再到 0.14 Mbps。

根因是 Media Foundation 编码器固定按最高 8 Mbps 目标工作，快速变化时产生的码流持续超过 WebRTC transport 当前可发送码率。即使编码样本不再被本地覆盖，发送队列仍会停止前进；接收端长时间拿不到可解码的新帧后发出 PLI。另一个缺口是硬件 MFT 没有显式开启 low-latency/real-time 模式，无法把“一输入一输出、无重排延迟”作为稳定契约。

## 为解决什么问题

- 快速拖动、快速滚动和大面积内容变化时，编码输出不持续压垮 WebRTC transport。
- 保持 H264 样本有序的同时，避免 transport 背压演变为长时间 RTP 停顿。
- 在拥塞时优先保持连续帧率和可解码性，同时保留合理的临时画质下限。
- 让 Media Foundation 硬件编码器显式运行在实时、低延迟模式。

## 已采纳方案

1. RTCP reader 解析 `ReceiverEstimatedMaximumBitrate`，用“是否已收到 + 最新值”把 REMB 传给 Windows 编码 worker；尚无反馈使用初始最大码率，显式 0 bps 必须进入最低码率，二者不能混用。
2. 编码目标采用 `min(maxBitrate, REMB × 85%)`，保留 15% transport/RTCP/突发余量。
3. 初始最低码率曾为 `maxBitrate / 8`（1080p60 为 1 Mbps），实机发现窗口或文字变化后只能逐步补回细节。屏幕共享质量下限现采用 `max(maxBitrate / 2, 1 Mbps)`；当前 1080p60 最大 8 Mbps，因此最低 4 Mbps。网络低于该质量预算时应进入后续分辨率/帧率自适应，而不是继续把 1080p60 压到文字不可读。
4. 码率变化不足 10% 时不重复调用驱动，减少 codec property 抖动。
5. Media Foundation MFT 在设置媒体类型前尝试启用 `CODECAPI_AVLowLatencyMode`、`CODECAPI_AVEncCommonRealTime` 和 `LowDelayVBR`。
6. 运行中通过 `CODECAPI_AVEncCommonMeanBitRate` 更新硬件编码目标；驱动拒绝某个目标时记录一次错误、停止对同一值逐帧重试，并让 `bitrateKbps=0` 表明未应用。OpenH264 fallback 保持原配置，不伪装成已应用。
7. WebRTC 单次 sample 写入限制为 2 秒；超时或发送失败后丢弃该样本并立即请求 IDR，保证 stop 有界。
8. transport 进入等待 IDR 状态，丢弃失败时已经预编码排队的旧 P 帧；检测到 Annex-B NAL type 5 且成功发送后才恢复普通有序发送，避免旧参考 epoch 在新 IDR 前泄漏给接收端。

transport timeout 必须在 Tokio runtime 内部构造；对应实现修复与回归证据单独记录在 `2026-07-13-windows-transport-tokio-runtime-context-fix-adopted.md`。

## 验证数据

同一台 Windows 机器、1920×1080@60 目标、Display 1、同一 Notepad 14 次大文本/小文本交替：

| 方案 | PLI 基线→压力后 | RTP 丢包 | 结果 |
| --- | ---: | ---: | --- |
| 固定 8 Mbps | 2→4 | `total_lost=0` | RTP 序号冻结，REMB 降至约 0.14 Mbps，出现恢复请求 |
| REMB 闭环，300 kbps 下限 | 1→1 | `total_lost=0` | 无新增 PLI |
| REMB 闭环，1 Mbps 下限 | 1→1 | `total_lost=0` | 无新增 PLI，RTP 序号持续前进 |

最终示例状态：捕获约 58.9 FPS、发布 60.0 FPS、`framesCpuReadback=0`、编码后端 `media-foundation-d3d11-surface`。测试结束后 Notepad 内容已恢复。

自动测试 `remb_target_bitrate_keeps_transport_headroom` 覆盖无反馈、正常反馈、超过上限和低于画质下限四种输入。

## 参考

- [AVEncCommonMeanBitRate property](https://learn.microsoft.com/en-us/windows/win32/codecapi/avenccommonmeanbitrate-property)
- [eAVEncCommonRateControlMode](https://learn.microsoft.com/en-us/windows/win32/api/codecapi/ne-codecapi-eavenccommonratecontrolmode)
- [CODECAPI_AVLowLatencyMode](https://learn.microsoft.com/en-us/windows/win32/medfound/codecapi-avlowlatencymode)
- [Codec API Properties](https://learn.microsoft.com/en-us/windows/win32/codecapi/codec-api-properties)

## 已知边界

REMB 是接收端估算，不是画质目标。网络长期低于最低码率时，单纯码率调整不足以维持质量与流畅度，后续应增加分辨率/帧率自适应；但不能通过重新允许编码后样本覆盖来“解决”拥塞。不同硬件 MFT 对动态 CodecAPI 支持不同，调用失败时必须保持当前码率并通过后续遥测暴露，而不能破坏会话。
