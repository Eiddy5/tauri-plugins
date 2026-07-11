# WIN-001 Windows Wi-Fi 联网启动

**基线状态：应通过**

## 测试用途

验证应用在 Windows 已连接 Wi-Fi 且公网正常时启动，插件能够生成完整、稳定的首份设备网络和公网快照。

## 测试背景

这是最常见的桌面启动路径，也是其他网络变化用例的健康基线。首份数据错误会导致后续增量事件无法正确合并。

## 测试输入条件

1. Windows 10 或 Windows 11 物理机。
2. 仅启用一张可访问公网的 Wi-Fi 网卡，暂时禁用有线、VPN 和虚拟网卡。
3. `autoStart=false`，`targets=[]`，`intervalMs=10000`，`timeoutMs=3000`。
4. 前端先注册 `onNetworkUpdated`，再调用 `startWatching()` 和 `getSnapshot()`。

## 测试场景

1. 通过普通家庭或办公 Wi-Fi 启动应用。
2. 关闭应用后重新启动，重复执行三次。
3. Wi-Fi 已连接至少 30 秒后再启动应用。

## 执行步骤

1. 确认 Windows 设置页显示 Wi-Fi 已连接且浏览器可访问公网。
2. 启动示例应用并开始 watcher。
3. 等待首个 `network-updated`，随后读取完整快照。
4. 保持网络不变观察 60 秒。

## 通过要求

1. `state.overall=online`，`state.network=connected`。
2. `state.internet=available`，且 `network.internet.verified=true`。
3. `primaryInterfaceId` 指向 `type=wifi`、`status=up` 的接口。
4. 主接口至少包含一个 IPv4 或 IPv6 地址，并返回当前可获得的网关和 DNS。
5. 公网主动探测为 `success`，HTTP 状态为 `200`。
6. 网络保持稳定时，不因时间戳或探测耗时变化重复发送 `network-updated`。

## 反例与失败判定

1. `type=unknown`、主接口为空或仍显示已禁用的旧接口，判定失败。
2. 浏览器可联网但插件持续返回 `unavailable`，判定失败。
3. 60 秒内持续产生无语义变化的网络事件，判定失败。

## 证据留存

1. 保存首份完整 JSON 快照。
2. 保存 60 秒内的网络事件列表和时间戳。
3. 记录 Windows `ipconfig /all` 输出用于接口、网关和 DNS 对照。
