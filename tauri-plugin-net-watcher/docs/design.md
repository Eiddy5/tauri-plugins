# Net Watcher 插件设计

日期：2026-07-02

## 目标

`tauri-plugin-net-watcher` 是一个 Tauri v2 桌面插件，用于在 Windows 和 macOS 上监控设备网络状态与主动探测质量。

第一版回答两个问题：

- 当前设备是否有可用网络接口。
- 配置的 HTTP/HTTPS 探测目标是否可达，近期网络质量是否稳定。

Linux、Android、iOS 等平台第一版不实现真实监控；除 `get_config` 外，其它命令返回 `unsupported_platform`。

## 配置

配置从 `tauri.conf.json` 的 `plugins.net-watcher` 读取，只暴露核心字段：

```json
{
  "plugins": {
    "net-watcher": {
      "autoStart": true,
      "target": "https://example.com/health",
      "intervalMs": 10000,
      "timeoutMs": 3000
    }
  }
}
```

字段含义：

- `autoStart`：插件初始化后是否自动开始监控。
- `target`：主动质量探测使用的 HTTP/HTTPS URL。
- `intervalMs`：两次探测之间的间隔。
- `timeoutMs`：单次探测超时时间。

内部质量窗口和阈值由 Rust 私有默认值控制，不作为公开配置契约暴露。

## 数据结构

快照统一使用 `NetWatcherSnapshot`：

- `meta`：快照 ID、时间、平台、插件版本。
- `state`：汇总状态、网络层状态、质量层状态、评分和原因。
- `network`：网络接口列表、主接口、地址、网关和 DNS 预留字段。
- `quality`：探测配置快照、目标、最近一次探测和滚动质量统计。
- `changes`：相对上一份快照的变化字段。

事件名为：

```text
net-watcher://snapshot-updated
```

事件 payload 始终是完整 `NetWatcherSnapshot`。后台任务会更新内部快照；只有出现语义变化时才发送事件，避免仅因 probe id 或时间戳变化产生噪音事件。

## 监控逻辑

后台 watcher 采用“系统网络事件唤醒 + 定时质量探测”的混合模型：

1. watcher 启动时创建后台任务，并尝试订阅系统网络变化事件。
2. Windows 当前通过 `NotifyIpInterfaceChange` 监听网卡、IP 地址和接口状态变化；收到系统回调后立即唤醒一次刷新。
3. macOS 当前通过 `SCDynamicStore` 监听网络配置变化；收到系统回调后立即唤醒一次刷新。
4. 无论是系统事件唤醒还是定时唤醒，刷新流程都相同：读取系统网络接口快照。
5. 对 `target` 执行 HTTP/HTTPS 主动探测。
6. 把探测结果写入滚动窗口。
7. 根据接口可用性、失败率、P95 延迟、连续失败次数计算状态。
8. 生成完整快照并更新内部状态。
9. 若网络、质量汇总或状态发生语义变化，则发送 `snapshot-updated` 事件。

系统网络事件只负责提前触发刷新，不直接等同于前端通知。前端是否收到 `net-watcher://snapshot-updated`，仍取决于新旧快照之间是否存在语义变化。HTTP 目标可达性、延迟和失败率不会总是触发系统网络事件，因此质量探测必须保留定时执行。
