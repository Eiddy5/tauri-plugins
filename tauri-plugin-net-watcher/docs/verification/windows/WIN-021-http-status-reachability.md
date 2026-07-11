# WIN-021 HTTP 状态码与网络可达性

**基线状态：应通过**

## 测试用途

验证 target 收到任何合法 HTTP `100..=599` 状态码时都视为网络路径可达，同时保留原始状态码。

## 测试背景

插件只判断连接和 HTTP 服务可达，不判断业务健康。鉴权失败或服务端错误不能被误判成网络断开。

## 测试输入条件

1. 准备可按路径返回指定状态码的 HTTP 测试服务。
2. 公网或测试局域网路径稳定。
3. 将各路径分别配置为独立 target。

## 测试场景

1. HTTP 200、204、301。
2. HTTP 401、403、404。
3. HTTP 500、503。
4. HTTP 100 到 599 边界值。

## 执行步骤

1. 启动 watcher 并等待每个 target 完成探测。
2. 检查 currentProbe 和 target state。
3. 切换服务端状态码并重复。

## 通过要求

1. 所有合法状态码对应 `currentProbe.status=success`。
2. `state.reachability=reachable`。
3. `currentProbe.http.statusCode` 等于服务端实际状态码。
4. 不生成网络错误 ProbeError。
5. 401、403、404、500、503 不增加 failureCount。

## 反例与失败判定

1. HTTP 500 被记为 target unreachable，判定失败。
2. HTTP 401 被记为鉴权网络异常，判定失败。
3. 原始状态码丢失或被统一改为 200，判定失败。

## 证据留存

1. 保存服务端访问日志和返回状态。
2. 保存各 target currentProbe JSON。
3. 保存 summary 计数。
