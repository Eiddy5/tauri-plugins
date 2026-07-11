# WIN-004 不配置 targets 独立运行

**基线状态：应通过**

## 测试用途

验证 `targets` 为空时插件仍能完整监控设备接口和真实公网，不依赖任何业务服务地址。

## 测试背景

公网验证和自定义服务探测属于两个独立领域。空 targets 是公开支持的核心配置，不应导致 `insufficient_data` 覆盖公网状态。

## 测试输入条件

1. Windows 设备已通过 Wi-Fi 或有线访问公网。
2. `tauri.conf.json` 不声明 `targets`，或显式设置 `targets=[]`。
3. 监听网络事件和 target 事件。

## 测试场景

1. 完全省略 `targets` 字段。
2. 显式配置空数组。
3. 使用 `startWatching({ targets: [] })` 覆盖原配置。

## 执行步骤

1. 启动 watcher。
2. 等待公网验证完成并读取快照。
3. 观察 60 秒事件。

## 通过要求

1. `reachability.targets` 始终为空数组。
2. `state.overall` 和 `state.internet` 正常完成判定。
3. `network.internet.activeProbe` 有 Windows 公网验证结果。
4. 不发送任何 `target-updated`。
5. 不出现 target 相关错误或 panic。

## 反例与失败判定

1. 公网状态因无 target 保持 `unknown`，判定失败。
2. 插件自动生成默认 target，判定失败。
3. 空数组导致配置校验失败，判定失败。

## 证据留存

1. 保存配置、完整快照及 60 秒事件记录。
2. 记录 `target-updated` 事件数量，应为零。
