# Net Watcher 插件设计

日期：2026-07-02

## 目标

开发一个 Tauri v2 桌面插件，用于在 Windows 和 macOS 上监控设备网络可用性和网络质量。

插件需要回答两个问题：

1. 当前设备正在使用什么网络？
2. 配置的探测目标是否可达，网络质量是否可接受？

第一版只支持 Windows 和 macOS。移动端不在第一版范围内，如果被调用，应返回不支持的平台错误。

## 非目标

- 不做局域网设备发现。
- 不做抓包。
- 不实现自定义长连接或短连接协议栈。
- 不依赖 ICMP 作为唯一探测方式。
- 第一版不实现 Android 或 iOS。

## 配置

插件从 `tauri.conf.json` 的 `plugins.net-watcher` 读取默认配置。

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

第一版只暴露会直接影响核心行为的配置：

- `autoStart`：为 `true` 时，插件初始化后自动开始监控。
- `target`：主动质量探测使用的 HTTP 或 HTTPS URL。
- `intervalMs`：两次质量探测之间的间隔。
- `timeoutMs`：单次探测的超时时间。

内置默认值：

- `autoStart`：`false`
- `target`：`https://www.apple.com/library/test/success.html`
- `intervalMs`：`10000`
- `timeoutMs`：`3000`
- `windowSize`：`20`
- `degradedFailureRate`：`0.15`
- `degradedP95LatencyMs`：`800`
- `offlineConsecutiveFailures`：`3`
- `includeMacAddress`：`false`

`startWatching` 传入的运行时选项可以覆盖本次监控会话的 `target`、`intervalMs` 和 `timeoutMs`。

## 公共 API

通过 Tauri 插件暴露的 Rust 命令：

- `getSnapshot()`：返回最新的 `NetWatcherSnapshot`。
- `startWatching(options?)`：开始网络监控和质量探测。
- `stopWatching()`：停止后台监控任务。
- `getConfig()`：返回最终生效配置。

前端 JavaScript API：

```ts
export function getSnapshot(): Promise<NetWatcherSnapshot>
export function startWatching(options?: StartWatchingOptions): Promise<void>
export function stopWatching(): Promise<void>
export function getConfig(): Promise<NetWatcherConfig>
export function onSnapshotUpdated(handler: (snapshot: NetWatcherSnapshot) => void): Promise<UnlistenFn>
```

插件只暴露一个主要事件：

```text
net-watcher://snapshot-updated
```

事件 payload 始终是完整的 `NetWatcherSnapshot`。前端只需要简单状态时读取 `snapshot.state.overall`；需要诊断详情时再读取 `network` 和 `quality`。

## 快照结构

```json
{
  "meta": {
    "snapshotId": "nw_20260702_012435_001",
    "timestamp": "2026-07-02T01:24:35.218+08:00",
    "platform": "windows",
    "pluginVersion": "0.1.0"
  },
  "state": {
    "overall": "degraded",
    "network": "connected",
    "quality": "unstable",
    "score": 68,
    "reason": "high_latency_and_recent_failures"
  },
  "network": {
    "primaryInterfaceId": "if_wifi_0",
    "interfaces": [
      {
        "id": "if_wifi_0",
        "name": "Wi-Fi",
        "displayName": "Intel(R) Wi-Fi 6 AX201",
        "type": "wifi",
        "status": "up",
        "isPrimary": true,
        "addresses": {
          "ipv4": ["192.168.1.23"],
          "ipv6": ["fe80::a12b:34ff:fe56:7890"],
          "mac": null
        },
        "gateway": "192.168.1.1",
        "dnsServers": ["192.168.1.1", "8.8.8.8"]
      }
    ]
  },
  "quality": {
    "config": {
      "intervalMs": 10000,
      "windowSize": 20,
      "timeoutMs": 3000
    },
    "target": {
      "type": "http",
      "url": "https://example.com/health"
    },
    "currentProbe": {
      "id": "probe_00192",
      "status": "success",
      "startedAt": "2026-07-02T01:24:34.982+08:00",
      "endedAt": "2026-07-02T01:24:35.218+08:00",
      "durationMs": 236,
      "phases": {
        "dnsMs": 28,
        "tcpMs": 74,
        "tlsMs": 91,
        "httpMs": 43
      },
      "http": {
        "statusCode": 204
      },
      "error": null
    },
    "summary": {
      "sampleCount": 20,
      "successCount": 17,
      "failureCount": 3,
      "failureRate": 0.15,
      "latencyMs": {
        "avg": 184,
        "min": 72,
        "max": 822,
        "p95": 610
      },
      "jitterMs": 96,
      "consecutiveFailures": 0,
      "lastSuccessAt": "2026-07-02T01:24:35.218+08:00",
      "lastFailureAt": "2026-07-02T01:23:45.114+08:00",
      "lastFailureReason": "tcp_timeout"
    }
  },
  "changes": {
    "hasChanges": true,
    "previousOverall": "online",
    "currentOverall": "degraded",
    "changedFields": [
      "state.overall",
      "state.quality",
      "quality.summary.failureRate",
      "quality.summary.latencyMs.p95"
    ]
  }
}
```

## 监控逻辑

插件内部由两个后台组件协作完成监控。

`Network Watcher` 负责观察系统网络状态：

- 可用网络接口。
- 主网络接口。
- 接口类型，例如 Wi-Fi、Ethernet、VPN、loopback 或 unknown。
- 接口 up/down 状态。
- IPv4 和 IPv6 地址。
- 默认网关。
- DNS 服务器。

`Quality Monitor` 负责主动探测配置的目标：

- DNS 解析。
- TCP 建连。
- HTTPS 场景下执行 TLS 握手。
- 发送 HTTP 请求并测量响应耗时。
- 记录成功状态、失败原因、状态码和总耗时。

每次探测结果都会写入滚动窗口。滚动窗口负责计算失败率、延迟统计、抖动、连续失败次数，以及最近成功或失败时间。

## 状态机

最终状态由系统网络数据和滚动质量统计共同决定。

- `unknown`：插件刚启动，或样本不足。
- `offline`：没有可用网络接口。
- `localOnly`：有可用网络接口，但配置的目标连续不可达。
- `degraded`：目标可达，但延迟、抖动或近期失败率较差。
- `online`：目标可达，并且近期质量稳定。

状态字段说明：

- `state.overall`：最终状态，取值为 `unknown`、`offline`、`localOnly`、`degraded` 或 `online`。
- `state.network`：系统网络层状态，例如 `unknown`、`disconnected` 或 `connected`。
- `state.quality`：质量探测层状态，例如 `unknown`、`unreachable`、`unstable` 或 `stable`。
- `state.score`：0 到 100 的网络质量分。
- `state.reason`：当前状态的机器可读原因。

初始评分规则：

- 从 100 分开始。
- 根据近期失败率扣分。
- 根据 P95 延迟过高扣分。
- 根据抖动过高扣分。
- 根据连续失败次数扣分。
- 最终分数限制在 0 到 100 之间。

## 平台策略

Windows 和 macOS 共享同一套公共 API、数据模型、状态机、滚动窗口和探测逻辑。

平台相关代码只负责系统网络状态观察：

- Windows 实现通过原生系统 API 读取并监听网卡、IP、网关和 DNS 变化。
- macOS 实现通过原生系统 API 读取并监听接口、路由和 DNS 变化。

如果原生事件订阅不完整或不可靠，插件可以对系统网络状态使用保守的轮询兜底。质量探测逻辑仍保持跨平台一致。

## 错误处理

错误需要结构化并可序列化：

- `unsupported_platform`
- `invalid_config`
- `already_watching`
- `not_watching`
- `probe_failed`
- `system_network_unavailable`
- `internal_error`

探测失败不一定等于命令失败。正常情况下，单次探测失败应该体现在 `quality.currentProbe.error` 中，并更新快照，而不是让整个后台监控任务失败。

## 隐私

默认不返回 MAC 地址。

第一版不暴露包内容、附近设备、SSID 或局域网发现数据。插件只暴露应用健康诊断所需的网络接口元数据和主动探测结果。

## 测试策略

单元测试：

- 配置默认值和运行时覆盖合并。
- 滚动窗口统计。
- 状态机转换。
- 质量评分计算。
- 错误序列化。

可行时补充集成测试：

- `getSnapshot` 在未启动监控前返回有效默认快照。
- `startWatching` 能产生探测结果。
- `stopWatching` 能停止后台任务。
- 事件 payload 是完整快照。

手动平台验证：

- Windows Wi-Fi 断开和重连。
- Windows 在 Wi-Fi 和 Ethernet 之间切换。
- macOS Wi-Fi 断开和重连。
- macOS 切换 DNS 或 VPN。
- 探测目标超时和恢复。

