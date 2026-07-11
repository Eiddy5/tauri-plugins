# WIN-033 IPv6-only 主接口

**基线状态：预期失败/待修复**

## 测试用途

验证只有 IPv6 地址的可用 Windows 网络能够选出主接口并完成设备网络判定。

## 测试背景

当前 `has_available_interface` 接受 IPv6，但主接口筛选要求 IPv4，可能生成 `online` 与 `primaryInterfaceId=null` 的矛盾快照。

## 测试输入条件

1. 可提供原生 IPv6-only 的 Windows 测试网络。
2. 禁用接口 IPv4，只保留全局 IPv6、IPv6 网关和 DNS。
3. 准备 IPv6 可访问的公网或 target。

## 测试场景

1. Wi-Fi IPv6-only。
2. 有线 IPv6-only。
3. IPv6-only 网络从断开恢复。

## 执行步骤

1. 使用 `Get-NetIPConfiguration` 确认没有 IPv4 地址。
2. 启动 watcher并读取快照。
3. 验证 IPv6 target 实际可达。

## 通过要求

1. `state.overall=online`、`state.network=connected`。
2. `primaryInterfaceId` 非空并指向实际 IPv6 接口。
3. 主接口 IPv6、DNS 和网关与系统一致。
4. 网络变化后能重置指纹和探测。

## 反例与失败判定

1. **当前已知反例：**`can_be_primary` 要求非空 IPv4，预期主接口为空。
2. 将 IPv6-only 接口错误标记为 down，判定失败。
3. 因无 IPv4 直接返回 overall offline，判定失败。

## 证据留存

1. 保存 Windows IPv6 配置。
2. 保存完整快照。
3. 保存 IPv6 target 访问证据。
