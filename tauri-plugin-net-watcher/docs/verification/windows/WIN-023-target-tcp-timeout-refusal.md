# WIN-023 target TCP 超时与拒绝

**基线状态：应通过**

## 测试用途

验证 target 主机端口拒绝连接或无响应时，能够分类为连接失败并按独立调度重试。

## 测试背景

TCP 拒绝通常快速返回，黑洞超时则受 `timeoutMs` 控制，两者都不应阻塞其他 targets。

## 测试输入条件

1. 公网正常。
2. 配置一个正常 target。
3. 配置一个本机关闭端口和一个被防火墙丢弃报文的地址。
4. `timeoutMs=3000`。

## 测试场景

1. TCP Connection Refused。
2. TCP SYN 黑洞直到超时。
3. 慢失败与正常 target 同时执行。

## 执行步骤

1. 启动 watcher并记录探测开始时间。
2. 等待三个 target 分别完成。
3. 检查事件到达顺序和错误分类。

## 通过要求

1. 拒绝连接返回 `tcp_failed`。
2. 黑洞不超过配置总超时，通常返回 `http_timeout`。
3. 两个失败 target 独立标记 unreachable。
4. 正常 target 不等待慢失败 target 才发送事件。
5. 顶层设备和公网状态不改变。

## 反例与失败判定

1. 慢 target 阻塞所有 target 事件，判定失败。
2. 单次探测明显超过 `timeoutMs` 且无调度误差解释，判定失败。
3. TCP 错误被记录为合法 HTTP 响应，判定失败。

## 证据留存

1. 保存 Windows 防火墙规则。
2. 保存每个探测 duration 和 phases。
3. 保存事件到达时间线。
