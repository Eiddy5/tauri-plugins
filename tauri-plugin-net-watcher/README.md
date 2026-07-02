# Tauri Plugin Net Watcher

`tauri-plugin-net-watcher` 是一个用于 Tauri 2 应用的网络状态监测插件。它会采集本机网络接口信息，并通过 HTTP 探测目标地址，生成可用于前端展示和业务判断的网络快照。

## 功能特性

- 获取当前网络总体状态：`unknown`、`offline`、`localOnly`、`degraded`、`online`。
- 计算网络质量评分、状态原因、失败率、P95 延迟和连续失败次数。
- 监听快照更新事件，前端可实时刷新状态面板；Windows 网络变化会触发插件立即刷新。
- 支持手动启动、停止监测，也支持通过配置自动启动。
- 返回主网络接口、接口类型和 IP 地址；网关和 DNS 字段已预留，首版暂不采集。
- 首个版本支持 Windows 和 macOS；MAC 地址采集暂不支持。

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
      "target": "https://www.apple.com/library/test/success.html",
      "intervalMs": 10000,
      "timeoutMs": 3000
    }
  }
}
```

配置项说明：

- `autoStart`：应用启动后是否自动开始监测。
- `target`：HTTP 探测目标地址。
- `intervalMs`：探测间隔，单位毫秒。
- `timeoutMs`：单次探测超时时间，单位毫秒。

后台 watcher 使用“系统网络事件唤醒 + 定时质量探测”的混合模型。系统网络变化会提前触发一次完整刷新；HTTP 目标质量仍按 `intervalMs` 探测。只有新旧快照存在语义变化时，插件才会发送 `net-watcher://snapshot-updated`。

## 前端用法

```ts
import {
  getSnapshot,
  onSnapshotUpdated,
  startWatching,
  stopWatching,
  type NetWatcherSnapshot,
} from 'tauri-plugin-net-watcher-api'

let snapshot: NetWatcherSnapshot | null = null
let unlisten: (() => void) | null = null

export async function startNetWatcher() {
  await startWatching()
  snapshot = await getSnapshot()
}

export async function stopNetWatcher() {
  await stopWatching()
  snapshot = await getSnapshot()
}

export async function refreshSnapshot() {
  snapshot = await getSnapshot()
}

export async function listenSnapshotUpdates() {
  unlisten = await onSnapshotUpdated((nextSnapshot) => {
    snapshot = nextSnapshot

    console.log('overall:', nextSnapshot.state.overall)
    console.log('score:', nextSnapshot.state.score)
    console.log('reason:', nextSnapshot.state.reason)
    console.log('target:', nextSnapshot.quality.target.url)
    console.log('failure rate:', nextSnapshot.quality.summary.failureRate)
    console.log('p95 latency:', nextSnapshot.quality.summary.latencyMs.p95)
  })
}

export function cleanupNetWatcherListener() {
  unlisten?.()
  unlisten = null
}
```

## 快照字段

常用字段包括：

- `snapshot.state.overall`：总体网络状态。
- `snapshot.state.score`：网络质量评分。
- `snapshot.state.reason`：当前状态原因。
- `snapshot.quality.target.url`：当前探测目标。
- `snapshot.quality.summary.failureRate`：探测失败率。
- `snapshot.quality.summary.latencyMs.p95`：P95 延迟。
- `snapshot.network.primaryInterfaceId`：主网络接口 ID。
- `snapshot.network.interfaces`：网络接口列表。

## 平台说明

首个版本目标平台为 Windows 和 macOS。网络接口基础信息和 HTTP 探测可用；MAC 地址在首个版本中暂不支持，相关字段可能返回 `null`。网关和 DNS 字段当前为预留字段，首版会返回 `null` 或空列表。
