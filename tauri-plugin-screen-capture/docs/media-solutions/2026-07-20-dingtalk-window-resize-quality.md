# 钉钉窗口共享缩放与清晰度策略调研

> 文档状态：**RESEARCH（调研结论，不代表钉钉桌面客户端内部实现已公开）**
>
> 日期：2026-07-20
>
> 适用平台：macOS 窗口共享、示例 App 接收渲染

## 结论

钉钉没有公开桌面客户端在窗口 resize 时的捕获与编码源码，因此不能确认它具体调用了哪些私有策略或图像增强算法。公开资料能确认的是：

1. 钉钉会议把普通屏幕共享与“视频流畅度优先”区分为两种使用方式；后者用于共享视频文件并提升流畅度。这说明产品层至少存在“文字/文档清晰度”和“高运动流畅度”的场景取舍，而不是所有内容都固定使用同一组分辨率和帧率。[钉钉会议：在 PC 端共享屏幕](https://help.aliyun.com/zh/document_detail/208711.html)
2. DingRTC 的 macOS 屏幕流配置默认分辨率是 `0 × 0`，官方定义为“推流分辨率等于屏幕采集分辨率”，上限为 `3840 × 2160`；默认帧率为 5 FPS、最高 30 FPS；默认码率为 0，表示 SDK 根据分辨率和场景自动计算，且官方明确举例说明 PPT 与游戏应使用不同码率策略。[DingRTC iOS/Mac 数据类型：DingRtcScreenShareEncoderConfiguration](https://help.aliyun.com/document_detail/2667833.html)
3. Apple 对 ScreenCaptureKit 的建议也是：文字、表格等低运动内容可以使用 4K/10 FPS；高运动视频则使用 1080p/60 FPS，即用帧率换空间清晰度或反向取舍。[Apple WWDC22：Meet ScreenCaptureKit](https://developer.apple.com/videos/play/wwdc2022/10156/)

因此，最接近公开证据的“钉钉式”方案不是把缩小后的窗口强制插值到固定 2K/60 FPS，而是：**保持源像素有效信息，按内容场景调整编码分辨率、帧率和码率；接收端独立负责把视频铺到完整视图。**

## 已验证的钉钉 / DingRTC 事实

### 1. 捕获和接收渲染是两个独立问题

DingRTC macOS SDK 提供按窗口 ID 开始共享的接口 `startScreenShareWithWindowId`，也提供屏幕共享配置的运行时更新接口 `updateScreenShareConfig`。[DingRtcEngine macOS 接口](https://help.aliyun.com/zh/document_detail/2667826.html)

DingRTC 的远端渲染另有独立显示模式：

- `Auto`：自动；
- `Stretch`：拉伸填满，不保持比例；
- `Fill`：保持比例并以黑边填充；
- `Crop`：保持比例并裁剪以填满视图。

官方把这些模式定义在渲染画布上，而不是捕获分辨率上。[DingRTC Mac 基本功能：渲染模式](https://help.aliyun.com/document_detail/2640103.html)

这意味着“接收方始终显示满播放器”不要求发送方预先把较小窗口放大到一个固定大画布。发送方可以传接近源分辨率的画面，由接收方按 `contain` / `cover` 语义完成显示。

### 2. DingRTC 屏幕流默认跟随采集分辨率，并自动选择码率

DingRTC 官方对屏幕共享编码参数的定义为：

| 参数 | 官方默认值 | 官方说明 |
| --- | ---: | --- |
| `dimensions` | `0 × 0` | 推流分辨率等于屏幕采集分辨率；最大 3840 × 2160 |
| `frameRate` | 5 FPS | 最大 30 FPS |
| `bitrate` | 0 | SDK 根据视频分辨率和场景自动计算，官方推荐保留默认值 |

官方还明确指出，PPT 画面变化较小、游戏变化较大，因此屏幕流所需码率差异很大；手动设置时建议不超过 3000 Kbps。[DingRTC iOS/Mac 数据类型](https://help.aliyun.com/document_detail/2667833.html)

钉钉会议室连接器的公开规格另外说明内容分享流支持分辨率与帧率自适应，最高 1080p@30fps；这是连接器范围的事实，不能直接外推为钉钉桌面客户端的完整上限。[钉钉会议室连接器 FAQ](https://help.aliyun.com/zh/document_detail/208859.html)

### 3. 清晰度优先通常需要降低帧率，而不是放大像素

钉钉会议提供“视频流畅度优先”开关，用于共享视频文件时提高流畅度。[钉钉会议共享说明](https://help.aliyun.com/zh/document_detail/208711.html)

这与 Apple 的 ScreenCaptureKit 官方建议一致：

- Notes、电子表格等低运动且文字清晰度重要的内容：4K、10 FPS；
- 高运动视频：1080p、60 FPS。

Apple 同时说明 ScreenCaptureKit 可以达到显示器原生分辨率和刷新率，并允许在运行中更新过滤器与配置。[Apple WWDC22：Meet ScreenCaptureKit](https://developer.apple.com/videos/play/wwdc2022/10156/)

## ScreenCaptureKit 在窗口 resize 时的真实行为

Apple 明确说明：

- 流的输出尺寸基本固定，不会随源窗口自动 resize；
- 频繁改变输出尺寸会产生额外内存分配，因此不推荐；
- 当源窗口变大时，ScreenCaptureKit 会进行硬件缩放，使窗口内容不超过配置的输出帧；
- 当窗口最小化时，输出暂停，恢复窗口后继续输出。

来源：[Apple WWDC22：Take ScreenCaptureKit to the next level](https://developer.apple.com/videos/play/wwdc2022/10155/?time=1345)

ScreenCaptureKit 还在帧元数据中提供 `contentRect` 和 `contentScale`。Apple 的示例说明，可先按 `contentRect` 裁出有效内容，再按 `contentScale` 还原，从而按接近源窗口 1:1 像素的方式显示。[Apple WWDC22：contentRect / contentScale](https://developer.apple.com/videos/play/wwdc2022/10155/?time=276)

## 当前实现为什么会变模糊

当前 macOS 配置在固定输出宽高上启用了：

```rust
.with_scales_to_fit(true)
.with_preserves_aspect_ratio(true)
```

当窗口从较大尺寸缩小后，窗口自身只会渲染更少的源像素。`scalesToFit(true)` 再把这些像素放大到固定的 1080p/2K 画布。放大只能插值，不能恢复源窗口已经不再产生的细节；随后还要经过 H.264 压缩，所以文字边缘和细线会进一步变软。

这不是码率单一问题。即使增加码率，也只能更准确地保存已经插值过的模糊画面，不能生成真实细节。

## 对本项目的建议策略

### 建议一：捕获层不做无条件向上放大

把“视频画布填满”和“采集像素放大”拆开：

- 捕获/编码尽量使用窗口当前的原生像素尺寸，最多向下缩到用户选择的清晰度上限；
- 不把小于清晰度档位的窗口在 ScreenCaptureKit 阶段强制向上插值；
- 接收端 `<video>` 使用固定容器和 `object-fit: contain`，保持比例并铺满可用区域；若产品允许裁剪，则使用 `cover`。

这样接收端仍是完整的大播放器，但编码器处理的是高质量的源像素，不是预先放大的插值像素。

### 建议二：使用两个内容档位

| 场景 | 分辨率策略 | 帧率策略 | 码率/拥塞策略 |
| --- | --- | --- | --- |
| 文档清晰度优先（默认） | 源原生分辨率，上限 2K/4K | 5–15 FPS，静态内容可重复最后一帧 | 优先保分辨率，自动或内容自适应码率 |
| 视频流畅度优先 | 上限 1080p | 30–60 FPS | 优先保帧率，按 REMB/网络反馈降分辨率或码率 |

这与钉钉公开的“视频流畅度优先”入口以及 Apple 的 4K/10 FPS 与 1080p/60 FPS 建议一致。

### 建议三：如果确实需要改变编码尺寸，采用阈值化重配置

如果固定编码画布仍导致明显质量浪费，可在窗口稳定 resize 后更新捕获/编码尺寸，但不要跟随每一个鼠标拖动事件：

- debounce 300–500 ms；
- 只在尺寸变化超过约 15%–20% 或跨越清晰度档位时更新；
- 保持宽高为偶数；
- 更新后发送 IDR 与新的 SPS/PPS；
- 验证 WebRTC 接收端是否能在同一会话中稳定处理 H.264 分辨率变化。

这是根据 Apple“不建议频繁改变输出尺寸”的约束提出的工程折中，不是已公开的钉钉内部实现。

## 能力边界

如果一个 Retina 窗口从 1600 × 1000 物理像素真正缩小到 800 × 500 物理像素，普通窗口捕获 API 只能得到后者的真实内容。接收端可以把 800 × 500 显示到更大的区域，但无法保持与 1600 × 1000 完全相同的细节。

若产品要求“窗口无论缩小到多小，远端仍保持缩小前的原生清晰度和信息量”，需要源应用继续以原尺寸离屏渲染、虚拟显示器，或应用级文档/画布共享，而不是普通像素窗口捕获。公开资料没有证明钉钉桌面客户端对任意应用窗口使用了这种机制。

## 事实与推断边界

- **已确认**：钉钉会议提供视频流畅度优先；DingRTC 支持窗口共享、源分辨率默认值、自动码率和独立渲染模式；Apple 说明 ScreenCaptureKit 固定输出、硬件缩放及元数据行为。
- **合理推断**：钉钉式体验更可能通过场景档位、源分辨率编码、内容/网络自适应以及接收端独立缩放组合实现。
- **无法确认**：钉钉桌面客户端是否使用特定超分算法、私有 WindowServer 接口、固定离屏画布，或具体 resize 阈值。

