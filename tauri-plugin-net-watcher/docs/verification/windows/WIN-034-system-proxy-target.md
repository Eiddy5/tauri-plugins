# WIN-034 系统代理/PAC 下的 target 可达性

**基线状态：预期失败/产品决策待确认**

## 测试用途

验证企业系统代理或 PAC 环境中，插件 target 探测与桌面应用实际访问路径保持一致。

## 测试背景

当前 target 使用原始 DNS、TCP 和 TLS 直连，不读取 Windows 系统代理。企业网络可能禁止直连但允许经代理访问。

## 测试输入条件

1. Windows 配置有效的系统 HTTP/HTTPS 代理或 PAC。
2. 防火墙禁止目标服务直连，只允许通过代理。
3. WebView 或浏览器可通过系统代理访问 target。
4. 公网验证记录其系统 HTTP 栈结果。

## 测试场景

1. 固定 HTTP 代理。
2. 固定 HTTPS CONNECT 代理。
3. PAC 按域名选择代理。
4. 代理认证成功和认证失败。

## 执行步骤

1. 验证应用或浏览器通过代理可访问 target。
2. 启动 watcher并观察 target。
3. 关闭直连限制后再次测试。

## 通过要求

1. 产品若定义“应用可达性”，target 应遵循系统代理/PAC 并返回 reachable。
2. 代理连接和目标连接仍受总体 timeout 控制。
3. 代理失败应提供可诊断错误，不应误报公网完全断开。
4. 文档必须明确选择直连语义还是应用代理语义。

## 反例与失败判定

1. **当前已知反例：**原始 `TcpStream` 绕过代理，浏览器可访问但 target 返回 unreachable。
2. 公网 available 与所有代理内 targets unreachable 且无说明，判定失败。
3. PAC 执行阻塞 watcher，判定失败。

## 证据留存

1. 保存系统代理/PAC 配置。
2. 保存代理访问日志和 target JSON。
3. 保存开启/关闭直连限制的对照结果。
