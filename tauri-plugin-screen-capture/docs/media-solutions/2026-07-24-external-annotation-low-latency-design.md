# 外部画板低延迟输入与插件自动合成方案

> 状态：**PROPOSED**
>
> 日期：2026-07-24
>
> 适用平台：macOS / Windows
>
> 目标：画板由调用方维护，插件只接收统一标注信息，并把标注自动合成到共享视频

## 结论

这套职责划分可以做到丝滑输入，但必须把两种延迟分开处理：

1. **本地反馈延迟**由外部画板负责。真实输入到达后立即更新本地“湿墨”预览，不能等待 JS → Rust IPC、视频合成、编码或本地回环视频。
2. **远端标注延迟**由插件负责。外部画板按显示帧批量提交真实采样点；插件保留最新源视频帧和最新标注版本，以单槽、latest-wins 的方式触发合成和发布。

在 60 Hz 屏幕上，一个显示帧为 16.7 ms；在 120 Hz 屏幕上为 8.3 ms。建议把本地真实笔迹首帧设为“不超过一个显示帧”的工程目标，把标注进入视频合成设为“不超过一个视频帧”的目标。远端最终可见延迟还包含编码、网络、抖动缓冲和解码，不应与本地笔迹延迟使用同一指标。

## 一手资料给出的约束

### Pointer Events：保留真实采样精度，预测点只用于临时预览

W3C Pointer Events Level 3 说明，浏览器可能把多次硬件采样合并成一次 `pointermove` 或 `pointerrawupdate`；`getCoalescedEvents()` 可以取回这些按时间排序的真实采样，因此更适合绘制平滑曲线。规范同时提供 `getPredictedEvents()` 降低感知延迟，但明确不规定浏览器采用什么预测算法。因此：

- 画笔应优先消费 `getCoalescedEvents()`，不支持时退化为父 `PointerEvent`；
- 父事件和其 coalesced events 二选一处理，不能重复追加；
- predicted events 只能绘制到可替换的临时层，下一批真实点到达后立即清除并重画；
- predicted events 不写入业务文档、不进入撤销历史、也不提交插件。

来源：[W3C Pointer Events Level 3，Coalesced and predicted events](https://www.w3.org/TR/pointerevents3/#coalesced-and-predicted-events)

成熟画板 tldraw 的当前源码采用相同原则：只有对高保真输入有收益的工具才启用 coalesced events，绘图和高亮工具启用，其他工具默认关闭；iOS 缺少接口时退化为普通事件。这说明 coalesced events 应作为渐进增强，而不是协议前提。

来源：

- [tldraw `useCanvasEvents.ts`：按工具消费 coalesced events](https://github.com/tldraw/tldraw/blob/f8a4bdc003b1031990ae9dee88c14da3c76dab21/packages/editor/src/lib/hooks/useCanvasEvents.ts#L215-L253)
- [tldraw `DrawShapeTool.ts`：画笔启用 coalesced events](https://github.com/tldraw/tldraw/blob/f8a4bdc003b1031990ae9dee88c14da3c76dab21/packages/tldraw/src/lib/shapes/draw/DrawShapeTool.ts#L8-L11)
- [tldraw `HighlightShapeTool.ts`：高亮启用 coalesced events](https://github.com/tldraw/tldraw/blob/f8a4bdc003b1031990ae9dee88c14da3c76dab21/packages/tldraw/src/lib/shapes/highlight/HighlightShapeTool.ts#L8-L11)

如果未来外部画板不是 WebView，而是 Windows 原生输入层，Win32 的 `GetPointerFramePenInfoHistory` 同样可以从最近一次 `WM_POINTERUPDATE` 取回被合并的历史采样点，接口语义与 Web 的 coalesced events 对齐。

来源：[Microsoft `GetPointerFramePenInfoHistory`](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getpointerframepeninfohistory)

### RAF：对齐本地显示和 IPC 批次，而不是降低输入采样率

`requestAnimationFrame()` 在下一次重绘前回调，通常跟随显示器刷新率；后台页面中通常会暂停。因此建议：

- 输入事件到达时立即把真实点写入内存中的当前 stroke，并把 predicted points 写入临时层；
- 只安排一个待执行的 RAF；
- RAF 中完成一次本地重画，并把自上次提交后积累的所有真实点打成一个增量批次；
- `pointerup` / `pointercancel` 必须立即安排最终 flush，不能依赖后台状态下可能暂停的 RAF；
- RAF 是提交节流器，不是采样器：一个 RAF 批次内必须保留所有 coalesced 真实点，或用明确的几何简化算法压缩，不能只保留最后一点。

来源：[MDN `requestAnimationFrame()`](https://developer.mozilla.org/en-US/docs/Web/API/Window/requestAnimationFrame)

Web Ink API 的设计进一步验证了“本地湿墨”和“最终文档”分层：它允许系统合成器在应用尚未渲染最新输入时补画 delegated ink trail，等应用渲染后再无缝替换。该 API 仍处于 WICG 孵化阶段，且不能假设 macOS WKWebView 和 Windows WebView2 都可用，因此本项目只能把它作为可选渐进增强，不能作为跨平台基础能力。

来源：[WICG Ink API](https://wicg.github.io/ink-enhancement/)

## 推荐运行架构

```text
Pointer Events / 原生指针输入
        │
        ├── 真实点 + coalesced points ──→ 外部画板模型
        │                                  ├── 本地湿墨即时预览
        │                                  └── RAF 增量批次
        │                                             │
        └── predicted points ─────────→ 临时预览层    │
                                                     ▼
                                         插件统一标注入口
                                                     │
                                     最新不可变场景快照/版本
                                                     │
                       最新源视频帧 ────────────────┤
                                                     ▼
                                          合成 worker 单槽
                                                     │
                                             编码并发布远端
```

外部画板是工具、选中、撤销、图层、协作和预测点的唯一所有者。插件不提供画笔工具状态，也不解释 pointer gesture；插件只校验、存储、合成标准图元。

## 插件入口：增量优先，整场景仅用于初始化和恢复

不应在每次 `pointermove` 后调用现有 `setAnnotationDocument` 替换整个文档。文档越大，序列化、校验和复制成本越高，而且旧请求可能晚于新请求完成。

建议公开以下语义，具体 Rust/TypeScript 名称可在实现阶段确定：

```text
replace_scene(session_id, revision, elements)
begin_stroke(session_id, revision, stroke_id, style, first_point)
append_points(session_id, revision, stroke_id, points[])
commit_stroke(session_id, revision, stroke_id)
upsert_elements(session_id, revision, elements[])
remove_elements(session_id, revision, element_ids[])
```

协议要求：

- `revision` 单调递增，插件拒绝或忽略旧版本；
- `stroke_id` / `element_id` 在会话内唯一，操作可幂等重放；
- 坐标、宽度继续归一化到输出视频空间；
- 点至少包含 `x/y/time`，压感画笔再携带 `pressure/tilt`；
- 插件用不可变快照或 copy-on-write 保存最新场景，合成线程不持有写锁；
- `replace_scene` 用于初始化、撤销后的大范围重建和版本失配恢复，不走高频路径。

## JS ↔ Rust IPC 策略

### 第一阶段不需要共享内存

Tauri 官方文档支持直接以 `ArrayBuffer` / `Uint8Array` 作为 raw request body，避免把点数组反复 JSON 序列化；官方同时说明事件系统中的异步监听在高频发送时可能乱序，并建议有序高吞吐场景使用 Channel。因此高频标注数据不应使用普通 Tauri event。

来源：[Tauri：Calling Rust from the Frontend](https://v2.tauri.app/develop/calling-rust/)

建议先使用“一次 RAF、一次二进制增量提交”：

- 60 Hz 显示最多约 60 次 IPC/s；120 Hz 本地显示仍可把插件提交限制为视频目标帧率，例如 60 次/s；
- 单个 IPC 包含这一帧内所有真实点和必要操作；
- 本地渲染不 `await invoke()`；
- 同一会话最多允许一个请求在途；在途期间的新点合并到下一批；
- ack 返回已应用的 `revision`；超时或版本失配时以 `replace_scene` 恢复；
- 二进制头包含协议版本、会话、revision、操作数和字节长度，Rust 端先做上限检查再解析。

共享内存/环形缓冲不是第一版必需条件。它会引入跨进程内存映射、生命周期、唤醒、覆盖和崩溃恢复等复杂度，而批量后的数据率通常远低于视频帧本身。只有实机指标同时表明 IPC 已成为瓶颈时再升级，触发条件应来自测量，例如：

- `ipc_enqueue_to_apply_ms` 的 p95 持续超过 4 ms；
- 在途批次长期大于 1，或频繁发生版本恢复；
- 序列化/反序列化明显占用 UI 或合成线程预算；
- 高频多指/多画布场景无法在目标视频帧率内清空积压。

升级时优先选择“固定容量单生产者/单消费者环形缓冲 + 单独唤醒信号”。环形缓冲只传增量操作；场景版本和整场景恢复仍走可靠命令通道。缓冲满时不能阻塞输入线程：合并未发送批次，必要时进行有误差上限的几何简化，然后发送最新完整 stroke 作为恢复，不能悄悄丢掉中间真实点。

## 视频合成刷新与背压

ScreenCaptureKit 的 `minimumFrameInterval` 用来限制更新频率，例如 `1/60` 表示最高 60 FPS；其 `queueDepth` 默认最少为 3，增加深度可能避免阻塞但会增加内存，Apple 明确要求不要超过 8。这说明采集缓冲不能无限加深来解决消费者过慢。

来源：

- [Apple `minimumFrameInterval`](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/minimumframeinterval)
- [Apple `queueDepth`](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/queuedepth)

Windows Graphics Capture 的 `FrameArrived` 只表示帧进入 frame pool；`CreateFreeThreaded` 可以让该事件在内部 worker thread 触发，避免依赖 UI `DispatcherQueue`。采集回调同样不应执行画板协议解析或复杂合成。

来源：

- [Microsoft `Direct3D11CaptureFramePool.FrameArrived`](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.framearrived)
- [Microsoft `Direct3D11CaptureFramePool.CreateFreeThreaded`](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.createfreethreaded)

推荐插件内部采用：

1. 始终保存最新的未标注源帧；
2. 始终保存最新已确认的标注场景 revision；
3. “新源帧”和“新标注 revision”都能唤醒同一个合成 worker；
4. worker 只有一个 pending slot，槽内保存“最新源帧句柄 + 最新标注 revision”，新状态覆盖旧的未开始状态；
5. 合成开始时取得一致快照；完成时若已有更新 revision，则不要排成长队，立即用最新状态安排下一帧；
6. 屏幕内容静止但标注变化时，必须重放最新源帧并发布新视频帧；
7. 输出按视频 deadline pacing，落后时从当前时间重建 deadline，不突发追帧；
8. stroke `commit`、清空、撤销等语义边界应触发一次最终发布，即使刚好与节流窗口重合。

这与仓库已经采纳的 Windows “最新帧按节拍续发、deadline pacing、不突发追帧”原则一致，标注只需成为同一 worker 的第二个唤醒源，而不是增加另一条编码队列。

## 建议指标和验收

以下是本项目的工程目标，不是平台 API 保证：

| 指标 | 建议目标 |
| --- | --- |
| 输入事件 → 本地真实湿墨首帧 | p50 ≤ 1 个显示帧，p95 ≤ 2 个显示帧 |
| RAF flush → Rust 应用 revision | p95 ≤ 4 ms |
| Rust 应用 revision → 进入合成 | p95 ≤ 1 个视频帧 |
| 同一会话 IPC 在途批次 | 正常为 0–1 |
| 合成 pending 深度 | 固定为 1 |
| 旧 revision 应用次数 | 0 |
| predicted point 写入正式文档次数 | 0 |
| 静态屏幕绘制时发布 FPS | 保持目标发布节拍 |

需要在 JS 和 Rust 使用可关联的 `session_id + revision + batch_id` 记录：

- `pointer_to_local_paint_ms`
- `batch_points`
- `ipc_enqueue_to_apply_ms`
- `annotation_revision`
- `annotation_revisions_skipped`
- `composite_ms`
- `annotation_to_publish_ms`
- `pending_overwrites`

验收至少覆盖鼠标、触控板、压感笔、60/120 Hz 显示器、静态共享画面、快速连续长笔画、窗口缩放、多屏不同 DPI、CPU/GPU 忙碌以及 IPC 人工延迟。判断“丝滑”必须看分位数和长笔画是否断点，不能只看平均 FPS。

## 决策建议

采用“外部画板即时预览 + RAF 二进制增量 + 插件不可变场景快照 + 单槽最新帧合成”的方案。第一阶段不要引入共享内存；先落地增量入口和全链路指标。只有 IPC 指标证明其占用超过预算，再在不改变公开标注协议的前提下把传输层替换为环形缓冲。

这能保持插件职责稳定：外部可自由更换 Web Canvas、WebGL、原生 AppKit/Win32 或第三方画板，而插件始终只负责“接收标准标注信息并自动合成到视频”。
