# Windows Overlay 批量隐藏 HWND 方案

> 方案状态：**未采纳（REJECTED）**
>
> 评估日期：2026-07-12

## 问题背景与目标

共享边框由 8 个 HWND 组成。候选假设是逐个调用隐藏操作触发多次窗口管理/DWM 更新，导致拖动开始瞬间卡顿；目标是用 DeferWindowPos 批处理一次提交全部隐藏。

## 实验方案与结果

实验使用 `BeginDeferWindowPos`、`DeferWindowPos`、`EndDeferWindowPos` 批量隐藏 8 个 Overlay HWND。实测拖动起始隐藏延迟约 **7.5–14.2 ms**，没有实质消除延迟，说明主要瓶颈不在逐个隐藏，而在 Overlay 线程等待期间没有及时泵送 WinEvent。

## 决策

该方案增加窗口位置批处理复杂度但没有解决根因，因此未采纳，实验代码已撤回。

## 参考

- Microsoft Learn：[BeginDeferWindowPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-begindeferwindowpos)
- Microsoft Learn：[DeferWindowPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-deferwindowpos)
- Microsoft Learn：[EndDeferWindowPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enddeferwindowpos)
