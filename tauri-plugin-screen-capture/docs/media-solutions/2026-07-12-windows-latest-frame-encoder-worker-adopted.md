# Windows latest-frame 独立编码线程

> 状态：**已采纳（ADOPTED）**
> 采纳标记：`ADOPTED-WINDOWS-LATEST-FRAME-WORKER-2026-07-12`

## 问题背景

旧链路在 `WebRtcPublisher::push_frame` 的 Tokio 任务中同步编码，然后等待 WebRTC track 写入。编码或传输变慢时，pipeline worker 被占用；Windows 捕获入口容量为 1，后续帧被跳过，示例 UI 却可能仍显示 `Dropped 0`。

## 解决目标

- 捕获回调和 pipeline 不等待编码或传输。
- 在编码前永远只保留最新一帧，不积压过期画面。
- COM、Media Foundation transform 和编码器在同一专用 MTA 线程创建、使用与释放。
- keyframe、stop 和统计具有明确线程安全语义。

## 已落地方案

新增 `WindowsEncoderWorker` 和容量为 1 的 `LatestFrameSlot`。提交新原始帧时原子替换尚未消费的旧帧并累计 `framesEncoderDropped`；worker 独占编码器。编码完成后的 H264 样本必须通过独立的有序 handoff 发送，不能应用 latest-frame 覆盖。停止时关闭 slot、唤醒线程并 join，避免后台资源悬挂。关键帧请求通过原子标记在编码线程消费。

此方案把编码前延迟上界固定为“一帧正在处理 + 一帧最新待处理”，不会因短时卡顿产生越来越大的直播延迟。编码后的参考帧完整性由独立方案 `2026-07-13-windows-ordered-h264-sample-handoff-adopted.md` 保证。

## 验证结果

- `latest_frame_slot_replaces_stale_frame` 验证新帧替换旧帧。
- pipeline 慢 publisher 测试验证捕获提交不等待下游。
- Windows H264 worker 使用独立线程初始化原生 MFT。
- 全量测试通过。

## 参考

- [OBS hardware encoding overview](https://obsproject.com/kb/hardware-encoding)
- [Media Foundation Hardware MFT asynchronous model](https://learn.microsoft.com/en-us/windows/win32/medfound/hardware-mfts)
