# WIN-027 公网不可用时 target 暂停与恢复

**基线状态：应通过**

## 测试用途

验证公网明确不可用时 targets 不执行无意义探测、不累计服务失败，公网恢复后立即重新开始。

## 测试背景

设备断网不能等价于所有业务服务宕机，否则监控日志会制造大量错误告警。

## 测试输入条件

1. 配置至少两个初始可达 targets。
2. watcher 已运行且公网 available。
3. 可完全断开网络并随后恢复。

## 测试场景

1. 拔出唯一网线。
2. 关闭唯一 Wi-Fi。
3. 保留本地接口但让公网最终 unavailable。
4. 恢复原网络或切换到新网络。

## 执行步骤

1. 保存健康 targets summary。
2. 使公网 unavailable。
3. 保持不可用超过两个基础周期。
4. 恢复公网并观察首次 target 探测。

## 通过要求

1. targets 状态变为 reachability/quality unknown。
2. reason 为 `probe_suspended_network_unavailable`。
3. 暂停期间 sampleCount 和 failureCount 不增加。
4. 每个 target 只在进入暂停时推送一次暂停事件。
5. 公网恢复后 targets 立即退出暂停并探测。
6. 新网络恢复时旧质量窗口被重置。

## 反例与失败判定

1. 断网期间所有 targets 持续累计失败，判定失败。
2. 公网恢复后 targets 仍暂停，判定失败。
3. 暂停原因被写成服务端错误，判定失败。

## 证据留存

1. 保存暂停前、暂停中、恢复后三份快照。
2. 保存 target 事件数量。
3. 保存 summary 计数变化。
