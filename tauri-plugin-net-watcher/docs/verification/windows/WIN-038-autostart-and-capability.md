# WIN-038 autoStart 与 Tauri Capability 权限

**基线状态：应通过**

## 测试用途

验证插件自动启动行为和 Tauri 2 权限声明符合公开接入契约。

## 测试背景

生产应用可能依赖 `autoStart`，也可能按最小权限只开放部分命令。权限错误会表现为前端无法调用或 watcher 重复启动。

## 测试输入条件

1. 准备 `autoStart=true` 和 `autoStart=false` 两份配置。
2. 准备包含 `net-watcher:default`、仅 allow-get-snapshot、完全无插件权限三种 capability。
3. 每次场景重新启动应用进程。

## 测试场景

1. autoStart=true 且默认权限完整。
2. autoStart=false 后手动 start。
3. 缺少 start 权限时调用 start。
4. 只有 getSnapshot 权限时读取快照。

## 执行步骤

1. 依次应用每组配置并启动应用。
2. 调用对应 JS API 并记录结果。
3. 检查是否产生网络探测和事件。

## 通过要求

1. autoStart=true 时无需前端 start 即运行 watcher。
2. 此时再次 start 返回 `already_watching`。
3. autoStart=false 时不会在手动 start 前探测。
4. Capability 只允许声明的命令，不允许越权调用。
5. `net-watcher:default` 包含四个公开命令权限。

## 反例与失败判定

1. 无权限仍可调用命令，判定失败。
2. autoStart=false 仍自动发送网络请求，判定失败。
3. autoStart=true 创建两个 watcher，判定失败。

## 证据留存

1. 保存两份 tauri 配置和三份 capability。
2. 保存每个 API 返回或权限错误。
3. 保存启动阶段网络请求和事件记录。
