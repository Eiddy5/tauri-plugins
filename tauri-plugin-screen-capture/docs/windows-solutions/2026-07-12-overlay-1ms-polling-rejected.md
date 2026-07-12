# Windows Overlay 1 ms 高频轮询方案

> 方案状态：**未采纳（REJECTED）**
>
> 评估日期：2026-07-12

## 问题背景与目标

原实现用 `recv_timeout(16 ms)` 驱动 Overlay 线程，WinEvent 消息泵最多被固定等待阻塞一帧。此候选方案仅把等待间隔降到 1 ms，目标是减少共享窗口刚开始拖动时的 Overlay 隐藏延迟。

## 实验结果

真实 Win32 owner window 与 8 个 Overlay HWND 的拖动起始测试下降到 **5.9–15.1 ms**，证明固定轮询是主要瓶颈，但仍存在明显抖动，不能稳定满足 8 ms 回归阈值。同时空闲线程每秒会无效唤醒约 1000 次。

## 决策

该方案以持续 CPU 唤醒换取部分延迟改善，仍不能稳定达到目标，因此未采纳。最终采用内核 Event 与消息队列的事件驱动等待。

## 参考

- Rust 标准库：[Receiver::recv_timeout](https://doc.rust-lang.org/std/sync/mpsc/struct.Receiver.html#method.recv_timeout)
- Microsoft Learn：[MsgWaitForMultipleObjectsEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjectsex)
