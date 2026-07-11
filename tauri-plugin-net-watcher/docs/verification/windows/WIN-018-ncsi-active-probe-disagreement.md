# WIN-018 NCSI 与主动探测结果不一致

**基线状态：应通过**

## 测试用途

验证 NCSI 仅作为系统提示，主动探测证据能够纠正过期或错误的系统公网提示。

## 测试背景

Windows NCSI 状态可能存在缓存或与应用实际请求路径不同。插件不能把 NCSI 当成唯一真源。

## 测试输入条件

1. 可分别控制 NCSI 系统提示和 Microsoft Connect Test 请求结果的测试网络或防火墙。
2. 本地接口始终可用。
3. 记录 `systemHint` 与 `activeProbe`。

## 测试场景

1. NCSI=`internetAccess`，主动探测失败。
2. NCSI=`none/localAccess`，主动探测成功。
3. NCSI 状态暂时保持旧值，主动探测结果已经变化。

## 执行步骤

1. 构造每个不一致场景。
2. 强制网络变化或等待公网探测。
3. 保存每次完整公网证据。

## 通过要求

1. 主动探测成功时最终为 `available/verified=true`，不能被较低 NCSI 提示覆盖。
2. NCSI 提示互联网但主动探测失败时不得设置 `verified=true`。
3. 连续失败最终能够覆盖陈旧 NCSI 并收敛为 unavailable。
4. `systemHint` 和 `activeProbe` 都原样保留，便于诊断。

## 反例与失败判定

1. 只要 NCSI 为 internetAccess 就永久返回 available，判定失败。
2. 主动探测成功仍因 NCSI none 返回 unavailable，判定失败。
3. 不一致时丢失任一证据字段，判定失败。

## 证据留存

1. 保存三种场景的公网 JSON。
2. 保存 Windows NCSI 状态获取结果。
3. 保存主动探测请求结果。
