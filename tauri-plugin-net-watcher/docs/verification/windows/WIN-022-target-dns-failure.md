# WIN-022 target DNS 解析失败

**基线状态：应通过**

## 测试用途

验证自定义 target 域名无法解析时，错误被精确分类并只影响该 target。

## 测试背景

DNS 故障与 TCP、TLS、HTTP 故障的处理和诊断不同，日志必须保留可操作的错误码。

## 测试输入条件

1. 公网验证保持 available。
2. 配置一个正常 target 和一个确定不存在的测试域名。
3. 确保本地 DNS 不会把不存在域名重定向到搜索页。

## 测试场景

1. 域名返回 NXDOMAIN。
2. DNS 服务器超时。
3. 正常域名与失败域名同时探测。

## 执行步骤

1. 启动 watcher。
2. 等待 DNS 失败 target 完成至少一次探测。
3. 检查两个 targets 和顶层状态。

## 通过要求

1. 失败 target `currentProbe.status=failed`。
2. ProbeError code 为 `dns_failed` 或总体超时为 `http_timeout`。
3. 失败 target reachability 为 unreachable、quality 为 unstable。
4. 正常 target 和顶层公网状态不受影响。
5. failureCount 和 lastFailureReason 正确更新。

## 反例与失败判定

1. DNS 失败导致所有 targets unreachable，判定失败。
2. DNS 失败被记录为 HTTP 500，判定失败。
3. 错误没有 message，判定失败。

## 证据留存

1. 保存 `nslookup` 结果。
2. 保存失败 target JSON 和事件。
3. 保存正常 target 对照数据。
