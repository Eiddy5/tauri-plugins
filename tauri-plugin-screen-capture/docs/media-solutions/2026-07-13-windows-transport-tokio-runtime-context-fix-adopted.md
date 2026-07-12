# Windows transport Tokio runtime 上下文修复

> 方案状态：**ADOPTED（已采纳）**
>
> 采纳日期：2026-07-13
>
> 采纳标记：`ADOPTED-WINDOWS-TRANSPORT-TOKIO-RUNTIME-CONTEXT-FIX`

## 问题背景

Windows 全 GPU 编码启动成功后，`screen-capture-h264-transport` 原生线程立即 panic：

```text
there is no reactor running, must be called from the context of a Tokio 1.x runtime
```

捕获线程仍持续产生帧，但 transport 已退出，因此示例 App 没有共享视频输出，停止时还会报告 transport worker panic。

## 根因

原代码写成：

```rust
runtime.block_on(tokio::time::timeout(duration, send_future))
```

Rust 会先求值函数参数，所以 `tokio::time::timeout` 在进入 `Handle::block_on` 之前就在普通 OS 线程创建 timer。该线程尚无 Tokio reactor，构造阶段即 panic；传入的 runtime handle 本身有效。

## 已采纳修复

把 timer 的创建移动到 `block_on` 驱动的 async 块内部：

```rust
runtime.block_on(async move { tokio::time::timeout(duration, send_future).await })
```

这样 transport 原生线程先进入已有 runtime 上下文，再创建并轮询 timeout。2 秒发送上限、发送失败后的 IDR 恢复 gate 和有序样本交接语义保持不变。

## 验证

- 回归测试 `transport_timeout_enters_runtime_before_constructing_timer` 在修复前稳定产生同一 `no reactor running` panic，修复后通过。
- Windows encoder worker 5 项测试全部通过。
- 重新构建并操作示例 App：共享画面正常输出，捕获约 59.4 FPS、发布约 59.0 FPS、`framesCpuReadback=0`、后端为 `media-foundation-d3d11-surface`。
- 运行期间和关闭 App 后日志 `PANIC_COUNT=0`，transport 正常清理。

## 参考

- [Tokio `Handle::block_on`](https://docs.rs/tokio/latest/tokio/runtime/struct.Handle.html#method.block_on)：在关联 runtime 上驱动 future；timer 必须在可用 runtime 上下文中执行。
- [Tokio `time::timeout`](https://docs.rs/tokio/latest/tokio/time/fn.timeout.html)：创建带 deadline 的 future，超时会取消内部 future。

## 已知边界

该修复解决的是 timer 构造时机，不改变 `Handle::block_on` 对 runtime 生命周期的要求。publisher 必须保证 transport worker 退出前 runtime 仍然有效；runtime 已关闭属于另一类生命周期错误。
