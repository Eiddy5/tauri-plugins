# WIN-003 Windows 完全断网启动

**基线状态：应通过**

## 测试用途

验证应用在没有任何可用物理网络接口时启动，可以立即报告离线，并暂停所有 target 探测。

## 测试背景

完全断网不应等待公网超时，也不应把设备侧断网记录为业务服务故障。

## 测试输入条件

1. 禁用 Wi-Fi，拔出网线并断开 VPN。
2. 配置至少两个 targets，其中一个正常服务、一个不可达服务。
3. 清空应用上一次运行产生的页面事件记录。
4. `autoStart=false`。

## 测试场景

1. 应用在完全断网状态启动。
2. Windows 飞行模式下启动。
3. 所有物理网卡在设备管理器中禁用后启动。

## 执行步骤

1. 确认 Windows 显示无网络连接。
2. 启动应用并开始 watcher。
3. 读取首份完整快照。
4. 观察 targets 和事件 30 秒。

## 通过要求

1. `state.overall=offline`、`state.network=disconnected`、`state.internet=unavailable`。
2. `network.internet.reason=no_available_interface`。
3. 不发起 Microsoft Connect Test 公网请求。
4. 每个 target 状态为 `unknown`，原因为 `probe_suspended_network_unavailable`。
5. target 的 `summary.failureCount` 不增加。
6. 每个刚进入暂停的 target 最多发送一次暂停事件。

## 反例与失败判定

1. 断网时仍返回 `overall=online` 或 `internet=available`，判定失败。
2. targets 被记录为 `unreachable` 并累计失败率，判定失败。
3. 离线状态循环发送相同暂停事件，判定失败。

## 证据留存

1. 保存首份快照和 30 秒事件记录。
2. 保存 Windows 网络设置页截图。
3. 如可用，保存抓包结果证明未发送外部探测。
