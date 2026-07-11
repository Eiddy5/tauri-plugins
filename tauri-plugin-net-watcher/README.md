# Tauri Plugin Net Watcher

`tauri-plugin-net-watcher` 是一个用于 Tauri 2 应用的网络状态监测插件。它会采集本机网络接口信息，并可选地探测一个或多个自定义 HTTP 服务目标。

## 功能特性

- 不配置 `targets` 也能运行，独立输出本地网络接口状态和公网可用状态。
- Windows 使用 NCSI 作为系统提示，并通过 Microsoft Connect Test 主动探测作为公网可用性的最终验证。
- macOS 使用 SystemConfiguration reachability 作为系统提示，并通过 Apple captive 检测页主动验证公网与认证门户。
- 每个自定义 target 独立计算服务可达性、连接质量和核心统计。
- 多目标使用固定上限为 4 的有界并发探测，避免目标数量线性放大单轮等待时间。
- 每个 target 独立提供失败率、P95 延迟、抖动和连续失败次数。
- 分别监听设备/公网更新和单 target 更新事件，前端可按页面区域增量刷新；Windows 和 macOS 网络变化会触发插件立即刷新。
- 支持手动启动、停止监测，也支持通过配置自动启动。
- 返回主网络接口、接口类型、IP 地址、DNS 和网关；Windows 通过系统适配器 API 采集 DNS/网关，macOS 按系统可得信息返回。
- 首个版本支持 Windows 和 macOS；MAC 地址默认不采集。

## 安装

在 Tauri 应用中安装 Rust 插件和前端 API 包后，在 `src-tauri` 中注册插件，并在前端通过 `tauri-plugin-net-watcher-api` 调用。

```ts
// src-tauri/src/lib.rs
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_net_watcher::init())
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
```

## 配置

在 `src-tauri/tauri.conf.json` 的顶层添加 `plugins.net-watcher`：

```json
{
  "plugins": {
    "net-watcher": {
      "autoStart": false,
      "targets": [
        { "id": "api", "url": "https://api.example.com/health" },
        { "id": "cdn", "url": "https://cdn.example.com/ping" }
      ],
      "intervalMs": 10000,
      "timeoutMs": 3000
    }
  }
}
```

配置项说明：

- `autoStart`：应用启动后是否自动开始监测。
- `targets`：可选的服务可达性目标数组；配置目标时每个 `id` 必须唯一。
- `intervalMs`：target 健康状态下的基础探测间隔，单位毫秒，允许范围为 `1000` 到 `3600000`；异常确认、恢复确认和失败退避由插件内部动态调度。
- `timeoutMs`：单次探测超时时间，单位毫秒，允许范围为 `100` 到 `60000`，且不能大于 `intervalMs`。

未配置 `targets` 或传入空数组时，不会探测任何自定义业务服务。Windows 公网验证会访问 Microsoft Connect Test，macOS 公网验证会访问 Apple captive 检测页；这些请求不是 target，也不参与 target 的统计。旧的单目标 `target` 字段不再接受。

后台 watcher 使用“系统网络事件唤醒 + 独立自适应调度”的混合模型。系统网络变化会在短窗口合并后比较网络指纹；主接口、地址、网关或 DNS 变化时立即废弃旧探测，重置公网和各 target 挡位及质量窗口。公网优先验证并发送网络事件，各 target 最多并发 4 个且完成一个就独立推送一个。健康 target 以 `intervalMs` 为基础周期并加入 ±10% 抖动；失败先按 3 秒快速确认，持续失败后按 10、20、40、60 秒退避，恢复后按 3 秒确认。公网明确不可用时 targets 进入 `suspended`，不累计服务失败样本；公网恢复后立即重新探测。

## 前端用法

```ts
import {
  getSnapshot,
  onNetworkUpdated,
  onTargetUpdated,
  startWatching,
  stopWatching,
} from 'tauri-plugin-net-watcher-api'

let unlistenNetwork: (() => void) | null = null
let unlistenTarget: (() => void) | null = null

export async function startNetWatcher() {
  unlistenNetwork = await onNetworkUpdated((payload) => {
    console.log('internet:', payload.state.internet)
    console.log('network:', payload.network)
  })
  unlistenTarget = await onTargetUpdated((payload) => {
    console.log('target:', payload.target.id, payload.target.state)
  })
  await startWatching()
  return await getSnapshot()
}

export async function stopNetWatcher() {
  await stopWatching()
  return await getSnapshot()
}

export async function refreshSnapshot() {
  return await getSnapshot()
}

export function cleanupNetWatcherListener() {
  unlistenNetwork?.()
  unlistenTarget?.()
  unlistenNetwork = null
  unlistenTarget = null
}
```

`getSnapshot()` 返回完整 `NetWatcherSnapshot`，用于初始化和诊断。实时推送使用两个精简事件：

- `net-watcher://network-updated`：仅包含 `snapshotId`、`timestamp`、`platform`、`state` 和 `network`，与示例顶部设备/公网区域一致。
- `net-watcher://target-updated`：仅包含 `snapshotId`、`timestamp` 和一个 `target`，每个 target 独立推送。

## 快照字段

常用字段包括：

- `snapshot.state.overall`：设备网络接口总体状态，不受自定义 targets 成败影响。
- `snapshot.state.network`：设备网络接口状态。
- `snapshot.state.internet`：业务可直接使用的公网状态，取值为 `unknown`、`available`、`degraded`、`unavailable` 或 `captivePortal`。
- `snapshot.state.reason`：当前状态原因。
- `snapshot.network.internet`：公网判断的详细证据，包括平台系统提示、主动探测、是否已验证、延迟、连续失败次数和原因。
- `snapshot.reachability.config`：所有目标共用的探测周期、超时和窗口大小。
- `snapshot.reachability.targets`：相互独立的服务探测实例列表，每个目标包含自己的 `state`、`currentProbe` 和滚动统计。
- `snapshot.reachability.targets[].currentProbe.http.statusCode`：目标返回的原始 HTTP 状态码；任何合法 HTTP 响应都表示网络可达。
- `snapshot.changes.changedTargetIds`：本次发生语义变化的目标 ID 列表。
- `snapshot.network.primaryInterfaceId`：主网络接口 ID。
- `snapshot.network.interfaces`：网络接口列表。

## 平台说明

首个版本目标平台为 Windows 和 macOS。Windows 会采集接口类型、IP、DNS 和默认网关，并提供 NCSI + Microsoft Connect Test 公网验证。macOS 会采集接口基础信息、通过 `SCDynamicStore` 监听网络变化，使用 `SCNetworkReachability` 作为系统提示，并通过 `NSURLSession` 请求 Apple captive 检测页完成公网验证和认证门户识别。两端共用网络指纹重置、失败收敛、退避调度和 target 自适应逻辑。MAC 地址默认不采集，相关字段通常返回 `null`。
