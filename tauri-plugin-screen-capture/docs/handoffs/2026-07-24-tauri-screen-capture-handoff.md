# tauri-plugin-screen-capture 交接

## 工作区

- 仓库：`/System/Volumes/Data/workspace/rust/tauri/tauri-plugins/tauri-plugin-screen-capture`
- 分支：`main`
- 当前工作区干净，最新提交尚未推送。

## 已完成并提交

- `0d50185 fix: 修复 macOS 标注合成与背压处理`
- `31529ed feat: 示例应用接入 Rust 原生画板`
- `05235a2 docs: 添加外部画板低延迟合成方案`
- 更早的相关提交：`0af3de3 fix: 修复 macOS 屏幕采集并优化分享结束后的回退流程`

具体实现请直接查看提交 diff，不在此重复。低延迟方案见：

- `docs/media-solutions/2026-07-24-external-annotation-low-latency-design.md`

macOS 实机已验证屏幕采集、WebRTC 发布、Rust/AppKit 原生输入、Metal 合成、工具切换及关闭清理。前端测试、Vite 构建、Rust 测试、`cargo check`、格式检查均通过。链接阶段仍会输出项目原有的 Swift 重复符号警告，但不阻止构建和运行。

## 关键决策变化

用户最后确定的目标不是让插件维护画笔工具或输入行为，而是：

1. 外部画板自行采集输入并维护工具、撤销、图层和场景状态。
2. 插件提供统一的外部标注信息入口。
3. 插件负责校验、保存最新场景、与视频帧自动合成并发布远端。

因此 `31529ed` 中的插件原生 `AnnotationSession + AppKit AnnotationView + pen/eraser` 是已验证的中间实现，但与最终目标的职责划分存在冲突。下一阶段应基于设计文档重构，而不是继续扩充插件内部工具。

当前代码中 `setAnnotationDocument` / `AnnotationLayer` 已接近目标 seam；macOS 新原生画板走的是另一套 `AnnotationSession + Metal` 路径，Windows 则只有外部 `AnnotationDocument` 的 CPU 合成能力，没有原生笔迹采集。下一步需要统一数据入口和跨平台合成语义。

## 建议下一步

1. 先用测试锁定统一入口的 revision、幂等、乱序拒绝和恢复行为。
2. 设计并实现外部增量入口：整场景替换仅用于初始化/恢复，高频路径使用 stroke/element patch。
3. 前端到 Rust 每个 RAF 批量提交真实采样点，优先 raw `Uint8Array`；禁止每点一次 `invoke` 或每次替换整份 JSON 文档。
4. 插件内部采用不可变场景快照和容量为 1 的 latest-wins 合成槽；commit、删除、撤销、清空不可丢。
5. macOS Metal 与 Windows CPU/D3D 合成都读取同一种外部场景。
6. 将 AppKit 原生画板移到示例 App Rust 层作为可选外部画板，或用其他外部画板替换；插件核心不再解释 pointer gesture 和工具状态。
7. 保证共享内容静止时，标注 revision 更新仍能触发最新源帧重放、合成和发布。
8. 按设计文档中的指标做 60/120 Hz、长笔画、DPI、多屏、静态画面和人工 IPC 延迟验收；未证明 IPC 是瓶颈前不要引入共享内存。

## 常用验证

```sh
cd /System/Volumes/Data/workspace/rust/tauri/tauri-plugins/tauri-plugin-screen-capture/examples/tauri-app
/Users/cses-7/.nvm/versions/node/v22.19.0/bin/node --test tests/*.test.mjs
PATH=/Users/cses-7/.nvm/versions/node/v22.19.0/bin:$PATH /Users/cses-7/.nvm/versions/node/v22.19.0/bin/yarn build

cd /System/Volumes/Data/workspace/rust/tauri/tauri-plugins/tauri-plugin-screen-capture
/Users/cses-7/.cargo/bin/cargo test --features macos-screencapturekit
/Users/cses-7/.cargo/bin/cargo fmt --all -- --check
git diff --check
```

示例 App 单独检查：

```sh
/Users/cses-7/.cargo/bin/cargo check \
  --manifest-path examples/tauri-app/src-tauri/Cargo.toml
```

## Suggested skills

- `codebase-design`：重新放置外部画板与插件的 seam，保持插件为深模块。
- `tdd`：先覆盖增量协议、revision、背压和跨平台合成行为，再替换现有链路。
- `diagnosing-bugs`：处理实机输入延迟、Metal/D3D 合成或 IPC 积压问题。
- `research`：只有需要补充平台官方实现细节或成熟方案时再使用；现有调研优先参考上述设计文档。

