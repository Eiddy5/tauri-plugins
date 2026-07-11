# WIN-002 Windows 有线联网启动

**基线状态：应通过**

## 测试用途

验证应用在 Windows 已连接有线网络且公网正常时启动，插件能正确选择有线接口并验证公网。

## 测试背景

有线和 Wi-Fi 使用不同的系统接口类型与连接状态来源，需要避免沿用上一次 Wi-Fi 快照或名称启发式结果。

## 测试输入条件

1. Windows 10 或 Windows 11 物理机。
2. 插入可访问公网的网线并禁用 Wi-Fi、VPN 和虚拟网卡。
3. 使用默认插件配置且不配置 targets。
4. 前端已监听 `network-updated`。

## 测试场景

1. 应用启动前网线已经连接。
2. 应用关闭重启三次。
3. DHCP 地址租约稳定后启动。

## 执行步骤

1. 使用浏览器确认有线网络可以访问公网。
2. 启动应用并调用 `startWatching()`。
3. 等待首个网络事件并调用 `getSnapshot()`。
4. 保持网络稳定观察 60 秒。

## 通过要求

1. `state.overall=online`、`state.network=connected`、`state.internet=available`。
2. 主接口 `type=ethernet` 且 `status=up`。
3. IPv4、网关、DNS 与 `ipconfig /all` 中对应适配器一致。
4. `network.internet.verified=true` 且主动探测成功。
5. 快照中不存在仍被标为主接口的旧 Wi-Fi。

## 反例与失败判定

1. 主接口仍为 Wi-Fi、`type=unknown` 或 `primaryInterfaceId=null`，判定失败。
2. 有线可联网但公网长期为 `unknown` 或 `unavailable`，判定失败。
3. 稳定网络产生事件风暴，判定失败。

## 证据留存

1. 保存完整快照和首个网络事件。
2. 保存 `ipconfig /all` 输出。
3. 记录使用的网卡型号和 Windows 版本。
