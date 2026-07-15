# macOS 屏幕共享 Overlay 设计

## 状态

已于 2026-07-14 确认进入实施；2026-07-15 将窗口移动同步调整为目标窗口定时采样方案。

## 目标

为显示器共享和单窗口共享增加原生 macOS 共享提示框，并尽量与现有 Windows 行为保持一致：

- 使用现有 `OverlayStyle` 默认值绘制绿色角标；
- 仅在本地显示，绝不进入 ScreenCaptureKit 输出；
- 不获取焦点，不拦截鼠标输入；
- 拖动目标窗口时隐藏，在最新边界处重新显示；
- 暂停采集时隐藏，停止采集时销毁；
- 空闲时几乎不增加 CPU/GPU 工作，不运行持续渲染循环。

## 现有架构

`ScreenCaptureState` 为每个采集会话持有一个 `ShareOverlay`。当前顺序是先启动采集，再启动 publisher，最后通过 `ShareOverlay::start` 显示共享提示。暂停、恢复、停止和异步采集结束已经分别驱动 `hide`、`show` 和 `stop`。

Windows 通过 8 个原生 Win32 popup window 实现该 trait。窗口共享使用 owned、non-topmost overlay；显示器共享使用 topmost overlay。WinEvent hook 驱动可见性和边界更新，`WDA_EXCLUDEFROMCAPTURE` 防止 overlay 进入采集输出。

macOS 当前由 `DefaultShareOverlayFactory` 返回 `NoopShareOverlay`，ScreenCaptureKit 的显示器过滤器也尚未排除任何 overlay。

## 非目标

- 不引入 egui、wgpu、GLFW、WebView 或 helper 可执行程序。
- 不增加交互控件、标注、文字、动画或公开的 overlay 样式 API。
- 不要求用户授予辅助功能或输入监控权限。
- 用户拖动窗口时，不按显示器刷新率连续跟随。
- 不修改 Windows overlay 实现及其行为。
- 不承诺零资源消耗，改为通过可测量的性能预算验收。

## 采用方案

每个活动 overlay 使用 4 个小型无边框 `NSPanel`，分别对应目标区域的 4 个角。每个 panel 包含一个透明、layer-backed 的 `NSView`，并通过两个纯色 `CALayer` 子层组成 L 形角标。

4 个 `32 x 32` point 的 surface 可以避免创建和合成覆盖整个显示器的透明 surface。所有 layer 只创建一次，仅在颜色、缩放或几何位置发生变化时更新。不使用 `CADisplayLink`、动画定时器、egui 渲染循环、Metal 渲染循环或逐帧 drawing callback。

实现使用与当前 Tauri 依赖图兼容的显式 `objc2` AppKit、Foundation、QuartzCore 和 CoreGraphics binding。

## 窗口配置

每个角标 panel 使用以下配置：

- borderless、transparent；
- 关闭 shadow；
- `opaque = false`，背景透明；
- `ignoresMouseEvents = true`；
- 不能成为 key window 或 main window；
- 不出现在 Window 菜单和正常窗口切换循环中；
- 加入所有 Space，并能显示在全屏应用所在的 Space；
- 移动、缩放、显示或隐藏时不使用 Core Animation 隐式动画；
- 使用稳定的内部 title 前缀，让 ScreenCaptureKit 能区分 overlay 和普通应用窗口。

显示器 overlay 使用适合本地共享提示的高窗口层级。窗口 overlay 不使用固定 topmost 或 floating 层级，而是动态读取目标窗口的 `kCGWindowLayer`，将 4 个 panel 设置到相同 `NSWindow.Level`，再相对目标窗口排序。最终行为必须通过原生集成测试验证，包括普通窗口、浮动窗口、全屏和 Stage Manager。

## 窗口层级与遮挡语义

窗口共享必须满足以下 Z-order 合约：

1. Overlay panel 与被共享窗口使用相同的 Window Server layer。
2. 4 个 panel 在同一 layer 内连续排列，并紧邻目标窗口之上。
3. 在目标窗口之前、也就是视觉上更靠前的其他窗口，必须继续位于全部 panel 之上。
4. 目标窗口失焦或被其他窗口部分遮挡时，由 Window Server 自然遮挡对应 panel 像素；不把 overlay 提升到遮挡窗口之上，也不把整个 overlay 粗暴隐藏。
5. 目标窗口重新获得焦点或 Z-order 发生变化时，重新建立“紧邻目标窗口之上”的关系。
6. 目标窗口最小化、隐藏、销毁或离开活动 Space 时，全部 panel 必须隐藏，不能遗留悬浮角标。

具体实现步骤：

1. 通过 `CGWindowListCopyWindowInfo` 读取目标 `CGWindowID` 的 `kCGWindowLayer` 和当前全局前后顺序。
2. 将每个 panel 的 `level` 设置为 `NSWindow.Level(rawValue: target_layer)`。
3. 依次调用 `panel.order(.above, relativeTo: Int(target_window_id))`，把 4 个 panel 插入目标窗口正上方。多次插入会让 panel 形成连续的小组，但仍位于原本遮挡目标的窗口之后。
4. 排序完成后重新读取有序窗口列表，验证目标与 panel 的相对顺序。若系统拒绝跨进程相对排序或结果不符合合约，则立即隐藏 overlay，不能回退到 topmost。

2026-07-14 的本机原生透明窗口探针已验证：`NSPanel.windowNumber` 能对应 Window Server 列表中的 `kCGWindowNumber`，并且以目标 `CGWindowID` 调用 `order(.above, relativeTo:)` 后，panel 位于目标正上方。该探针结论必须转化成可重复的 macOS ignored 集成测试，不能只依赖一次人工验证。

该方案不需要计算遮挡区域或拆分可见 segment。只要 panel 与目标同层级且紧邻目标上方，Window Server 就会按照真实 Z-order 合成部分遮挡和完全遮挡效果。

## 渲染与几何

跨平台的 `OverlayStyle` 继续作为唯一样式来源：

- 颜色：`#22C55E`；
- 粗细：`4` point；
- 角标长度：`32` point。

每个 panel 的尺寸为 `corner_length x corner_length`，内部两个子 layer 分别构成同一角的水平和垂直部分。目标区域过小时，先应用现有的长度钳制语义，再计算 panel 和子 layer frame。

AppKit 边界上的所有几何值都使用逻辑 screen point。每个 layer 的 `contentsScale` 跟随所在 `NSScreen` 的 backing scale factor，从而在 Retina 和混合缩放多显示器环境中保持边缘清晰。

几何和可见性更新通过 `CATransaction` 禁用隐式动画。不使用 `shouldRasterize`、模糊、mask、圆角、shadow 或异步自定义绘制；这些能力对纯色矩形没有收益，只会增加不必要的离屏工作。

## 边界与坐标转换

显示器 source ID 已包含 ScreenCaptureKit 使用的 `CGDirectDisplayID`。Overlay 通过 `NSScreenNumber` device description 找到对应 `NSScreen`，再读取该 screen 的 AppKit frame。

窗口 source ID 包含 `CGWindowID`。100 ms 快速路径通过 `CGWindowListCreateDescriptionFromArray` 只读取这个目标窗口的边界、可见性和 `kCGWindowLayer`；只有需要恢复显示、纠正 Z-order 或处理系统通知时，才通过 `CGWindowListCopyWindowInfo` 读取全局前后顺序。Quartz 窗口坐标必须按显示器转换成 AppKit screen coordinate，不能假设主显示器原点或统一缩放比例。测试需要覆盖位于主显示器上、下、左、右的显示器，以及 Retina 混合缩放布局。

如果 source 已不存在、已最小化、不在当前 Space，或者边界无效或为空，overlay 必须隐藏。采集 backend 仍然是最终报告 source unavailable 并结束采集会话的权威来源。

## 事件模型

窗口边界采用“目标窗口采样 + 稳定后重建”的状态机：

1. 窗口 overlay 可见期间，每 100 ms 通过 `CGWindowListCreateDescriptionFromArray` 精确读取目标 `CGWindowID`，不扫描全局窗口列表。
2. 首次发现 `x/y/width/height` 任一值变化时立即 `orderOut` 四个 panel，并记为移动中。此后只要每次样本仍变化，就保持隐藏。
3. 第一次得到与上次完全相同的样本时，将状态切回稳定；随后读取一次完整 Z-order，在同一个主线程事务中重定位 4 个 panel、同步目标 layer、重新建立并验证相对排序，然后恢复显示。
4. 监听 `NSWorkspace` 的应用激活、隐藏、终止、活动 Space、睡眠和唤醒通知。应用激活通知不能在回调栈内同步重排；必须安排到下一轮主 RunLoop，等 AppKit 完成 Window Server 激活排序事务后再检查 Z-order。目标被其他窗口盖住时不提升 panel，目标重新到前台时恢复相对排序。
5. 监听显示器配置变化，并重新计算显示器边界。
6. 另保留 2 秒低频校正，捕获程序化 reorder 或遗漏通知。窗口仍处于移动状态时，系统通知和低频校正只能保持隐藏，不能提前恢复 panel。

该方案不依赖鼠标来源，因此标题栏拖动、边缘缩放、程序化移动和程序化缩放行为一致。它不安装全局或本地 `NSEvent` monitor，也不使用 Accessibility `AXObserver` 或 `CGEventTap`，因此不要求辅助功能或输入监控权限。代价是隐藏与恢复的最坏检测时间受 100 ms 采样周期约束，而不是按 60 Hz 每帧同步。

## AppKit 主线程所有权

AppKit window 和事件监听器只能在 macOS 主线程创建、访问和销毁。

插件 setup 已经能取得 `AppHandle<R>`。实现新增一个由 `AppHandle::run_on_main_thread` 支撑的内部 object-safe 主线程 dispatcher，并在插件初始化时注入 `MacOsShareOverlayFactory`。

`ShareOverlay` 必须继续满足 `Send + Sync`，所以不能直接持有 `NSPanel`、`NSView`、`CALayer` 或事件监听对象。Rust overlay handle 只持有生成的 overlay ID 和 dispatcher；真实 AppKit 状态存放在以 overlay ID 为键、仅主线程访问的 registry 中。异步 trait 方法向主线程派发命令，并等待 oneshot response。

这种结构与 Windows 的“会话侧 handle”和“UI 线程原生窗口状态”分离方式一致，同时避免为 macOS 引入第二套事件循环。

## ScreenCaptureKit 排除策略

不使用 `NSWindow.sharingType`。Apple 已将其不共享取值定义为 legacy 机制，不能依赖它从采集内容中隐藏窗口。

显示器采集通过以下方式构造 ScreenCaptureKit filter：将当前 `SCRunningApplication` 设为 excluded application，再把属于当前进程且不是 overlay 的所有可见窗口作为 `exceptingWindows` 传入。

该策略具备三个关键性质：

1. 普通 Tauri 应用窗口仍能出现在显示器共享中。
2. 所有 overlay panel 都会被排除，包括 stream 启动后新建的 panel。
3. 并发采集会话之间的 overlay 也会互相排除，无需为每个 stream 维护 overlay window ID 列表。

Overlay window 通过当前进程 ID 和保留的内部 title 前缀双重识别。如果无法把当前进程解析成 `SCRunningApplication`，显示器采集必须启动失败，不能回退为可能泄漏 overlay 的无过滤采集。

单窗口采集继续使用 `SCContentFilter::with_window`。因为 filter 只包含选定目标窗口，overlay panel 不会进入该 stream。

如果显示器采集运行期间创建了新的普通 Tauri 窗口，需要刷新 filter，把它加入 `exceptingWindows`；否则该窗口会随当前应用的其他内容一起被排除。这属于应用窗口可见性正确性问题，不会造成 overlay 泄漏。首版可以在现有低频校正路径刷新；如果能取得合适的 Tauri window-created event，则应增加立即刷新。

## 模块划分

```text
src/overlay/
  mod.rs                         跨平台 trait、样式和几何
  macos/
    mod.rs                       MacOsShareOverlay facade 和 factory
    dispatcher.rs                object-safe Tauri 主线程派发
    registry.rs                  仅主线程访问的 overlay 状态 registry
    panel.rs                     NSPanel/NSView/CALayer 创建、层级和更新
    window_info.rs               source ID、CGWindow/NSScreen 边界、层级和坐标
    events.rs                    NSTimer 和 NSWorkspace observer 生命周期
  windows/                       保持现有 Windows 实现

src/platform/macos/
  screencapturekit.rs            显示器 filter 排除策略
```

如果某个模块最终非常简单，可以合并文件；但 AppKit 所有权、边界转换、事件处理和 ScreenCaptureKit filtering 必须保持职责分离。

## 会话生命周期与回滚

显示器 filter 会在 panel 创建前排除 overlay 所属的整个应用，因此可以保留现有会话顺序：

1. 使用带排除策略的 filter 启动 ScreenCaptureKit；
2. 启动 publisher；
3. 创建并显示 overlay；
4. 插入 session record。

如果 overlay 启动失败，沿用现有 state 逻辑停止 capture 和 publisher。原生启动过程还必须在返回错误前，使已创建的 timer 失效、移除 notification observer，并关闭已创建的部分 panel。

暂停时通过 `orderOut` 隐藏 panel，并暂停窗口位置 timer 和低频校正 timer。恢复时先重新计算最新边界，再把 panel 排到前面。停止时移除 notification observer，使两个 timer 失效，关闭所有 panel，并移除 registry entry。所有操作都必须幂等，确保异步采集结束可以与显式 stop 安全竞争。

## 错误处理

- source ID 无效时返回 `SourceNotFound`。
- 启动期间窗口消失时返回 `SourceUnavailable`。
- AppKit 创建或主线程派发失败时返回 `Internal`，并包含平台上下文。
- 无法构造安全的显示器 filter 时返回 `CaptureStartFailed`，不能回退到无过滤采集。
- timer 或 observer 安装失败时清理部分 overlay 并使启动失败。
- 目标 layer 或跨进程相对排序无法建立、验证失败时隐藏 overlay，并输出限频诊断；不能回退到 topmost 或 floating。
- 运行时边界读取失败时隐藏 overlay，并输出限频诊断；不直接停止 frame publishing。
- `show`、`hide` 和 `stop` 必须保持幂等。

## 测试策略

### 纯逻辑与 mock 测试

- 从 `OverlayRect` 和 `OverlayStyle` 正确计算 4 个角标 panel frame；
- 水平和垂直子 layer frame 与 `corner_segments` 的 8 个 segment 一致；
- 小目标钳制后不会产生负尺寸或相互越界；
- Quartz 到 AppKit 坐标转换支持负原点和不同显示器布局；
- Z-order 计算确认 4 个 panel 连续位于目标之上，并保持所有原有前置窗口仍在 panel 之上；
- 目标 layer 变化时，panel level 会同步更新；
- 目标窗口 frame 首次变化时进入隐藏状态，持续变化时保持隐藏，首次稳定样本触发一次恢复；
- 低频纠偏在 frame 仍变化时不能绕过隐藏状态；
- 会话生命周期通过现有抽象调用 macOS overlay 的 start/show/hide/stop；
- 启动回滚会销毁部分创建的原生状态；
- 显示器 filter 分类器排除保留前缀的 overlay，并重新包含当前进程的普通窗口；
- 窗口采集只包含目标窗口，因此忽略无关 overlay。

### macOS 原生 ignored 测试

- 创建 4 个真实 panel，并确认其 non-key、click-through、无 shadow、无边框；
- 在 Retina 显示器上，显示器和窗口 panel 坐标与真实目标边界一致；
- 4 个真实 panel 与目标窗口使用相同 `kCGWindowLayer`，并在 Window Server 列表中紧邻目标之上；
- 创建一个覆盖窗口后，覆盖窗口位于 panel 和目标之前，实际像素遮挡 overlay；
- 将覆盖窗口失焦并重新激活目标后，panel 会恢复到目标正上方，但不会越过更靠前的其他窗口；
- 同一目标应用的另一个窗口盖住目标时，该窗口同样盖住 overlay；
- 交互式或程序化移动、缩放时，frame 变化后的一个采样周期内隐藏 panel；
- 停止改变 frame 后，在第一个稳定样本触发重定位并恢复 panel；
- pause/resume/stop 后不残留可见 panel、observer 或 timer；
- 切换应用、Space、全屏和 Stage Manager 时，窗口 overlay 始终遵守目标 layer 和相对顺序，不会悬浮在无关内容之上；
- 目标最小化、隐藏、关闭或切换出活动 Space 后，不残留任何可见 panel；
- ScreenCaptureKit 显示器 frame 不包含绿色 overlay，同时普通 Tauri 窗口仍然可见；
- 单窗口 frame 不包含 overlay；
- 两个并发采集会话不会采集到彼此的 overlay。

### 回归命令

```bash
cargo fmt --check
cargo test
cargo test --features macos-screencapturekit
cargo clippy --features macos-screencapturekit --all-targets -- -D warnings
```

原生 ignored 测试需要在已登录的 macOS GUI session 中运行，并提前授予屏幕录制权限。

具体真机步骤见 [macOS 浮层验收清单](../../macos-overlay-acceptance.md)。

## 性能预算

必须通过 Instruments 测量 release build；不能仅凭使用 Core Animation 就认定性能合格。

- 不使用 display-link 或持续渲染循环；
- 不创建覆盖整个显示器的透明 surface；
- 在受支持的 Apple Silicon 测试机上，overlay 空闲 CPU 低于 `0.5%`；
- 目标 frame 实际变化到 panel 隐藏的 P95 低于 `150 ms`；
- 目标 frame 最后一次变化到 panel 重新可见的 P95 低于 `150 ms`；
- 不产生持续的 Core Animation offscreen rendering pass；
- overlay 更新不增加 ScreenCaptureKit frame copy、encoder 工作或 publisher 工作；
- 100 ms 目标采样未发现变化时，不扫描全局窗口列表，也不提交 AppKit 几何更新；
- 2 秒低频快照没有变化时，不提交 AppKit 几何更新；
- Z-order 没有变化时，不重复调用 `order(.above, relativeTo:)`。

## 风险与缓解措施

### 跨进程 Z-order

macOS 没有 Win32 实现所使用的 owned-window 关系，因此不能依赖系统自动绑定生命周期和层级。实现通过同步 `kCGWindowLayer` 并使用 `order(.above, relativeTo: target_window_id)` 建立相邻顺序，使更靠前的窗口自然遮挡 overlay。每次相关激活、目标 frame 恢复稳定、Space 变化或低频快照发现 reorder 时重新验证该关系。若验证失败则隐藏 overlay，绝不回退为永久 topmost。最终接受该策略前，必须通过真实跨进程窗口测试。

### 坐标系统

Quartz 和 AppKit 的全局坐标约定不同，多显示器还可能存在负原点和不同缩放。所有转换集中放在 `bounds.rs`，按显示器映射，并通过纯逻辑测试和真实 Retina 测试覆盖布局矩阵。

### ScreenCaptureKit filter exception

排除当前应用、再重新包含普通窗口，比使用已弃用的 AppKit sharing flag 隐藏 overlay 更安全。新建普通应用窗口后必须刷新 filter。无法构造排除策略时必须 fail closed。

### 不使用辅助功能权限的采样取舍

不申请辅助功能权限意味着无法使用跨进程 AX move/resize observer。100 ms 目标窗口采样统一覆盖交互式与程序化几何变化，workspace/display 通知覆盖生命周期变化，2 秒低频快照负责 Z-order 和遗漏通知纠正。快速路径只查询一个 `CGWindowID`，避免以 10 Hz 扫描完整 Window Server 列表。

### Core Animation 性能假设

4 个小型静态 surface 预期成本很低，但真实性能仍受硬件和 compositor 影响。必须执行既定 Instruments 性能预算并保留原生回归测量，不能只依赖架构推断。

## 官方参考

- Apple `NSWindow.order(_:relativeTo:)`：在同一窗口层级内，将窗口放到指定 window number 的正前方或正后方。<https://developer.apple.com/documentation/appkit/nswindow/order(_:relativeto:)>
- Apple `kCGWindowNumber`：Window Server 当前用户会话内唯一的 window ID。<https://developer.apple.com/documentation/coregraphics/kcgwindownumber>
- Apple `kCGWindowLayer`：窗口在 Window Server 中的 layer number。<https://developer.apple.com/documentation/coregraphics/kcgwindowlayer>
- Apple `CGWindowListCopyWindowInfo`：返回有序窗口列表、ID、边界、layer 和 owner 等信息。<https://developer.apple.com/documentation/coregraphics/cgwindowlistcopywindowinfo(_:_:)>
