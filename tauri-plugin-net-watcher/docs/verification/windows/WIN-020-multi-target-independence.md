# WIN-020 多 target 独立状态

**基线状态：应通过**

## 测试用途

验证每个 target 是独立探测实例，一个服务的成功、失败或质量变化不覆盖其他 target，也不修改设备公网结论。

## 测试背景

targets 是用户自定义服务，不允许使用成功数量或并发批次计算一个聚合服务状态。

## 测试输入条件

1. 公网正常且 `state.internet=available`。
2. 配置三个 targets：稳定可达、稳定不可达、可控制间歇失败。
3. 三个 target 使用唯一 ID。

## 测试场景

1. 第一个可达、第二个 TCP 失败、第三个可达。
2. 只恢复第二个 target。
3. 只让第三个 target 产生高延迟。

## 执行步骤

1. 启动 watcher 并等待所有 target 至少完成一次探测。
2. 保存每个 target 的状态和 summary。
3. 依次改变第二和第三个服务。
4. 检查事件和完整快照。

## 通过要求

1. 每个 target 只使用自己的 currentProbe 和 summary 计算状态。
2. 一个 target unreachable 不改变其他 target 的 reachability。
3. target 失败不修改 `state.overall`、`state.network` 或 `state.internet`。
4. targets 在完整快照中保持配置顺序。
5. 每个 target 完成后独立发送自己的事件。

## 反例与失败判定

1. 按“多数成功”把全部 targets 设为 reachable，判定失败。
2. 任一 target 失败导致顶层公网 unavailable，判定失败。
3. 多 target 事件携带错误 target 的 summary，判定失败。

## 证据留存

1. 保存配置和完整 targets JSON。
2. 保存各 target 服务端状态与事件。
3. 记录每个 target 的独立状态变化时间线。
