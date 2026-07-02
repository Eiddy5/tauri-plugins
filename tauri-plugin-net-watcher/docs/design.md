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

后台 watcher 启动后按 `intervalMs` 循环：

1. 读取系统网络接口快照。
2. 对 `target` 执行 HTTP/HTTPS 主动探测。
3. 把探测结果写入滚动窗口。
4. 根据接口可用性、失败率、P95 延迟、连续失败次数计算状态。
5. 生成完整快照并更新内部状态。
6. 若网络、质量汇总或状态发生语义变化，则发送 `snapshot-updated` 事件。

