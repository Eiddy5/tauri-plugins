# Windows owned-window overlay 生命周期方案

> 方案状态：**已采纳（ADOPTED）**
> 采纳日期：2026-07-12

## 问题背景

窗口共享边框必须准确贴合 DWM 可见边界；拖动/缩放期间不能持续计算并出现追赶抖动；共享窗口被别的应用覆盖或最小化时，边框不能悬浮在其他应用之上。

## 要解决的问题

- overlay 使用真实可见窗口边框，而不是包含透明阴影的传统 `GetWindowRect`。
- `MOVESIZESTART` 后立即隐藏并暂停 location-change hook；操作结束且鼠标释放后再合并计算一次。
- overlay 与共享窗口处于同一 Z-order 所有者组，失焦/被覆盖时一起下沉，最小化时由系统隐藏。
- overlay 不抢焦点、不进入任务栏、不被自己的屏幕捕获采入。

## 采用方案

窗口捕获 overlay 创建为共享 HWND 的 owned、non-topmost `WS_POPUP`，附加 `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_LAYERED`，并设置 `WDA_EXCLUDEFROMCAPTURE`。只有整屏 overlay 使用 topmost。

边界优先取 `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`，失败才回退 `GetWindowRect`。WinEvent hook 监听 move/size start/end、location change、show/hide/destroy：开始时隐藏并卸载 location hook；结束后延迟合并，鼠标仍按下则继续等待，最终只计算一次并恢复 hook。

## 自动验证

```powershell
cargo test overlay -- --nocapture
cargo test --test overlay_geometry -- --nocapture
cargo test --lib owned_overlay_tracks_real_window_geometry_and_z_order -- --ignored --nocapture
```

覆盖内容包括：owned/non-topmost placement、8 段角标几何、DWM 边界优先、move/size start 隐藏、移动期间忽略 location change、结束延迟刷新、鼠标按住期间不刷新、hide/destroy 清理以及多 session hook 隔离。

真实 Win32 集成测试会创建 owner、owned overlay 和遮挡窗口，断言 overlay 像素坐标准确、遮挡窗口位于 overlay 之上、owner 最小化后 overlay 由系统隐藏。

## 参考

- Microsoft Learn：[Window Features / Owned Windows 与 Z-order](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features)。Owned window 始终位于 owner 之上，owner 最小化时由系统隐藏；激活另一个 top-level window 时，其窗口组进入更高 Z-order。
- Microsoft Learn：[DwmGetWindowAttribute](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmgetwindowattribute)，用于读取 DWM 扩展 frame bounds。
- Microsoft Learn：[GetWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindow)，定义 owner 与 Z-order 关系的查询语义。

## 未采用的替代方案

- 所有 overlay 永久 topmost：共享窗口失焦或被覆盖后仍压在其他应用上，违反层级要求。
- 拖动时持续轮询边界：造成重复 DWM/Win32 调用和视觉追赶。
- overlay 作为 child window：坐标与裁剪受 owner client area 约束，无法覆盖非客户区外框。
