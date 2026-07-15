# macOS 共享窗口聚焦层级快速纠正设计

## 状态

2026-07-15 用户确认采用方案 A：使用公开 CoreGraphics API 轻量采样 Window ID 顺序，在 100 ms 快速路径内发现并纠正聚焦导致的 overlay 失序。

## 问题与根因

窗口共享时，四个 `NSPanel` 与目标窗口使用相同 Window Server layer，并通过 `orderWindow:relativeTo:` 排在目标窗口正上方。用户点击目标窗口后，Window Server 会把目标窗口重新排序到这些 panel 前面。panel 没有隐藏，但目标窗口覆盖了位于其边界内的绿色部分，所以画面只剩窗口边缘外侧的少量绿色尾部。

现有 100 ms 快速路径只定向读取目标窗口的 frame。点击聚焦不改变 frame，因此 `WindowFrameTracker` 返回 `Keep`，不会检查或修复层级。最终只能等待 2 秒低频纠偏读取完整窗口列表并恢复相对顺序，造成可见的约 1 秒至 2.5 秒异常。

## 目标

- 目标窗口点击聚焦导致失序后，在下一个 100 ms 采样周期内开始重排；
- 保持 overlay 紧邻目标窗口之上，同时不越过真正遮挡目标的其他窗口；
- 不使用私有 CGS API；
- 不安装全局鼠标监听、`CGEventTap` 或 Accessibility `AXObserver`；
- 不新增辅助功能、输入监控或其他系统授权；
- 不把每 100 ms 的快速路径升级为完整窗口描述扫描；
- 保留现有移动/缩放时隐藏、稳定后恢复以及 2 秒低频兜底语义。

## 采用方案

### 轻量 Window ID 顺序采样

在 `window_info` 中增加只读取当前可见窗口 ID 顺序的 helper。它调用公开的 `CGWindowListCreate(OptionOnScreenOnly, kCGNullWindowID)`，并通过 CoreFoundation 的数组访问函数把 Window Server 返回的指针编码值转换为 `CGWindowID`。该调用不创建每个窗口的描述字典，也不读取 title、owner、frame 等属性。

helper 返回按 Window Server 前后顺序排列的 `Vec<u32>`。纯逻辑函数只判断以下快速路径不变量：

1. 目标窗口和当前可见的四角 panel 都存在于列表；
2. 每个可见 panel 都位于目标窗口之前；
3. 当前可见 panel 在目标窗口前形成连续组，中间没有其他窗口。

如果当前策略本来就隐藏了部分角标，只校验 `panel_visibility` 标记为可见的 panel。隐藏 panel 不得因为不在窗口列表中触发误修复。

### 快速路径状态流

现有 100 ms `WindowPositionTimer` 继续先读取目标 geometry，并由 `WindowFrameTracker` 处理：

- frame 变化：保持现有行为，立即隐藏并进入 moving；
- frame 首次稳定：保持现有行为，执行完整 `refresh_window_geometry`；
- frame 稳定且 overlay 不可见：执行完整刷新；
- frame 稳定且 overlay 可见：读取轻量 Window ID 顺序。

最后一种情况下，如果快速不变量有效，则直接返回，不读取完整描述。如果失序，则调用现有 `refresh_window_geometry`。该方法会读取完整 `ordered_windows`、重新计算同应用窗口对四角的遮挡、设置 panel level、调用 `orderWindow:relativeTo:`，并安排下一轮 RunLoop 延迟校验。

这样目标聚焦、同应用内部窗口切换或其他未知 Window Server reorder 都走同一条纠正路径，不依赖鼠标事件来源。

## 为什么不采用其他方案

全局 `NSEvent` mouseDown 监听虽然事件驱动，但会观察所有应用点击；它仍需要等待 Window Server 提交排序，并且 macOS 不同版本对跨应用输入监控的授权与投递行为更敏感。通过点击坐标判断目标也无法覆盖程序化聚焦。

Accessibility `AXObserver` 可以直接监听目标窗口事件，但需要辅助功能授权。系统权限必须由用户在系统设置中授予，不能保证是应用内直接点击“确定”即可完成，不符合当前功能的无权限目标。

私有 CGS API 不进入生产方案，避免 App Store、系统升级兼容性和签名审核风险。

## 错误与失败策略

- 轻量 Window ID 列表读取失败时不隐藏 overlay，也不把错误传播给采集会话；本次采样退化为现有完整刷新，让既有错误处理决定是否隐藏。
- 目标或应该可见的 panel 缺失时视为失序，进入完整刷新。
- 完整刷新和延迟校验仍保持 fail-closed：无法安全维持相对层级时隐藏 overlay，不提升为 topmost。
- 暂停、停止、窗口最小化、离开当前 Space 和移动中的行为不变。

## 性能约束

- 100 ms 快速路径只获取 Window ID 数组，不创建全量窗口描述字典；
- 只有 frame 改变、overlay 隐藏或 Z-order 失序时才执行 `ordered_windows` 完整扫描；
- 不缩短 timer 周期，不增加 display link，不持续修改原生窗口；
- 保留 2 秒纠偏 timer 作为遗漏事件和异常状态的兜底。

## 测试设计

### 纯逻辑测试

- 四个 panel 连续位于目标之前时判定有效；
- 目标被点击并排到 panel 之前时判定失序；
- panel 与目标之间夹入其他窗口时判定失序；
- 被策略隐藏的 panel 缺失时不误判；
- 应该可见的 panel 缺失时判定失序；
- frame 稳定但层级失序时选择 `Refresh`，而不是 `Keep`。

### macOS 真实 API 测试

- `CGWindowListCreate` 返回的轻量 ID 列表包含当前 GUI 会话中的有效可见窗口；
- 创建真实目标窗口和四个 panel，建立正确顺序后断言快速校验通过；
- 重新 `orderFront` 目标窗口模拟点击聚焦，推进主 RunLoop 后断言快速校验失败；
- 执行现有完整重排后断言下一轮 RunLoop 恢复正确顺序。

真实窗口测试标记为 macOS ignored，要求已登录 GUI session；纯逻辑测试必须进入常规测试套件。

## 验收标准

- 点击被共享窗口后不再出现约 1 秒以上的绿色角标下沉；
- 自动检测和纠正延迟不超过一个 100 ms 采样周期加一次主 RunLoop 提交；
- 点击其他遮挡窗口时，overlay 不越过该窗口；
- 移动、缩放、最小化、切换 Space 和同应用多窗口遮挡回归通过；
- 完整 feature 测试、格式检查和 Clippy 通过；
- 真机人工重复点击目标窗口至少 20 次，不出现持续失序或权限提示。
