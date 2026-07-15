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

helper 返回按 Window Server 前后顺序排列的 `Vec<u32>`。每次完整层级校验通过后，host 从已验证的完整窗口栈缓存一个顺序片段：

1. 有可见 panel 时，片段从最靠前的可见 panel 开始，到目标窗口结束；
2. WindowServer 插入 panel 与目标之间、且已被完整策略验证安全的同 owner sibling 会保留在片段中；
3. 四角全部因 sibling 遮挡而隐藏时，片段从最靠前的同 owner、同 layer sibling 开始，到目标窗口结束；
4. 没有可用锚点时，片段至少包含目标窗口本身。

100 ms 快速校验只比较当前 Window ID 列表中以目标结尾的同长度片段是否与缓存完全一致。这样合法 ToDesk sibling 稳态不会反复触发完整扫描；目标窗口前置、可见 panel 缺失、未知窗口插入或 sibling 顺序变化仍会触发一次完整刷新。完整策略接受新状态后更新缓存并重新收敛。

### 快速路径状态流

现有 100 ms `WindowPositionTimer` 继续先读取目标 geometry，并由 `WindowFrameTracker` 处理：

- frame 变化：保持现有行为，立即隐藏并进入 moving；
- frame 首次稳定：保持现有行为，执行完整 `refresh_window_geometry`；
- frame 稳定且 overlay 不可见：执行完整刷新；
- frame 稳定且 overlay 可见：读取轻量 Window ID 顺序。

最后一种情况下，如果当前顺序片段与缓存一致，则直接返回，不读取完整描述。如果失序或缓存不存在，则调用现有 `refresh_window_geometry`。该方法会读取完整 `ordered_windows`、重新计算同应用窗口对四角的遮挡、设置 panel level、调用 `orderWindow:relativeTo:`，并安排下一轮 RunLoop 延迟校验。完整校验成功后更新缓存；隐藏、移动或开始新一轮原生重排时清空缓存。

这样目标聚焦、同应用内部窗口切换或其他未知 Window Server reorder 都走同一条纠正路径，不依赖鼠标事件来源。

## 为什么不采用其他方案

全局 `NSEvent` mouseDown 监听虽然事件驱动，但会观察所有应用点击；它仍需要等待 Window Server 提交排序，并且 macOS 不同版本对跨应用输入监控的授权与投递行为更敏感。通过点击坐标判断目标也无法覆盖程序化聚焦。

Accessibility `AXObserver` 可以直接监听目标窗口事件，但需要辅助功能授权。系统权限必须由用户在系统设置中授予，不能保证是应用内直接点击“确定”即可完成，不符合当前功能的无权限目标。

私有 CGS API 不进入生产方案，避免 App Store、系统升级兼容性和签名审核风险。

## 错误与失败策略

- 轻量 Window ID 列表读取失败时不隐藏 overlay，也不把错误传播给采集会话；本次采样退化为现有完整刷新，让既有错误处理决定是否隐藏。
- 目标、缓存片段中的可见 panel 或已验证 sibling 缺失时视为失序，进入完整刷新。
- 完整刷新和延迟校验仍保持 fail-closed：无法安全维持相对层级时隐藏 overlay，不提升为 topmost。
- 暂停、停止、窗口最小化、离开当前 Space 和移动中的行为不变。

## 性能约束

- 100 ms 快速路径只获取 Window ID 数组，不创建全量窗口描述字典；
- 只有 frame 改变、overlay 隐藏、缓存尚未建立或 Z-order 片段变化时才执行 `ordered_windows` 完整扫描；
- 完整策略接受的 ToDesk sibling 稳定窗口栈必须收敛到轻量 `Keep`，不能每 100 ms 重复扫描描述字典；
- 不缩短 timer 周期，不增加 display link，不持续修改原生窗口；
- 保留 2 秒纠偏 timer 作为遗漏事件和异常状态的兜底。

## 测试设计

### 纯逻辑测试

- 四个 panel 连续位于目标之前时判定有效；
- 目标被点击并排到 panel 之前时判定失序；
- 完整策略已验证安全的同 owner sibling 位于 panel 与目标之间时保持有效；
- 缓存建立后出现未知窗口或缺失可见 panel 时判定失序；
- 四角全部隐藏时以同 owner sibling 为锚点，目标前置后判定失序；
- 应该可见的 panel 缺失时判定失序；
- frame 稳定但层级失序时选择 `Refresh`，而不是 `Keep`。

### macOS 真实 API 测试

- `CGWindowListCreate` 返回的轻量 ID 列表包含当前 GUI 会话中的有效可见窗口；
- 创建真实目标窗口、sibling 和四个 panel，建立正确顺序并缓存片段后断言快速校验通过；
- 重新 `orderFront` 目标窗口模拟点击聚焦，推进主 RunLoop 后断言快速校验失败；
- 执行现有完整重排后断言下一轮 RunLoop 恢复正确顺序，并等待多个 100 ms 周期确认片段保持稳定。

真实窗口测试标记为 macOS ignored，要求已登录 GUI session；纯逻辑测试必须进入常规测试套件。

## 验收标准

- 点击被共享窗口后不再出现约 1 秒以上的绿色角标下沉；
- 自动检测和纠正延迟不超过一个 100 ms 采样周期加一次主 RunLoop 提交；
- 点击其他遮挡窗口时，overlay 不越过该窗口；
- 移动、缩放、最小化、切换 Space 和同应用多窗口遮挡回归通过；
- 完整 feature 测试、格式检查和 Clippy 通过；
- 真机人工重复点击目标窗口至少 20 次，不出现持续失序或权限提示。
