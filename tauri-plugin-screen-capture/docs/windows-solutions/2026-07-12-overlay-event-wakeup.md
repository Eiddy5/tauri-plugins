# Windows Overlay 线程事件唤醒方案

> 方案状态：**已采纳（ADOPTED）**
>
> 采纳日期：2026-07-12

## 问题背景

共享窗口进入位置拖动或尺寸调整时，`EVENT_SYSTEM_MOVESIZESTART` 已经产生，但 Overlay 专用线程原先通过 `recv_timeout(16 ms)` 等待内部命令。等待期间该线程没有泵送 Win32 消息队列，导致 `WINEVENT_OUTOFCONTEXT` 回调必须等到轮询超时后才有机会执行。用户看到的结果是：拖动开始的一瞬间 Overlay 隐藏不及时，并伴随明显卡顿。

针对真实 Win32 owner window、8 个 Overlay 边框窗口和真实 `NotifyWinEvent(EVENT_SYSTEM_MOVESIZESTART, ...)` 建立自动回归测试后，原实现从事件发出到全部 Overlay 隐藏的五次基线测量落在 **18.590–28.983 ms**。这一范围超过 60 Hz 的单帧预算 16.67 ms，也解释了为什么卡顿集中出现在拖动刚开始的瞬间。

## 要解决的问题

- 让拖动/缩放开始事件到达后，Overlay 在一帧内隐藏，不再受固定 16 ms 轮询相位影响。
- 内部 Overlay 命令和 WinEvent 使用同一个消息唤醒机制，避免消息队列与 Rust 通道两套等待时钟互相阻塞。
- 空闲时不进行 1 ms 高频轮询；只有已有的延迟刷新任务需要到期时才使用有限超时。
- 保持 `WINEVENT_OUTOFCONTEXT` 的顺序投递和注册线程回调语义。

## 采用方案

Overlay 线程改为事件驱动等待：

1. Overlay 专用线程循环首先通过 `PeekMessageW(..., PM_REMOVE)` 泵送消息队列，然后处理内部命令与到期刷新。
2. 启动 Overlay 线程前创建一个 Win32 自动复位 Event；发送内部 Overlay 命令时，先写入 Rust `mpsc` 通道，再调用 `SetEvent` 唤醒等待线程。Event 句柄通过共享所有权覆盖所有 sender 与等待线程的生命周期，避免线程 ID/消息队列失效造成丢失唤醒。
3. Overlay 线程使用 `MsgWaitForMultipleObjectsEx` 同时等待 Event 和 `QS_ALLINPUT`，并启用 `MWMO_INPUTAVAILABLE`。因此无论唤醒来源是内部命令还是 WinEvent，线程都会立即返回并泵送消息队列；若等待返回 `WAIT_FAILED`，线程会记录错误并停止，不会进入无上限空转。
4. 没有待处理的延迟刷新时使用无限等待；有刷新任务时仅把距离其截止时间的剩余毫秒数作为超时。这样保留原有的 160 ms 稳定后重算行为，同时消除常驻轮询。
5. 唤醒后先泵送 Win32 消息，再排空由 WinEvent 回调写入的内部命令，最后执行到期刷新；拖动开始路径可立即隐藏 Overlay 并暂停位置计算。

Microsoft 说明，`WINEVENT_OUTOFCONTEXT` 会跨进程排队、异步投递且保持顺序；接收线程必须有消息循环，并且事件回调会在调用 `SetWinEventHook` 的同一线程执行。因此，Overlay hook 的注册、等待和消息泵必须位于同一专用线程。`CreateEventW` 创建的自动复位 Event 在一次等待被释放后会自动回到未触发状态，适合将内部命令并入同一等待循环；`SetEvent` 用于将它置为已触发状态。

## 自动验证

初始诊断使用真实 `NotifyWinEvent(EVENT_SYSTEM_MOVESIZESTART, ...)` 得到 18.590–28.983 ms 基线。为避免 Windows 对合成 `NotifyWinEvent` 的投递时序影响回归稳定性，提交测试创建真实 owner window 和 8 个真实 Overlay HWND，并直接调用同一 WinEvent registry 分发路径；内核 Event 方案的连续五次结果为：

| 次数 | 隐藏延迟 |
| ---: | ---: |
| 1 | 3.201 ms |
| 2 | 2.696 ms |
| 3 | 2.637 ms |
| 4 | 3.705 ms |
| 5 | 3.196 ms |

平均延迟为 **3.087 ms**，最慢一次为 **3.705 ms**，全部显著低于 8 ms 的回归阈值，也低于 60 Hz 单帧预算。相对基线最低值 18.590 ms，最慢的新结果仍约快 **5.0 倍**。

最终内核 Event 方案的连续 5 次复测范围为 **2.637–3.705 ms**，全部通过 8 ms 阈值；WinEvent 到命令的语义另由 registry 单元测试覆盖。

验证命令：

```powershell
cargo test --lib owned_overlay_hides_within_one_frame_on_movesize_start -- --ignored --nocapture
```

## 官方参考

- Microsoft Learn：[SetWinEventHook function](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwineventhook)。`WINEVENT_OUTOFCONTEXT` 事件需要排队但保证顺序；调用线程必须有消息循环，out-of-context 回调在注册 hook 的同一线程执行。
- Microsoft Learn：[CreateEventW function](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createeventw)。自动复位 Event 在释放一个等待线程后自动恢复为未触发状态。
- Microsoft Learn：[SetEvent function](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-setevent)。将 Event 置为已触发状态，使等待线程立即恢复执行。
- Microsoft Learn：[MsgWaitForMultipleObjectsEx function](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjectsex)。该函数可等待线程输入队列；`MWMO_INPUTAVAILABLE` 使已被 `PeekMessage` 查看但尚未移除的输入仍能唤醒等待。

## 相关独立方案文档

- [1 ms 高频轮询方案（未采纳）](./2026-07-12-overlay-1ms-polling-rejected.md)
- [批量隐藏 Overlay HWND 方案（未采纳）](./2026-07-12-overlay-batched-hide-rejected.md)
- [保留 16 ms 轮询方案（未采纳）](./2026-07-12-overlay-16ms-polling-rejected.md)
