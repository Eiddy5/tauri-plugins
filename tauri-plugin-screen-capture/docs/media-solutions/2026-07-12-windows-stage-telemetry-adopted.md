# Windows 捕获链路分阶段丢帧统计

> 状态：**已采纳（ADOPTED）**
> 采纳标记：`ADOPTED-WINDOWS-STAGE-TELEMETRY-2026-07-12`

## 问题背景

Windows WGC callback 在入口 channel 满时直接返回，但旧统计只记录 pipeline 内 latest-frame 替换或 publisher 返回 `None`。因此示例曾出现 `Captured 6 / Published 5 / Dropped 0 / FPS 1.0`，无法判断是捕获流丢失、编码阻塞还是渲染卡顿。

## 解决目标

保持现有 `CaptureStats` 字段兼容，同时明确区分捕获入口、pipeline 和编码阶段的丢帧，并分别展示捕获/发布 FPS 与实际编码后端。

## 已落地方案

新增兼容字段：

- `framesCaptureDropped`
- `framesPipelineDropped`
- `framesEncoderDropped`
- `captureFps`
- `publishFps`
- `encoderBackend`

Windows callback 在容量检查和 `try_send(Full)` 两处累计入口丢帧；pipeline 记录待处理帧替换；编码 worker 记录编码队列替换、空输出和发送失败。`ScreenCaptureState` 汇总三个阶段，`framesDropped` 为阶段总和。新增字段带 serde 默认值，旧前端 payload 仍可反序列化。

## 验证结果

- legacy `CaptureStats` 反序列化契约测试通过。
- pipeline 阶段替换计数测试通过。
- 全量测试通过。

## 参考

- 项目实机诊断记录与 `CaptureStats` 公共契约。
- [OBS Stats documentation](https://obsproject.com/kb/encoding-performance-troubleshooting)
