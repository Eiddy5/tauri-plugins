# macOS 屏幕共享浮层验收清单

本文用于验收 macOS 的“显示器 + 单窗口”共享浮层。自动化测试只能覆盖几何、层级决策和捕获过滤策略；跨进程 Window Server 顺序、Space、全屏和 Stage Manager 必须在已登录的 macOS GUI 会话中实测。

## 测试前准备

1. 使用 Apple Silicon Mac，记录 macOS 版本、芯片和显示器布局。
2. 为示例应用授予“屏幕与系统音频录制”权限；不授予辅助功能或输入监控权限。
3. 启动示例应用，准备两个不同进程的普通窗口 A、B；A 作为共享目标，B 用作遮挡窗口。
4. 多显示器测试至少包含主屏左侧或上方的副屏，以覆盖负坐标。
5. 打开 Activity Monitor；性能深测使用 release 构建和 Instruments 的 Time Profiler、Core Animation 模板。

## 功能与层级矩阵

| 场景 | 操作 | 预期结果 |
|---|---|---|
| 显示器共享 | 选择一台显示器开始共享 | 本地四角出现绿色标记；标记不抢焦点、不接收点击，不覆盖整屏透明 surface |
| 单窗口共享 | 选择窗口 A 开始共享 | 四个绿色角与 A 边界对齐，拖动或缩放完成后恢复对齐 |
| 同层窗口遮挡 | 把普通窗口 B 放到 A 上方 | B 同时遮住 A 和绿色浮层；浮层不得悬浮到 B 上方 |
| 部分遮挡 | 让 B 只盖住 A 的一个角 | 对应绿色角被 B 遮挡，其余三个角仍可见 |
| 目标重获焦点 | 点击 A，使 A 回到 B 上方 | A 与浮层一起回到前方，浮层仍只紧邻 A，不越过此时更靠前的窗口 |
| 同应用兄弟窗口 | 用 A 所属应用的另一个窗口覆盖 A | 兄弟窗口自然遮住浮层，行为与跨进程窗口一致 |
| 拖动 | 按住并拖动 A | 拖动期间浮层在一帧内隐藏；鼠标抬起后按最新边界恢复 |
| 最小化 | 最小化 A | 浮层立即隐藏，不在桌面残留四个角 |
| 隐藏应用 | 使用 `⌘H` 隐藏 A 所属应用 | 浮层隐藏；恢复应用后重新对齐并恢复层级 |
| 关闭目标 | 关闭 A | 浮层和事件监听器被清理，采集会话按现有 source-closed 流程结束 |
| Space | 把 A 留在 Space 1，切到 Space 2 | Space 2 不出现浮层；返回 Space 1 后恢复 |
| 原生全屏 | 让 A 进入和退出全屏 | 浮层只在 A 所在空间显示，边界与全屏窗口一致 |
| Stage Manager | 把 A 收入侧边台，再恢复 | 浮层可见性跟随 A，且不盖住无关前台窗口 |
| 睡眠唤醒 | 系统睡眠后唤醒 | 睡眠时隐藏；唤醒后重新读取边界和层级 |
| 暂停/恢复 | 暂停再恢复共享 | 暂停时 panel 与低频纠偏 timer 停止；恢复先刷新边界再显示 |
| 停止共享 | 停止会话 | 四个 panel、鼠标 monitor、notification observer 和 timer 全部释放 |
| 两个并发会话 | 同时共享两个源 | 两组浮层互不覆盖状态；任一显示器采集都不包含另一组浮层 |

## Window Server 层级核验

单窗口模式下，用 `CGWindowListCopyWindowInfo` 按返回顺序检查窗口号。返回列表是从前到后的顺序，必须满足：

1. 四个标题以 `TAURI_SCREEN_CAPTURE_OVERLAY:` 开头的 panel 与目标窗口的 `kCGWindowLayer` 相同。
2. 四个 panel 连续出现在目标 `kCGWindowNumber` 的正前方。
3. 测试窗口 B 原本在目标之前时，B 仍位于四个 panel 之前。
4. 若第 1～3 条无法满足，四个 panel 应全部隐藏；不允许改用 floating、status 或 screen-saver 级别兜底。

建议在调试日志中记录：目标窗口号、目标 layer、四个 panel 窗口号、列表索引、校验结果。日志必须限频，空闲时不得每帧输出。

## 捕获内容核验

| 捕获类型 | 操作 | 预期结果 |
|---|---|---|
| 显示器 | 录制 10 秒并检查每秒至少一帧 | 本地可见绿色角，编码/发布画面中完全没有绿色角 |
| 显示器 | 在插件自身普通窗口内播放动画 | 普通 Tauri 窗口仍出现在共享画面，只有保留标题前缀的 panel 被排除 |
| 单窗口 | 共享 A 并移动、遮挡 A | 输出只包含 A 的 ScreenCaptureKit 单窗口内容，不包含浮层或遮挡窗口 B |
| 并发 | 已共享显示器后启动第二个浮层会话 | 第一个显示器流不出现第二组浮层 |

## 性能验收

使用 release 构建，保持目标窗口静止 60 秒：

- Activity Monitor 中浮层带来的空闲 CPU 目标低于 `0.5%`。
- Instruments 不应观察到 `CADisplayLink` 或持续重绘循环。
- Core Animation 不应存在覆盖整个显示器的透明 surface 或持续 offscreen rendering pass。
- 2 秒纠偏快照未发现 frame、layer 或相对顺序变化时，不调用 `setFrame` 或 `orderWindow_relativeTo`。
- 从 `LeftMouseDragged` 到四个 panel 隐藏的 P95 小于 `16 ms`。
- 从 `LeftMouseUp` 完成边界读取到四个 panel 恢复可见的 P95 小于 `16 ms`。

## 自动化回归

```bash
cargo fmt --check
cargo test --all-targets
cargo test --all-targets --features macos-screencapturekit
cargo clippy --all-targets --features macos-screencapturekit -- -D warnings
cargo check --manifest-path examples/tauri-app/src-tauri/Cargo.toml
```

## 验收记录模板

```text
日期：
macOS：
芯片：
显示器布局：
单窗口层级：通过 / 未通过
Space/全屏/Stage Manager：通过 / 未通过
显示器捕获排除：通过 / 未通过
单窗口捕获排除：通过 / 未通过
并发会话：通过 / 未通过
空闲 CPU：
拖动隐藏 P95：
释放恢复 P95：
备注：
```
