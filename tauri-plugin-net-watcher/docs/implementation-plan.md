# Net Watcher 实现计划

日期：2026-07-02

## 范围

实现一个只支持 Windows 和 macOS 的 Tauri v2 网络监控插件。第一版包含：

- 插件配置读取与校验。
- 结构化快照模型。
- 网络接口快照读取。
- HTTP/HTTPS 主动质量探测。
- 滚动窗口统计。
- 状态机与评分。
- 后台 watcher 生命周期。
- Tauri 命令、权限、TypeScript API。
- 中文 README 与示例应用。

## 模块划分

- `src/config.rs`：公开配置和运行时覆盖。
- `src/models.rs`：快照、网络、质量、事件 payload 模型。
- `src/network.rs`：系统网络接口读取。
- `src/probe.rs`：HTTP/HTTPS 探测。
- `src/stats.rs`：滚动窗口统计。
- `src/state.rs`：状态机与评分。
- `src/desktop.rs`：Windows/macOS watcher 生命周期和事件推送。
- `src/mobile.rs`：非 Windows/macOS 平台 unsupported 实现。
- `src/commands.rs`：Tauri 命令入口。
- `guest-js/index.ts`：前端类型和 API。

## 核心约束

- `tauri.conf.json` 中只接受 `autoStart`、`target`、`intervalMs`、`timeoutMs`。
- 内部阈值不通过配置文件或 `get_config` 暴露。
- `start_watching` 只能覆盖本次会话的 `target`、`intervalMs`、`timeoutMs`。
- 重复启动返回 `already_watching`。
- 未启动时停止返回 `not_watching`。
- 停止 watcher 时发送停止信号并等待后台任务退出。
- 非 Windows/macOS 平台除 `get_config` 外返回 `unsupported_platform`。

## 验证

实现完成后需要运行：

```powershell
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
cargo fmt --check
corepack pnpm build
```

示例应用需要运行：

```powershell
corepack pnpm build
corepack pnpm tauri build --debug
```

示例应用的 Tauri bundle identifier 不能保持默认 `com.tauri.dev`，否则 Tauri 会拒绝构建。

