# Net Watcher 插件设计

日期：2026-07-04

## 目标

`tauri-plugin-net-watcher` 是一个 Tauri v2 桌面插件，用于在 Windows 和 macOS 上监控设备网络状态与服务可达性。

插件回答三个相互独立的问题：

- 当前设备是否有可用网络接口。
- 当前设备是否已经主动验证为可访问公网。
- 配置的一个或多个 HTTP/HTTPS 服务目标是否可达，近期网络质量是否稳定。

Linux、Android、iOS 等平台第一版不实现真实监控；除 `get_config` 外，其它命令返回 `unsupported_platform`。

## 配置

配置从 `tauri.conf.json` 的 `plugins.net-watcher` 读取，只暴露核心字段：

```json
{
  "plugins": {
    "net-watcher": {
      "autoStart": true,
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

字段含义：

- `autoStart`：插件初始化后是否自动开始监控。
- `targets`：可选的服务可达性目标数组；配置目标时每个 `id` 必须唯一。
- `intervalMs`：target 健康状态下的基础探测间隔，允许范围为 `1000` 到 `3600000`；不是所有状态下的固定周期。
- `timeoutMs`：单次探测超时时间，允许范围为 `100` 到 `60000`，且不能大于 `intervalMs`。

未配置 `targets` 或显式配置空数组时，插件不发起自定义业务服务探测，但 Windows 和 macOS 的平台公网验证仍然运行。公网验证不是 target，不参与 target 的滚动统计。旧的单目标 `target` 字段不再属于公开契约。

内部质量窗口和阈值由 Rust 私有默认值控制，不作为公开配置契约暴露。

## 数据结构

快照统一使用 `NetWatcherSnapshot`：

- `meta`：快照 ID、时间、平台、插件版本。
- `state`：业务快速判断字段。`overall` 和 `network` 描述本地接口，`internet` 描述公网可用性，均不受自定义 target 状态影响。
- `network`：网络接口列表、主接口、地址、网关、DNS 和公网判断证据。Windows 通过系统适配器 API 采集 DNS 和网关；macOS 按系统 API 可得性返回。
- `reachability`：可选的服务探测数据，包含公共探测配置和独立目标实例列表。每个目标包含自己的可达性、质量、原因、最近探测和滚动统计。
- `changes`：相对上一份快照的变化字段。

完整快照只通过 `getSnapshot()` 主动查询。实时推送使用两个精简事件：

```text
net-watcher://network-updated
net-watcher://target-updated
```

`network-updated` 只包含 `snapshotId`、`timestamp`、`platform`、`state` 和 `network`，对应示例顶部设备与公网区域。`target-updated` 只包含 `snapshotId`、`timestamp` 和一个完整的 `ReachabilityTargetSnapshot`；多个 target 分别发送，事件中不携带整个 targets 数组。两类事件都不携带 `reachability.config` 和 `changes`。

后台任务仍维护完整 `NetWatcherSnapshot`。设备/公网只在语义变化时发送 `network-updated`，避免仅因时间戳或公网探测耗时变化产生噪音；每次实际完成 target 探测都会发送该 target 的 `target-updated`，作为独立探测日志。前端启动时先注册两个监听器，再调用 `startWatching()` 和 `getSnapshot()`，之后按事件区域增量合并。

## 监控逻辑

后台 watcher 采用“系统网络事件唤醒 + 独立自适应调度”的混合模型：

1. watcher 启动时创建后台任务，并尝试订阅系统网络变化事件。
2. Windows 当前通过 `NotifyIpInterfaceChange` 监听网卡、IP 地址和接口状态变化；收到系统回调后立即唤醒一次刷新。
3. macOS 当前通过 `SCDynamicStore` 监听网络配置变化；收到系统回调后立即唤醒一次刷新。
4. 每次唤醒先读取系统网络接口快照，并用主接口、接口状态、地址、网关和 DNS 生成网络指纹。指纹变化时立即重置公网挡位、各 target 挡位和 target 质量窗口；正在执行的旧网络探测会被中断，结果不写入快照。
5. 本地接口不可用时，公网直接判定为 `unavailable`，无需发起外部请求；所有 targets 进入 `suspended`，只推送一次 `probe_suspended_network_unavailable`，不累计服务失败样本。
6. Windows 读取 NCSI `NetworkConnectivityLevel` 作为系统提示，再使用 Windows HTTP 系统栈访问 `http://www.msftconnecttest.com/connecttest.txt`。只有 HTTP 200 且响应正文严格等于 `Microsoft Connect Test` 才算主动验证成功；禁止自动重定向以识别认证门户。
7. macOS 使用 `SCNetworkReachability` 查询 `captive.apple.com` 的系统 reachability 提示，再通过 `NSURLSession` 访问 `http://captive.apple.com/hotspot-detect.html`。只有最终 URL 未被改写、HTTP 200 且正文包含 Apple 成功标记才算主动验证成功；重定向或成功正文被替换时判为 `captivePortal`。请求使用 ephemeral session，禁用缓存和 cookie，并允许系统代理/PAC 生效。
8. 公网探测超时固定为 5 秒。正常状态最长每 30 秒验证一次；前两次失败按 3 秒快速确认，连续第三次失败收敛为 `unavailable`，之后按 10、20、40、60 秒有界退避。系统网络变化会立即重置挡位并重新验证，任意一次成功立即恢复。主动探测禁用 HTTP 缓存。
9. 若配置了 `targets`，每个 target 持有自己的挡位和下一次探测时间。健康状态以 `intervalMs` 为基础并加入 ±10% 抖动；失败前两次按 3 秒确认，第三次开始按 10、20、40、60 秒退避；高延迟或质量不稳定按不高于 5 秒复查；失败后的首次成功按 3 秒恢复确认。
10. 到期 targets 最多并发执行 4 个，但每个完成后立即更新自己的滚动窗口、挡位、完整快照并发送一个 `target-updated`，不等待同批较慢目标。网络事件到达时，尚未完成的 target 探测被废弃。
11. 公网优先生成快照并发送 `network-updated`，不会等待 targets。后台始终保存完整快照供 `getSnapshot()` 查询。

系统网络事件只负责提前触发刷新，不直接等同于前端通知。设备/公网是否发送事件取决于是否存在语义变化；target 每次实际探测完成都会发送自己的事件。HTTP 目标可达性、延迟和失败率不会总是触发系统网络事件，因此 target 自适应调度必须保留。

HTTP 探测只判断网络路径和 HTTP 服务是否可达，不判断业务响应是否健康。收到合法的 `100` 到 `599` HTTP 状态码都视为探测成功，原始状态码保存在 `currentProbe.http.statusCode`。DNS、TCP、TLS、超时、读写或 HTTP 协议解析失败才计入失败率并影响目标可达状态。因此 `401`、`403`、`404`、`500`、`503` 不会被误判为网络不可达。

多目标探测采用固定上限为 4 的有界并发，不增加公开配置。每个 target 完成后立即独立更新和推送，不等待较慢目标；完整快照中的 targets 始终保持配置顺序。

## 状态规则

顶层 `state` 只描述设备网络接口：

- 系统网络快照读取失败：`overall = unknown`、`network = unknown`。
- 没有可用网络接口：`overall = offline`、`network = disconnected`。
- 存在可用网络接口：`overall = online`、`network = connected`。

这里的 `online` 表示设备存在可用网络接口，不承诺公网或任意业务服务一定可达。业务判断公网使用 `state.internet`：

- `available`：主动探测已验证成功，`network.internet.verified = true`。
- `degraded`：平台系统提示仍认为目标可达，或从已可用状态开始出现少量主动探测失败，但当前没有新的成功证据。
- `unavailable`：无可用接口，或主动探测失败且系统提示不具备公网访问。
- `captivePortal`：收到重定向或 HTTP 200 非预期正文，通常需要网页认证。
- `unknown`：尚未完成验证、系统快照失败，或当前平台未实现公网验证。

详细依据位于 `network.internet`。其中 `systemHint` 仅为提示，`activeProbe` 才是验证证据；自定义 targets 不参与公网状态计算。

macOS 与 Windows 共用公网状态机：主动探测成功为 `available`，认证页为 `captivePortal`，失败先进入快速复核，连续第三次失败收敛为 `unavailable`。macOS 的系统网络事件、网络指纹重置、旧探测废弃、失败退避和 target 自适应挡位也与 Windows 共用同一套逻辑。

每个 `reachability.targets[]` 都是独立监控实例：

- `state.reachability`：`unknown`、`reachable`、`unreachable`。
- `state.quality`：`unknown`、`stable`、`unstable`。
- `state.reason`：当前目标状态的主要原因。
- `summary`：该目标自己的失败率、延迟、抖动和连续失败次数。

一个 target 的成功或失败不会修改顶层设备网络状态，也不会覆盖其他 target 的状态。

公网状态为 `unavailable` 时，target 的 `state.reachability` 和 `state.quality` 暂时回到 `unknown`，原因为 `probe_suspended_network_unavailable`。这是设备侧无法执行有效服务验证，不表示目标服务自身宕机。公网恢复后 target 立即退出暂停并执行一次全新探测。

生产兜底策略：

- 系统网络事件会做短窗口合并，避免网卡切换、VPN 连接等场景触发事件风暴。
- 如果系统事件监听初始化失败，watcher 降级为仅定时探测，不阻断插件启动。
- 配置使用严格 URL 解析和范围校验，避免生产误配造成过密探测或无效目标。
- MAC 地址默认不采集，避免引入额外隐私风险。
- 业务侧使用 `state.overall` 判断设备是否有可用网络接口，使用 `state.internet` 判断公网是否可用，使用 `reachability.targets[]` 判断各自定义服务是否可达。
