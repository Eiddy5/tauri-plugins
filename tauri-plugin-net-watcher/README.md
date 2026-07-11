# Tauri Plugin Net Watcher

面向 Tauri 2 桌面应用的设备网络、公网可用性与 HTTP/HTTPS 服务可达性监控插件。

插件提供三类相互独立的数据：

- 设备是否存在可用网络接口。
- 当前设备是否已验证可以访问公网。
- 每个自定义服务目标是否可达，以及近期连接质量。

## 支持平台

| 平台 | 支持状态 | 系统网络变化监听 | 公网验证 |
| --- | --- | --- | --- |
| Windows | ✓ | `NotifyIpInterfaceChange` | NCSI + Microsoft Connect Test |
| macOS | ✓ | `SCDynamicStore` | `SCNetworkReachability` + Apple captive probe |
| Linux | × | 不支持 | 不支持 |
| Android | × | 不支持 | 不支持 |
| iOS | × | 不支持 | 不支持 |

本插件要求 Rust `1.77.2` 或更高版本。

## 安装

插件由 Rust Core 和 JavaScript Guest bindings 两部分组成。

### 本地源码安装

在 Tauri 应用的 `src-tauri/Cargo.toml` 中添加 Rust 插件：

```toml
[dependencies]
tauri-plugin-net-watcher = { path = "../../path/to/tauri-plugin-net-watcher" }
```

使用你选择的包管理器安装 JavaScript bindings：

```bash
pnpm add file:../../path/to/tauri-plugin-net-watcher
# 或
npm install ../../path/to/tauri-plugin-net-watcher
# 或
yarn add file:../../path/to/tauri-plugin-net-watcher
```

### 包发布后安装

当 Rust crate 和 npm 包发布后，可以使用包名安装：

```bash
cargo add tauri-plugin-net-watcher
pnpm add tauri-plugin-net-watcher-api
```

当前仓库未配置公开 Git 仓库地址，因此本文档不提供 Git dependency 示例。

## 使用

### 注册插件

在 Tauri 应用中注册 Rust 插件：

`src-tauri/src/lib.rs`

```rust
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_net_watcher::init())
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
```

### 配置权限

Tauri 2 默认通过 Capability 控制插件命令访问。将 `net-watcher:default` 添加到使用插件的 capability：

`src-tauri/capabilities/default.json`

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Default desktop capability",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "net-watcher:default"
  ]
}
```

`net-watcher:default` 包含以下权限：

| 权限 | 允许调用的 API |
| --- | --- |
| `net-watcher:allow-get-snapshot` | `getSnapshot()` |
| `net-watcher:allow-start-watching` | `startWatching()` |
| `net-watcher:allow-stop-watching` | `stopWatching()` |
| `net-watcher:allow-get-config` | `getConfig()` |

需要最小权限时，可以不使用 `net-watcher:default`，只声明业务实际调用的 `allow-*` 权限。

### 配置插件

插件配置位于 `src-tauri/tauri.conf.json` 的 `plugins.net-watcher`：

```json
{
  "plugins": {
    "net-watcher": {
      "autoStart": false,
      "targets": [
        {
          "id": "api",
          "url": "https://api.example.com/health"
        },
        {
          "id": "cdn",
          "url": "https://cdn.example.com/ping"
        }
      ],
      "intervalMs": 10000,
      "timeoutMs": 3000
    }
  }
}
```

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `autoStart` | `boolean` | `false` | 插件初始化后是否自动启动监控。 |
| `targets` | `Array<{ id, url }>` | `[]` | 可选的 HTTP/HTTPS 服务探测目标。每个 `id` 必须唯一。 |
| `intervalMs` | `number` | `10000` | target 健康状态下的基础探测间隔，范围为 `1000..=3600000`。 |
| `timeoutMs` | `number` | `3000` | 单次 target 探测超时，范围为 `100..=60000`，且不能大于 `intervalMs`。 |

不配置 `targets` 时，设备网络和公网验证仍然运行，只是不探测自定义服务。

### JavaScript API

建议先注册事件监听器，再启动 watcher，避免遗漏启动阶段的第一条更新事件：

```ts
import {
  getConfig,
  getSnapshot,
  onNetworkUpdated,
  onTargetUpdated,
  startWatching,
  stopWatching,
} from 'tauri-plugin-net-watcher-api'

const unlistenNetwork = await onNetworkUpdated((event) => {
  console.log('device:', event.state.overall)
  console.log('internet:', event.state.internet)
  console.log('network:', event.network)
})

const unlistenTarget = await onTargetUpdated((event) => {
  console.log('target:', event.target.id)
  console.log('reachability:', event.target.state.reachability)
  console.log('quality:', event.target.state.quality)
})

await startWatching()

const config = await getConfig()
const snapshot = await getSnapshot()

// 页面或窗口销毁时清理监听器。
unlistenNetwork()
unlistenTarget()

// 不再需要后台监控时停止 watcher。
await stopWatching()
```

`startWatching()` 支持只对当前监控会话生效的运行时覆盖参数，不会修改 `tauri.conf.json`：

```ts
await startWatching({
  targets: [
    { id: 'api', url: 'https://api.example.com/health' },
  ],
  intervalMs: 5000,
  timeoutMs: 2000,
})
```

| API | 返回值 | 说明 |
| --- | --- | --- |
| `getConfig()` | `Promise<NetWatcherConfig>` | 获取当前插件基础配置。 |
| `getSnapshot()` | `Promise<NetWatcherSnapshot>` | 获取当前完整快照。 |
| `startWatching(options?)` | `Promise<void>` | 启动 watcher，可覆盖当前会话的 targets、间隔和超时。 |
| `stopWatching()` | `Promise<void>` | 停止 watcher。 |
| `onNetworkUpdated(handler)` | `Promise<UnlistenFn>` | 监听设备网络和公网语义变化。 |
| `onTargetUpdated(handler)` | `Promise<UnlistenFn>` | 监听单个 target 的探测结果。 |

## 事件

### `net-watcher://network-updated`

设备接口、主接口、公网状态等数据发生语义变化时发送。仅探测时间或耗时变化不会产生该事件。

```ts
interface NetworkUpdatedPayload {
  snapshotId: string
  timestamp: string
  platform: string
  state: SnapshotState
  network: NetworkSnapshot
}
```

### `net-watcher://target-updated`

每个 target 实际完成一次探测后独立发送，不等待其他 target。

```ts
interface TargetUpdatedPayload {
  snapshotId: string
  timestamp: string
  target: ReachabilityTargetSnapshot
}
```

完整快照只通过 `getSnapshot()` 获取，实时事件不会重复携带整个快照。

## 状态判断

业务应按问题读取不同字段，不要使用 target 状态覆盖设备网络状态：

| 业务问题 | 推荐字段 | 主要取值 |
| --- | --- | --- |
| 设备是否存在可用网络接口 | `snapshot.state.overall` | `unknown`、`offline`、`online` |
| 本地网络层是否连接 | `snapshot.state.network` | `unknown`、`disconnected`、`connected` |
| 真实公网是否可用 | `snapshot.state.internet` | `unknown`、`available`、`degraded`、`unavailable`、`captivePortal` |
| 某个服务是否可达 | `target.state.reachability` | `unknown`、`reachable`、`unreachable` |
| 某个服务连接质量 | `target.state.quality` | `unknown`、`stable`、`unstable` |

公网的详细判定证据位于 `snapshot.network.internet`：

- `verified`：是否已有主动探测成功证据。
- `systemHint`：Windows NCSI 或 macOS reachability 提示，只作为辅助证据。
- `activeProbe`：平台公网主动探测结果和耗时。
- `captivePortal`：是否识别到认证门户。
- `consecutiveFailures`：连续公网探测失败次数。
- `reason`：当前公网状态原因。

每个 `reachability.targets[]` 都是独立实例，包含自己的最近探测、失败率、延迟、P95、抖动和连续失败次数。合法的 HTTP `100..=599` 响应均表示目标网络路径可达；例如 `401`、`404`、`500` 和 `503` 不会被当作网络连接失败。

## 监控行为

- Windows 和 macOS 的系统网络变化会立即唤醒 watcher，并重置公网与 target 探测挡位。
- 主接口、地址、网关或 DNS 变化后，旧网络环境中的未完成探测结果会被丢弃。
- 公网正常时最长每 30 秒重新验证一次。
- 公网失败先按 3 秒快速确认，连续第三次失败后收敛为 `unavailable`，随后按 10、20、40、60 秒退避。
- 公网明确不可用时 targets 暂停探测，不把设备断网累计为服务故障；公网恢复后立即恢复 target 探测。
- 健康 target 以 `intervalMs` 为基础周期并加入抖动；每个 target 独立调度，最大并发数为 4。

更完整的数据结构和状态机说明见 [中文设计文档](docs/design.md)。

## 网络请求说明

即使没有配置 `targets`，插件也会发送平台公网验证请求：

| 平台 | 验证地址 | 用途 |
| --- | --- | --- |
| Windows | `http://www.msftconnecttest.com/connecttest.txt` | 验证公网并识别认证门户。 |
| macOS | `http://captive.apple.com/hotspot-detect.html` | 验证公网并识别认证门户。 |

配置 `targets` 后，插件还会按照配置周期访问对应 URL。生产环境应确认这些请求符合应用的隐私声明、企业代理和网络访问策略。

## 错误

JavaScript API 拒绝时返回包含 `code` 和 `message` 的结构化错误：

| 错误码 | 说明 |
| --- | --- |
| `invalid_config` | 配置、URL、间隔或超时不合法。 |
| `already_watching` | watcher 已经启动。 |
| `not_watching` | watcher 尚未启动。 |
| `unsupported_platform` | 当前平台不受支持。 |
| `system_network_unavailable` | 系统网络信息读取失败。 |
| `internal_error` | 插件内部错误。 |

## 示例

完整 Tauri 示例位于 [`examples/tauri-app`](examples/tauri-app)。

```bash
cd examples/tauri-app
pnpm install
pnpm tauri dev
```

如果开发端口 `1420` 已被占用，请先停止已有示例进程，或修改示例的 Vite/Tauri 开发端口配置。

## 开发

```bash
# 构建 JavaScript bindings
pnpm build

# Rust 格式、静态检查与测试
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test

# 构建示例前端和 Tauri Rust 应用
pnpm --dir examples/tauri-app build
cargo check --manifest-path examples/tauri-app/src-tauri/Cargo.toml
```

## 贡献

提交变更前请确保 Rust、JavaScript bindings 和示例应用均构建通过，并为状态机或调度行为变化补充测试与中文设计文档。

## 许可证

当前仓库尚未在 `Cargo.toml`、`package.json` 或根目录许可证文件中声明许可证。正式发布前应补充明确的开源或商业许可证，并保持 Rust crate、npm 包和仓库声明一致。
