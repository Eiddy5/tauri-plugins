# WIN-030 运行时配置覆盖与校验

**基线状态：应通过**

## 测试用途

验证 `startWatching(options)` 只覆盖当前会话允许的字段，严格拒绝无效 target、重复 ID 和越界时间配置。

## 测试背景

运行时配置不能修改基础 `tauri.conf.json`，也不能绕过初始化时的生产范围校验。

## 测试输入条件

1. `tauri.conf.json` 配置一个基础 target、10 秒间隔和 3 秒超时。
2. watcher 初始停止。
3. 准备合法和非法运行时 options。

## 测试场景

1. 合法覆盖 targets、intervalMs、timeoutMs。
2. 重复 target ID。
3. 空 ID、非法 URL、非 HTTP/HTTPS scheme。
4. intervalMs 小于 1000 或大于 3600000。
5. timeoutMs 小于 100、大于 60000 或大于 intervalMs。

## 执行步骤

1. 使用合法 options 启动并读取快照。
2. 停止 watcher，依次使用非法 options 启动。
3. 调用 `getConfig()` 检查基础配置。

## 通过要求

1. 合法覆盖只对当前会话快照和探测生效。
2. `getConfig()` 仍返回基础配置，不被运行时 options 修改。
3. 非法配置返回结构化 `invalid_config`。
4. 校验失败后 watcher 不启动，也不保留半初始化任务。
5. 停止后无 options 再启动时恢复基础配置。

## 反例与失败判定

1. 运行时覆盖永久修改基础配置，判定失败。
2. 重复 ID 被接受并造成事件路由冲突，判定失败。
3. 校验失败后再次启动返回 already_watching，判定失败。

## 证据留存

1. 保存基础配置、运行时 options 和快照。
2. 保存每个非法场景的错误 JSON。
3. 保存重启后的配置对照。
