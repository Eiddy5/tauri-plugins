# WIN-015 Windows 认证门户识别

**基线状态：应通过**

## 测试用途

验证公网探测被重定向或 HTTP 200 正文被登录页替换时，插件返回 `captivePortal` 而不是 available 或普通 unavailable。

## 测试背景

酒店、机场和访客 Wi-Fi 常提供本地连接但要求网页认证。该状态需要业务单独展示和引导。

## 测试输入条件

1. 可用的真实认证门户网络，或能拦截 Microsoft Connect Test 的测试网关。
2. 网关将探测请求重定向到登录页，或返回 HTTP 200 非预期正文。
3. watcher 在加入该网络前已启动或在加入后启动。

## 测试场景

1. 返回 HTTP 302 到登录页。
2. 返回 HTTP 307/308 到登录页。
3. 返回 HTTP 200 但正文不是 `Microsoft Connect Test`。
4. 完成认证后恢复标准成功响应。

## 执行步骤

1. 加入未认证网络并启动 watcher。
2. 等待公网结果并读取快照。
3. 完成网页认证。
4. 等待下一次验证或触发网络变化。

## 通过要求

1. 未认证时 `state.internet=captivePortal`。
2. `verified=false`、`captivePortal=true`。
3. `activeProbe.status=unexpectedResponse`。
4. 错误码和 reason 为 `captive_portal_detected`。
5. 完成认证后任意一次成功立即恢复为 available，并清零失败数。

## 反例与失败判定

1. 自动跟随重定向并把登录页当作公网成功，判定失败。
2. 认证门户被归为普通 target 故障，判定失败。
3. 认证完成后仍长期保持 captivePortal，判定失败。

## 证据留存

1. 保存认证前后公网 JSON。
2. 保存重定向状态码、Location 或替换正文证据。
3. 记录认证完成到状态恢复的时间。
