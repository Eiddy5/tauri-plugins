# Windows H264 编码样本有序交接方案

> 方案状态：**ADOPTED（已采纳）**
>
> 采纳日期：2026-07-13
>
> 采纳标记：`ADOPTED-WINDOWS-ORDERED-H264-SAMPLE-HANDOFF`

## 问题背景

Windows 示例 App 在共享内容快速变化、窗口快速拖动或尺寸快速变化时，接收视频偶发花屏。RTP Receiver Report 的 `total_lost` 没有增加，但接收端产生 PLI，说明问题发生在 RTP 包序号形成之前，而不是网络丢包。

原实现把编码后的 `EncodedVideoSample` 放入容量一 latest slot。WebRTC `send_sample` 短暂背压时，新编码样本会覆盖尚未发送的旧样本。H264 P 帧依赖此前参考帧；本地丢掉一个已经编码的参考帧后，后续 P 帧仍引用该帧，解码器只能显示损坏宏块并请求关键帧恢复。

## 为解决什么问题

- 快速画面变化时不再因本地丢失已编码参考帧而花屏。
- 保持实时低延迟：背压时仍可淘汰尚未进入编码器的旧原始帧。
- 让编码丢帧统计只表示安全的编码前淘汰，不混入会破坏码流的编码后覆盖。

## 已采纳方案

1. 编码前继续使用容量一 `LatestSlot<EncoderInput>`，新原始帧可替换尚未编码的旧帧。
2. 编码后改用容量一 `OrderedSlot<EncodedVideoSample>`；槽位占用时编码线程等待，禁止覆盖样本。
3. transport 消费一个样本后唤醒编码线程，所有 H264 access unit 按编码顺序进入 WebRTC。
4. transport 背压会传回编码线程，但不会传回捕获回调；此时原始帧 latest slot 安全地淘汰过期画面。
5. stop 同时停止并唤醒两个 slot，避免编码线程在满槽等待时无法退出。
6. transport 发送失败/超时属于不可避免的编码后丢失：立即请求 IDR，并在 IDR 成功发送前丢弃已预编码的旧 P 帧，禁止把损坏的参考链继续交付。

## 诊断与验证数据

同一台 Windows 机器、同一示例 App、同一快速内容变化操作：

| 交接策略 | 编码前替换 | 编码后替换 | 接收端 PLI | 结果 |
| --- | ---: | ---: | ---: | --- |
| 编码后 latest 覆盖 | 0 | +87，后续一次 +129 | 增加 | 出现花屏，REMB 一度降至 5000 bps |
| 编码后 ordered handoff | +53 | 0 | 0 | 无花屏，REMB 恢复约 4.4 Mbps，发布约 59–60 FPS |

自动回归测试 `encoded_sample_handoff_preserves_order_under_backpressure` 验证：第二个编码样本在第一个样本被消费前必须等待，消费后按 `1, 2` 顺序交付，不能替换。

## 参考

- [RFC 6184: RTP Payload Format for H.264 Video](https://www.rfc-editor.org/rfc/rfc6184)：H264 NAL 单元按解码顺序传输，丢失参考数据会影响后续预测帧。
- [RFC 4585: Extended RTP Profile for RTCP-Based Feedback](https://www.rfc-editor.org/rfc/rfc4585)：PLI 用于接收端无法继续正确解码并请求刷新画面的反馈。
- [WebRTC for the Curious — Video](https://webrtcforthecurious.com/docs/04-media-communication/#video)：说明视频帧间预测与关键帧恢复关系。

## 已知边界

该方案保证插件内部不会主动丢弃已经编码的 H264 样本，但不能消除真实网络丢包、驱动产生非法码流或接收端渲染故障。网络拥塞控制仍可通过降低码率、帧率或分辨率控制负载。
