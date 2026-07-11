# WIN-013 Windows 公网主动验证成功

**基线状态：应通过**

## 测试用途

验证 Windows 公网状态只有在 Microsoft Connect Test 返回预期状态和正文后才被标记为已验证可用。

## 测试背景

NCSI 只是系统提示，不能单独作为插件的最终公网结论。业务读取 `state.internet` 时需要明确的主动证据。

## 测试输入条件

1. Windows 网络接口已连接并可访问 `http://www.msftconnecttest.com/connecttest.txt`。
2. 不使用会改写该请求的认证门户或代理。
3. watcher 初始未启动。

## 测试场景

1. Wi-Fi 正常访问公网。
2. 有线网络正常访问公网。
3. NCSI 为 `internetAccess` 且主动探测成功。

## 执行步骤

1. 使用浏览器或抓包工具确认地址返回 `Microsoft Connect Test`。
2. 启动 watcher 并等待公网验证。
3. 读取网络事件和完整快照。

## 通过要求

1. `state.internet=available`。
2. `network.internet.status=available` 且 `verified=true`。
3. `systemHint.source=windowsNcsi`。
4. `activeProbe.status=success`、`httpStatus=200`。
5. `reason=internet_probe_succeeded`。
6. `checkedAt` 非空，`consecutiveFailures=0`。

## 反例与失败判定

1. 仅凭 NCSI 就设置 `verified=true`，判定失败。
2. 主动响应正文不匹配仍返回 available，判定失败。
3. 成功后连续失败数未清零，判定失败。

## 证据留存

1. 保存公网详情 JSON。
2. 保存 NCSI 系统状态和 HTTP 响应证据。
3. 记录主动探测耗时。
