# WIN-024 target TLS 握手失败

**基线状态：应通过**

## 测试用途

验证 HTTPS target 的证书、协议或握手失败被归类为 TLS 失败，而不是 HTTP 可达。

## 测试背景

TLS 是服务连接路径的一部分。只有成功完成 TLS 并收到合法 HTTP 状态，才能判为 target reachable。

## 测试输入条件

1. 公网正常。
2. 准备自签名证书、过期证书或主机名不匹配的 HTTPS 服务。
3. 另配置一个受信任证书的正常 HTTPS target。

## 测试场景

1. 自签名证书未加入系统信任。
2. 证书过期。
3. SNI/证书主机名不匹配。
4. 服务端拒绝协商可用 TLS 版本。

## 执行步骤

1. 分别配置每种异常 HTTPS target。
2. 启动 watcher 并等待探测完成。
3. 检查 phases、error 和 target state。

## 通过要求

1. 异常 target `currentProbe.status=failed`。
2. ProbeError code 为 `tls_failed`，除非总体超时先触发。
3. `phases.tcpMs` 可用，`phases.httpMs` 不应伪造成功值。
4. target 为 unreachable，正常 HTTPS target 不受影响。
5. 失败信息包含底层 TLS 错误描述。

## 反例与失败判定

1. 忽略证书错误并报告 reachable，判定失败。
2. TLS 失败被报告为 HTTP 状态码，判定失败。
3. 一个 target TLS 失败修改公网状态，判定失败。

## 证据留存

1. 保存测试证书信息。
2. 保存 target 探测 JSON。
3. 保存服务端 TLS 日志。
