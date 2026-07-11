# WIN-025 target 延迟、抖动和失败率统计

**基线状态：应通过**

## 测试用途

验证每个 target 的滚动窗口、延迟摘要、P95、抖动、失败率和连续失败数计算正确。

## 测试背景

质量数据用于简单探测日志和状态判定，统计错误会让 stable/unstable 结论失真。

## 测试输入条件

1. 准备可编排延迟和失败序列的 HTTP 测试服务。
2. 窗口大小使用插件内部默认值 20。
3. 公网保持 available。

## 测试场景

1. 连续 20 次约 100ms 成功。
2. 交替 100ms 和 500ms 成功，制造抖动。
3. 20 个样本中插入可预测数量的失败。
4. 连续失败后成功恢复。

## 执行步骤

1. 按场景控制服务端响应序列。
2. 收集至少一个完整窗口的 target 事件。
3. 独立计算预期统计并与 summary 对照。

## 通过要求

1. `sampleCount` 不超过 20，超出后淘汰最旧样本。
2. successCount、failureCount 和 failureRate 数学正确。
3. avg、min、max、p95 只使用成功样本。
4. jitter 使用相邻成功延迟差并符合实现定义。
5. consecutiveFailures、lastSuccessAt、lastFailureAt、lastFailureReason 正确。
6. 超过阈值时仅该 target quality 变为 unstable。

## 反例与失败判定

1. 失败样本的超时值参与成功延迟均值，判定失败。
2. 一个 target 的样本进入另一个 target 窗口，判定失败。
3. 窗口无限增长，判定失败。

## 证据留存

1. 保存服务端响应序列。
2. 保存 20 次 target 事件和最终 summary。
3. 保存独立统计计算表。
