# Windows Overlay 保留 16 ms 轮询方案

> 方案状态：**未采纳（REJECTED）**
>
> 评估日期：2026-07-12

## 问题背景与目标

原方案使用 `recv_timeout(16 ms)` 同时承担内部命令等待与 Overlay 线程节拍。评估该方案是为了确认能否在不改线程等待结构的情况下，通过保留现状满足拖动流畅度要求。

## 基线结果

真实 Win32 owner window、8 个 Overlay HWND 与拖动开始事件的测量为 **18.590–28.983 ms**。最慢结果接近两帧，且延迟受轮询相位影响；等待期间 Win32 消息队列没有被泵送，`WINEVENT_OUTOFCONTEXT` 回调不能及时执行。

## 决策

该方案无法满足 Overlay 在单帧内隐藏、拖动起始无明显卡顿的要求，因此未采纳并被事件驱动等待替换。

## 参考

- Rust 标准库：[Receiver::recv_timeout](https://doc.rust-lang.org/std/sync/mpsc/struct.Receiver.html#method.recv_timeout)
- Microsoft Learn：[SetWinEventHook](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwineventhook)
