# WIN-012 network-updated 事件契约与去噪

**基线状态：应通过**

## 测试用途

严格验证 `net-watcher://network-updated` 的 payload、发送条件和去噪行为。

## 测试背景

前端按区域增量合并事件。字段膨胀或无语义事件会破坏兼容性并增加日志量。

## 测试输入条件

1. watcher 初始未启动。
2. 前端在启动前注册网络事件监听器。
3. 准备正常联网、断网和恢复三个状态。

## 测试场景

1. watcher 首次启动产生网络状态。
2. 网络从在线切换为离线。
3. 网络从离线恢复在线。
4. 网络保持不变，仅公网探测耗时变化。

## 执行步骤

1. 记录每个网络事件原始 JSON。
2. 依次执行四种场景。
3. 与同时间点 `getSnapshot()` 返回值对照。

## 通过要求

1. payload 只包含 `snapshotId`、`timestamp`、`platform`、`state`、`network`。
2. payload 不包含 `meta`、`reachability` 和 `changes`。
3. `state` 与 `network.internet.status` 在同一事件中一致。
4. 设备或公网语义变化时发送事件。
5. 仅 `checkedAt`、probe ID 或 `durationMs` 变化时不发送事件。
6. 对应事件数据与最新完整快照一致。

## 反例与失败判定

1. 每次 30 秒公网验证都发送内容相同的网络事件，判定失败。
2. 事件携带整个 targets 数组，判定失败。
3. 事件中的公网状态与完整快照冲突，判定失败。

## 证据留存

1. 保存原始事件 JSON 和完整快照。
2. 记录各场景事件数量。
3. 保存事件字段集合比较结果。
