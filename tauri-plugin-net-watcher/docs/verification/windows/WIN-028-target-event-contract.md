# WIN-028 target-updated 事件契约

**基线状态：应通过**

## 测试用途

严格验证 `net-watcher://target-updated` 每次只携带一个完整 target，并在该 target 完成时立即发送。

## 测试背景

前端按 target ID 增量更新卡片和探测日志。事件携带整个数组会放大流量并造成并发覆盖。

## 测试输入条件

1. 配置三个响应时间分别为 50ms、1s、3s 的 targets。
2. 启动前注册 target 事件监听器。
3. 保持公网 available。

## 测试场景

1. 三个 targets 同批到期但完成时间不同。
2. 单个 target 失败。
3. targets 因公网不可用进入暂停。

## 执行步骤

1. 启动 watcher并保存全部原始 target 事件。
2. 对照服务端完成时间。
3. 检查 payload 字段和 target ID。

## 通过要求

1. payload 只包含 `snapshotId`、`timestamp`、`target`。
2. payload 不包含 `network`、顶层 `state`、`reachability`、`changes` 或 targets 数组。
3. `target` 包含 id、state、target、currentProbe、summary。
4. 每个 target 完成后立即发送，不等待最慢 target。
5. 事件 target 与完整快照中相同 ID 的最新数据一致。

## 反例与失败判定

1. 快 target 等待慢 target 后一起发送，判定失败。
2. 一个事件携带多个 targets，判定失败。
3. payload 中 target ID 与 currentProbe 所属服务不一致，判定失败。

## 证据留存

1. 保存事件原始 JSON。
2. 保存服务端完成时间与事件到达时间。
3. 保存同一时刻完整快照对照。
