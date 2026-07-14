# macOS 浮层延迟层级校验设计

## 状态

已于 2026-07-14 确认采用方案 A：在下一轮 AppKit RunLoop 校验窗口相对层级。

## 问题

窗口共享启动时，四个 `NSPanel` 通过 `orderWindow:relativeTo:` 排到目标窗口正上方。AppKit 将这次窗口服务器排序延迟提交到下一轮主 RunLoop，但当前实现紧接着调用 `CGWindowListCopyWindowInfo` 校验。此时查询结果尚未包含新的 panel 顺序，代码误判层级不可维持、隐藏浮层，并把错误传播给 `start_capture`，最终中止本来已经成功启动的窗口采集。

真机探针稳定观察到以下顺序：

1. 调用四次 `orderWindow:relativeTo:`；
2. 同一调用栈立即查询时看不到四个 panel；
3. 主 RunLoop 推进后再次查询，四个 panel 连续位于目标窗口正上方且层级一致。

## 目标

- 不在 AppKit 尚未提交排序时误判失败；
- 排序提交后仍校验四个 panel 与目标窗口的相对层级；
- 真正无法维持层级时继续 fail-closed：隐藏浮层，不降级为置顶；
- 浮层校验失败不得中止已经建立的屏幕共享；
- 暂停、停止或新一轮刷新后，旧校验回调不得错误隐藏当前浮层；
- 不改变 Windows、显示器共享和 ScreenCaptureKit 采集行为。

## 方案

### 两阶段刷新

`OverlayHost::refresh_window` 保留刷新前的层级读取和 `needs_native_update` 判断。当需要更新原生窗口时，它只执行边界更新、设置目标层级和相对排序，随后更新本地可见状态并安排一次主 RunLoop 延迟校验，不再在同一调用栈读取排序结果。

延迟校验由 `interval = 0`、`repeats = false` 的一次性 `NSTimer` 调度到下一轮主 RunLoop。回调只携带会话 ID 和校验代次，不跨线程持有 AppKit 对象。回调进入现有主线程 host registry 后先确认代次仍是最新待校验代次，再重新读取窗口列表并调用现有 `verify_relative_order`。

### 回调合并与过期保护

每个 `OverlayHost` 保存单调递增的刷新代次和最新待校验代次。每次原生排序都安排携带对应代次的下一轮 RunLoop 回调；只有与最新 pending 代次完全一致的回调能够消费校验，较旧回调直接结束。这样既不会重复执行有效校验，也不会让已取消的 Timer 消费隐藏后新创建的校验。

出现以下任一情况时，回调直接结束：

- 会话已经停止，registry 中不存在对应 host；
- 用户已经暂停或主动隐藏浮层；
- 正在拖动目标窗口；
- 回调代次早于 host 当前刷新代次。

这样可以避免旧回调在恢复、重新定位或停止之后改变新状态。

### 真实失败处理

延迟查询验证成功时，清除待校验状态并保持浮层显示。验证失败时隐藏四个 panel、清除可见状态，并用结构化 debug 日志记录会话 ID 和目标窗口 ID。不得将 panel 提升为固定置顶层级。

由于失败发生在初始 `start` 返回之后，它不会停止 capture 和 publisher。后续应用切换、空间切换或低频纠偏事件仍可触发刷新并尝试恢复浮层。

读取目标窗口或窗口列表在同步刷新阶段发生错误时，继续沿用现有错误传播行为；本次修复只改变排序提交后的验证时机。

## 测试

### 纯逻辑回归测试

增加一个可独立测试的校验状态模型，覆盖：

- 原生排序刚提交时只进入“等待校验”，不会立即判失败；
- 下一轮 RunLoop 的有效顺序完成校验；
- 下一轮 RunLoop 的无效顺序请求隐藏；
- 暂停、拖动、停止或过期代次忽略旧回调；
- 多次刷新合并且只验证最新代次。

测试先以当前实现运行并因缺少延迟状态而失败，再实现最小生产代码使其通过。

### 真机反馈环

保留一个 agent 可运行的 AppKit 探针作为原始症状验证：创建四个 panel，相对一个外部 layer-0 窗口排序，断言同栈查询尚未提交，而下一轮 RunLoop 查询顺序有效。实现完成后，用示例应用执行一次窗口共享，确认 `start_capture` 不再返回“无法维持目标窗口的相对层级”。

### 完整验证

依次运行：

```bash
cargo test --test macos_overlay_policy
cargo test --all-targets --features macos-screencapturekit
cargo clippy --all-targets --features macos-screencapturekit -- -D warnings
cargo fmt --check
```

## 非目标

- 不改变四角浮层视觉样式；
- 不引入持续逐帧轮询；
- 不增加辅助功能或输入监控权限；
- 不把浮层改为目标窗口的 child window，因为外部应用窗口不能建立 AppKit 父子关系；
- 不放宽 `verify_relative_order` 对遮挡窗口和 WindowServer layer 的校验。
