# macOS / Windows OBS 式平台原生 GPU 屏幕共享方案

> 方案状态：**Windows Phase 0/1 与 Phase 2 零读回核心链路已采纳（ADOPTED）；Display DXGI、device-lost 自动恢复、长期 soak 与 macOS 部分仍为 PROPOSED**
>
> 设计日期：2026-07-12
>
> 采纳标记：`ADOPTED-WINDOWS-NATIVE-MEDIA-PHASE-1`、`ADOPTED-WINDOWS-D3D11-GPU-SCALING-STAGING-RING`、`ADOPTED-WINDOWS-ZERO-READBACK-D3D11-MF-H264`。Windows 已落地阶段统计、编码前 latest-frame、编码后有序交接、WGC D3D11 BGRA→NV12 surface 与 Media Foundation hardware H264 零读回链路；这不等同于完整 Phase 2 验收，Display DXGI、device-lost/睡眠/热插拔自动恢复、60 秒持续运动和 8/24 小时 soak 完成前不得标记完整 Phase 2 ADOPTED。

## 1. 问题背景

该插件的长期架构必须同时支持 Windows 和 macOS。两个平台共享 session 生命周期、背压、编码样本、WebRTC transport、统计和验收契约，但 GPU surface、捕获 API、线程模型、硬件编码器与恢复机制必须采用各自平台的原生规范，不能用 Windows 的 D3D11 类型污染跨平台接口。

优化前 Windows 链路是：

```text
Windows Graphics Capture
  → GPU 纹理读回 CPU BGRA
  → capture callback 内整帧复制/CPU 缩放
  → Tokio 容量 1 通道
  → CapturePipeline latest-frame
  → WebRtcPublisher 内同步 H264 编码
  → WebRTC track
  → WebView2 <video>
```

现有 `VideoFrame` 只允许 `Arc<[u8]>` CPU 数据，因此捕获后立即失去 GPU surface；Windows 捕获、缩放、颜色转换和编码之间至少发生一次完整帧 CPU 往返。`WebRtcPublisher::push_frame` 又在 Tokio 异步任务内同步调用编码器，使编码耗时可能占住运行时线程。Windows capture callback 在容量为 1 的入口通道已满时直接跳过新帧，但这类跳过没有计入公开 `framesDropped`，示例 App 容易显示 `Dropped 0` 却只有 1–3 FPS。

2026-07-12 Windows 本机诊断数据：

| 路径 | 结果 |
| --- | ---: |
| 示例 App Debug | `Captured 6 / Published 5 / Dropped 0 / FPS 1.0` |
| 原生 2560×1440 纯捕获 | 58.55 FPS，P95 21.43 ms |
| Debug 2560×1440→1920×1080 捕获/缩放 | 1.25 FPS，P95 约 1055 ms |
| Release 2560×1440→1920×1080 捕获/缩放 | 28.49 FPS，P95 151.68 ms |
| Debug OpenH264 1080p 输入 | 2.84 FPS |
| Release OpenH264 1080p 输入 | 93.13 FPS |
| Release 捕获→缩放→OpenH264 | 23.87 FPS |

这些数据证明捕获 API 能达到接近 60 FPS；长期瓶颈是 CPU 帧模型、CPU 缩放/颜色转换、同步编码边界和不完整的丢帧观测，而不是 WebView2 单点渲染问题。

2026-07-13 Windows Phase 2 的零读回核心链路已落地：正常路径为 WGC D3D11 BGRA texture → D3D11 Video Processor NV12 surface → `MFCreateDXGISurfaceBuffer` → Media Foundation H264 MFT，`framesCpuReadback == 0`。具体采纳证据见独立方案 `2026-07-13-windows-zero-readback-d3d11-mf-h264-adopted.md`；编码后样本顺序约束见 `2026-07-13-windows-ordered-h264-sample-handoff-adopted.md`；高运动时的 transport 背压闭环见 `2026-07-13-windows-remb-low-latency-bitrate-control-adopted.md`。本段不宣称 Display DXGI、device-lost 恢复或长期 soak 已完成。

macOS 当前只完成代码路径审计，尚无本轮实体 Mac 性能数据；Phase 0 必须在 Apple Silicon/目标最低 macOS 版本上补齐同口径 baseline，未取得数据前不得宣称 macOS 已满足 1080p60。

## 2. 目标与范围

### 2.1 必须达到

- Windows 与 macOS 的屏幕和窗口共享在 Release 下稳定达到 1080p60；支持硬件允许时的 1440p60。
- 捕获、缩放、颜色转换和硬件编码全程保留在平台原生 GPU/media surface；Windows 使用 D3D11/DXGI surface，macOS 使用 `CMSampleBuffer`/`CVPixelBuffer`/IOSurface，正常路径不做整帧 CPU readback。
- 不启动外部编码 `.exe`；Windows 通过系统 COM/MFT 或 GPU 驱动动态接口，macOS 通过系统 ScreenCaptureKit/CoreVideo/VideoToolbox framework。
- 捕获 callback 不等待缩放、编码、WebRTC 或 UI。
- 队列只保留最新帧，任何时刻不累积过期视频。
- Windows 支持 AMD、NVIDIA、Intel，并有 Media Foundation 和 OpenH264 降级路径；macOS 使用 VideoToolbox 硬件编码并保留明确的软件/低规格降级策略。
- 处理 Windows GPU device-lost 与 macOS SCStream/VideoToolbox session failure，以及显示器插拔、睡眠恢复、分辨率/scale 变化和编码器重启。
- 保持现有 Tauri command 与 JavaScript API 兼容。
- Overlay/系统共享指示生命周期继续独立运行，不并入平台 media session。

### 2.2 本方案暂不处理

- 游戏进程注入式捕获。OBS Game Capture 使用注入获取游戏纹理，签名、兼容性和安全成本不适合当前通用插件。
- 第一阶段不替换 WebView2/WKWebView `<video>` 为原生 GPU 接收渲染。
- 第一阶段不引入 HEVC/AV1；WebRTC 仍以 H264 为兼容基线。
- 不要求 Windows 与 macOS 使用相同底层 GPU 类型或线程实现；只统一业务生命周期和媒体契约。

## 3. 事实、假设与待验证风险

### 3.1 已确认事实

- 当前机器主 GPU 是 AMD Radeon，同时存在 GameViewer、Oray、ToDesk 等虚拟显示适配器。
- DXGI Desktop Duplication 要求 D3D device 与目标输出属于同一 adapter；多适配器选错会失败。
- Windows Graphics Capture 更适合单窗口和现代组合窗口；DXGI Desktop Duplication 可直接提供桌面的 DXGI surface、dirty rect、move rect 和鼠标信息。
- 当前代码优先 AMF/QSV/NVENC，但依赖外部 FFmpeg 进程；没有 FFmpeg 时回退 OpenH264。
- macOS 已使用 ScreenCaptureKit 输出 `CMSampleBuffer`，配置 `queueDepth = 3`，并使用 VideoToolbox H264；但当前实现会锁定 `CVPixelBuffer`、复制成 CPU BGRA `VideoFrame`，编码时再次用字节数组创建 `CVPixelBuffer`，破坏了原生 surface 链路。
- macOS 当前每提交一帧都调用 `VTCompressionSessionCompleteFrames`，并且 `force_keyframe` 尚未实现，无法满足异步流水和 WebRTC PLI/FIR 恢复要求。

### 3.2 设计假设

- AMD 驱动在当前机器提供可动态加载的 AMF Runtime，并支持 H264 低延迟编码。
- WebView2/WebRTC 接收端能对兼容的 H264 Baseline/Main 流选择硬件解码；是否实际命中硬件需要单独观测。
- macOS VideoToolbox 能在目标 Apple Silicon/Intel Mac 上选择硬件 H264 encoder；必须通过 encoder specification 和运行时属性确认实际选择，而不是仅凭 session 创建成功推断。
- ScreenCaptureKit 能输出 VideoToolbox 可直接消费的 `CVPixelBuffer` 格式；优先验证 NV12/bi-planar 输出，无法直接输出时才进入 Metal/Core Image/VideoToolbox pixel transfer 路径。
- `WDA_EXCLUDEFROMCAPTURE` 对最终选择的屏幕捕获后端能保持 Overlay 不进入共享画面；DXGI 路径必须实机验证。

### 3.3 未解决风险

- 虚拟显示适配器可能不提供硬件编码器，捕获 adapter 与编码 adapter 之间可能需要 GPU 跨适配器复制。
- WGC、DXGI 和 AMF 对 HDR、旋转屏、混合显卡的 surface 格式与同步要求不同。
- 编码器运行时切换虽保持 H264 codec，但必须重新发送 SPS/PPS 并强制 IDR；分辨率变化还可能需要 WebRTC 重新协商。
- 当前基准依赖桌面是否持续变化，必须新增确定性 GPU 动画源，避免静态桌面造成假低 FPS。
- ScreenCaptureKit wrapper 的像素格式能力、IOSurface backing 和回调队列语义必须在实际 macOS 14+ 环境验证；Windows 主机无法完成这部分运行测试。
- Intel Mac 与 Apple Silicon 的 VideoToolbox 能力、功耗和 1440p60 上限不同，不能以单一机器结果代表全部 macOS 设备。

## 4. 目标架构

```text
Tauri commands / ScreenCaptureState
                │
                ▼
       CaptureSessionEngineFactory
                │
      ┌──────────┼──────────┐
      │          │          │
WindowsGpuSession│  MacOsMediaSession
      │          │          │
      │   ScreenCaptureKit  │
      │    CMSampleBuffer   │
      │     CVPixelBuffer   │
      │    VideoToolbox     │
      │          │          │
      └──────────┼──────────┘
                 │
          GenericCpuSession
            （最终回退）

WindowsGpuSession:
Windows Media Thread（独占 D3D11 device/context）
      │
      ├─ Display: DXGI Desktop Duplication
      ├─ Window: Windows Graphics Capture
      ├─ Adapter/LUID 选择与 device-lost 恢复
      ├─ 3-slot GPU texture pool / latest-frame scheduler
      ├─ D3D11 Video Processor：缩放、裁剪、BGRA→NV12
      └─ HardwareEncoder
           ├─ AMD AMF
           ├─ NVIDIA NVENC
           ├─ Intel oneVPL/QSV
           ├─ Media Foundation Hardware MFT
           └─ OpenH264 CPU fallback
                │
                ▼
       EncodedVideoSampleSink
                │
                ▼
       WebRTC H264 track / Agora adapter
```

核心原则是把平台的捕获、GPU/media surface 处理和编码合并到有明确所有权的 platform media session 内。跨平台层只看生命周期、编码配置、H264 sample 和统计；绝不把 `ID3D11Texture2D`、`CVPixelBuffer` 或 `CMSampleBuffer` 暴露进公共 API，也不要求它们通过同一个 `VideoFrame` 在 Tokio 任务间传递。

### 4.1 平台映射

| 统一职责 | Windows 规范 | macOS 规范 |
| --- | --- | --- |
| Session engine | `WindowsGpuSession` | `MacOsMediaSession` |
| 屏幕捕获 | DXGI Desktop Duplication，WGC fallback | ScreenCaptureKit display filter |
| 窗口捕获 | Windows Graphics Capture | ScreenCaptureKit window filter |
| 原生 surface | `ID3D11Texture2D` | `CMSampleBuffer` + IOSurface-backed `CVPixelBuffer` |
| 缩放/颜色转换 | D3D11 Video Processor / shader | ScreenCaptureKit 输出配置优先；Metal/Core Image/VT pixel transfer fallback |
| 硬件 H264 | AMF/NVENC/oneVPL/MF MFT | VideoToolbox `VTCompressionSession` |
| 编码输出 | H264 Annex-B access unit | VideoToolbox AVCC → Annex-B access unit |
| 最终软件回退 | OpenH264 | VideoToolbox software encoder或受控降规格 |

## 5. 模块边界

建议目录：

```text
src/
  session/
    engine.rs                 # CaptureSessionEngine 接口与 generic adapter
    generic_cpu.rs            # 包装现有 CaptureBackend + CapturePipeline
  platform/windows/media/
    mod.rs
    session.rs                # WindowsGpuSession 生命周期/状态机
    thread.rs                 # Windows Media Thread、命令队列、shutdown
    adapter.rs                # monitor/window → DXGI adapter LUID
    capture/
      mod.rs                  # WindowsFrameSource 接口
      desktop_duplication.rs  # Display 主路径
      graphics_capture.rs     # Window 主路径、Display fallback
    gpu/
      device.rs               # D3D11 device/context/device-lost
      texture_pool.rs         # 固定 3-slot surface pool
      processor.rs            # crop/scale/BGRA→NV12
      scheduler.rs            # latest-frame + 动态/静态帧节奏
    encoder/
      mod.rs                  # HardwareEncoder 与选择策略
      amf.rs                  # AMD 动态接口
      nvenc.rs                # 后续 NVIDIA 动态接口
      onevpl.rs               # 后续 Intel 动态接口
      media_foundation.rs     # 跨厂商 Hardware MFT fallback
      software.rs             # 适配现有 OpenH264
    telemetry.rs              # 分阶段计数与延迟直方图
  platform/macos/media/
    mod.rs
    session.rs                # MacOsMediaSession 生命周期
    capture.rs                # ScreenCaptureKit SCStream/filter/config
    surface.rs                # CMSampleBuffer/CVPixelBuffer lease
    processor.rs              # NV12 直通；Metal/Core Image/VT transfer fallback
    encoder.rs                # VideoToolbox VTCompressionSession
    telemetry.rs              # 与统一 stage 名称映射
  transport/
    encoded_sink.rs           # EncodedVideoSampleSink
    webrtc.rs                 # 从现有 WebRtcPublisher 拆出的传输职责
```

### 5.1 `ScreenCaptureState`

- 继续作为 session 权威状态和 Tauri command 入口。
- 不再直接组合 `CaptureBackend + CapturePipeline + CapturePublisher`，改为请求 `CaptureSessionEngineFactory` 创建运行引擎。
- Overlay、WebRTC signaling 和 session map 保持现有职责。

### 5.2 `WindowsGpuSession`

- 拥有捕获源、D3D11 device、texture pool、processor、encoder 和 telemetry。
- 只通过命令通道接收 start/pause/resume/stop/request-keyframe/reconfigure。
- 所有 COM/GPU 资源在 Windows Media Thread 创建并销毁，避免 Tokio worker 阻塞与线程归属不明确。

### 5.3 `MacOsMediaSession`

- 拥有 `SCStream`、输出回调、CVPixelBuffer latest-frame lease、可选 Metal/PixelTransfer processor、`VTCompressionSession` 和 telemetry。
- ScreenCaptureKit callback 只验证 frame status、保留 sample/pixel buffer 并替换 latest slot；不锁 base address、不复制整帧 BGRA、不等待编码输出。
- VideoToolbox 编码在专用串行 media queue 提交，压缩输出通过 callback 异步返回；只在 pause/stop/drain 时调用 `VTCompressionSessionCompleteFrames`。
- Apple framework 对象在明确的 queue/session 生命周期内创建和销毁；Tokio 只负责命令、signaling、stats 和 encoded sample transport。

### 5.4 捕获后端

- Display 默认 DXGI Desktop Duplication；若 adapter 不匹配、权限/会话状态或虚拟显示器不支持，则切换 WGC。
- Window 默认 WGC；PrintWindow/GDI 只保留给缩略图，不进入实时共享路径。
- 捕获回调只取得/复制 GPU texture 到空闲池槽并发布 slot generation，不执行 readback、缩放或编码。
- macOS display/window 都使用 ScreenCaptureKit filter；`minimumFrameInterval = 1/fps`、`queueDepth = 3`，输出目标尺寸和像素格式在 `SCStreamConfiguration` 配置，callback 直接保留 `CMSampleBuffer`/`CVPixelBuffer`。

### 5.5 GPU/media processor

- 输入为捕获格式 `DXGI_FORMAT_B8G8R8A8_UNORM` 或 WGC surface。
- 优先使用 D3D11 Video Processor 完成裁剪、旋转、缩放和 NV12 转换；驱动不支持目标组合时使用预编译 compute/pixel shader fallback，仍不回读 CPU。
- 输出 texture 尺寸始终为偶数，编码器只接收 NV12 GPU surface。
- 处理 surface 池预分配，正常运行不逐帧创建 texture 或 heap buffer。
- macOS 优先让 ScreenCaptureKit 直接产生目标尺寸的 NV12/VideoToolbox-compatible pixel buffer；需要额外裁剪、合成或格式转换时使用 IOSurface/Metal texture 或 VideoToolbox pixel transfer，禁止 CPU BGRA 中转。

### 5.6 编码器

- 统一接收 NV12 D3D11 surface，输出 H264 access unit、时间戳、关键帧标记和 codec config。
- 当前 AMD 机器优先实现直接 AMF；随后实现 Media Foundation Hardware MFT 通用后端。
- NVIDIA/Intel 后端共用相同接口，通过 adapter vendor ID 选择。
- OpenH264 只在所有硬件路径失败时启用，并自动降低到可持续的分辨率/FPS。
- macOS 使用 VideoToolbox H264：创建 session 时允许并优先硬件 encoder，设置 RealTime、ExpectedFrameRate、MaxFrameDelayCount、禁止 frame reordering、码率/GOP，并用 frame properties 实现强制关键帧。

### 5.7 WebRTC transport

- 从 `WebRtcPublisher` 移除编码器所有权，仅接收已经编码的 `EncodedVideoSample`。
- transport 写入慢时只影响编码输出队列；不会反向阻塞 capture callback。
- PLI/FIR 转换成 `request_keyframe` 命令发送给 media session。

### 5.8 macOS 平台规范细节

#### 捕获与 surface 所有权

- 使用 ScreenCaptureKit `SCStream` 统一捕获 display/window，保持现有 source id 与 permission API。
- `SCStreamConfiguration.minimumFrameInterval` 设为目标 FPS 的倒数，`queueDepth` 默认 3；队列深度只用于吸收系统调度抖动，不允许在应用层继续累积。
- 只接受 `SCFrameStatus.complete`/有有效 image buffer 的 screen sample；idle/blank/stopped 状态分别计数。
- callback 保留 `CMSampleBuffer` 或其中的 `CVPixelBuffer`，发布到容量 1 的 latest slot；下一阶段消费后立即释放引用。
- 禁止正常路径调用 `CVPixelBufferLockBaseAddress` 读取并复制整帧；缩略图、诊断 dump 和软件 fallback 才允许显式 readback。

#### 像素格式与 GPU 处理

- 首选让 ScreenCaptureKit 按目标尺寸直接输出 VideoToolbox 支持的 bi-planar NV12 pixel buffer，保留 IOSurface backing。
- 若 OS/wrapper/目标组合只提供 BGRA，则通过 `CVMetalTextureCache` 将 IOSurface-backed pixel buffer 映射为 Metal texture，再写入 `CVPixelBufferPool` 的 NV12 buffer；也可评估 VideoToolbox pixel transfer，按基准选择。
- Retina points 与 backing pixels 必须明确区分；输出编码尺寸使用物理像素并保持偶数，窗口移动不应触发不必要的 encoder 重建。
- 色彩元数据至少传递/规范化 range、matrix、primaries、transfer function；SDR 首版统一 BT.709 limited range，HDR 作为独立后续方案。

#### VideoToolbox 编码

- `VTCompressionSessionCreate` 的 encoder specification 设置 `EnableHardwareAcceleratedVideoEncoder = true`；发布构建可提供“必须硬件”策略，硬件不可用时进入明确降级而不是静默卡顿。
- 配置 `RealTime = true`、`ExpectedFrameRate`、`MaxFrameDelayCount = 1`、`AllowFrameReordering = false`、平均/数据率限制、GOP 与 H264 profile。
- 直接把原始/处理后的 `CVPixelBuffer` 传给 `VTCompressionSessionEncodeFrame`；禁止先转 `VideoFrame<Vec<u8>>` 再创建 pixel buffer。
- 编码输出完全由 `VTCompressionOutputCallback` 异步交付；每帧提交后不得调用 `VTCompressionSessionCompleteFrames`，只在 pause/stop/reconfigure/drain 调用。
- WebRTC PLI/FIR 使用 `kVTEncodeFrameOptionKey_ForceKeyFrame`；关键帧或 codec config 变化时输出 SPS/PPS，并把 AVCC access unit 转为现有 transport 使用的 Annex-B。
- session 实际 encoder ID、是否硬件、平均 encode latency 和 delayed-frame count 必须进入 telemetry。

#### 生命周期与恢复

- `MacOsMediaSession` 用串行 media command queue 管理 `SCStream` 与 `VTCompressionSession` 的 start/pause/resume/reconfigure/stop 顺序；callback 不直接操作 Tokio runtime 状态。
- SCStream error/source closed、显示器热插拔、scale/分辨率变化、睡眠恢复触发受控重建；同一后端最多重试一次，再进入降规格或 session error。
- reconfigure 时停止接收新 sample、drain encoder、交换新 session、强制首帧 IDR；旧 callback 带 generation，过期输出直接丢弃。
- macOS 使用系统屏幕录制权限与系统共享指示；自定义 Overlay 继续作为独立可选模块，不与 media session 耦合。

#### macOS 降级顺序

```text
ScreenCaptureKit NV12 → VideoToolbox hardware H264
        │不支持目标格式
        ▼
ScreenCaptureKit BGRA → Metal/VT pixel transfer → VideoToolbox hardware H264
        │硬件编码不可用或持续失败
        ▼
VideoToolbox software H264 + 自动降低 resolution/FPS
        │仍不可持续
        ▼
停止 session，返回可重试错误
```

## 6. 内部接口草案

```rust
#[async_trait]
trait CaptureSessionEngine: Send + Sync {
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn request_keyframe(&self) -> Result<()>;
    async fn stats(&self) -> Result<ExtendedCaptureStats>;
}

trait EncodedVideoSampleSink: Send + Sync {
    fn try_push(&self, sample: EncodedVideoSample) -> PushResult;
}

trait WindowsFrameSource {
    fn start(&mut self, config: &CaptureConfig) -> Result<()>;
    fn acquire_latest(&mut self) -> Result<Option<GpuFrameLease>>;
    fn stop(&mut self);
}

trait HardwareEncoder {
    fn backend(&self) -> EncoderBackendInfo;
    fn submit(&mut self, frame: ProcessedGpuFrame) -> Result<SubmitResult>;
    fn drain(&mut self, sink: &dyn EncodedVideoSampleSink) -> Result<()>;
    fn request_keyframe(&mut self) -> Result<()>;
    fn reconfigure(&mut self, config: EncoderConfig) -> Result<()>;
}
```

这些接口只放在 crate 内部。现有 JavaScript/Tauri API 暂不增加必须字段，避免一次迁移同时破坏前端和平台后端。

## 7. 帧调度与背压

- 跨平台规则一致：只处理最新帧、所有队列有固定上限、捕获 callback 永不等待 encoder/transport；平台按原生 surface 实现。
- Windows GPU texture pool 固定 3 个槽：capturing、processing/encoding、free。
- 每个槽状态为 `Free / Ready / InUse` 并带 generation；新帧到达但无 `Free` 时只能回收最旧的 `Ready`，绝不覆盖 encoder 正在使用的 `InUse`。若没有可安全回收的槽则丢弃新帧，并记录 `captureBackpressureDrops`。
- macOS 使用容量 1 的 retained `CMSampleBuffer`/`CVPixelBuffer` latest slot；新 sample 替换尚未提交给 VideoToolbox 的旧 sample，已提交的 pixel buffer 生命周期由 VideoToolbox callback/session 管理。
- 动态画面按目标 60/30 FPS 处理最新帧；生产速度高于目标时合并旧帧。
- 静态桌面不必重复编码 60 张相同帧，可降到 1–2 FPS keepalive；一旦 dirty rect、鼠标或窗口内容变化，下一目标 tick 立即恢复满帧率。
- 任何队列都必须有固定上限；禁止无界积压。
- capture callback、GPU processor、encoder、transport 分别记录 queue age，避免只看最终 FPS。

## 8. 后端选择与降级状态机

```text
Display + vendor encoder
  DXGI + AMF/NVENC/oneVPL
        │失败
        ▼
  WGC + 同厂商 encoder
        │失败
        ▼
  WGC + Media Foundation Hardware MFT
        │失败
        ▼
  WGC + OpenH264（自动降级分辨率/FPS）
```

macOS 使用 5.8 节定义的 `ScreenCaptureKit → VideoToolbox hardware → VideoToolbox software/降规格` 状态机；Windows 和 macOS 都向上层报告同一组 backend、fallback、drop 和 recovery reason。

规则：

- 编码 adapter 优先与捕获 adapter LUID 相同；跨 adapter 复制必须显式记录并视为降级。
- 硬件 encoder 启动前运行短探针，但不得启动外部进程。
- 运行时 encoder 错误先重建同一后端一次；再次失败才进入下一后端。
- 后端切换后发送 SPS/PPS 和 IDR，并清空旧 access unit。
- GPU device-lost 统一重建 capture、processor、encoder；目标恢复时间 ≤2 秒。
- macOS 的 SCStream/VTCompressionSession failure 与 Windows device-lost 使用相同恢复预算和 session 状态语义。
- 10 秒内连续三次恢复失败后停止 session 并返回可重试错误，禁止无限重启。

## 9. 可观测性

保留兼容字段，同时新增扩展指标：

```text
captureFrames
captureFps
captureBackpressureDrops
gpuProcessFrames
gpuProcessP50/P95Ms
encodeSubmittedFrames
encodeOutputFrames
encodeDrops
encodeP50/P95Ms
transportFrames
transportQueueDrops
renderedFrames（示例 App requestVideoFrameCallback）
renderFps
activeCaptureBackend
activeEncoderBackend
adapterLuid / adapterName
deviceResetCount
fallbackCount
```

旧 `framesDropped` 改为所有 stage drop 的总和，扩展接口保留每一阶段的独立原因。日志仅记录生命周期、后端切换、恢复和周期性汇总，不逐帧输出。

## 10. 性能与可靠性验收

### 10.1 自动性能基线

- 使用确定性的 D3D11（Windows）和 Metal/CoreAnimation（macOS）动画测试窗口或测试纹理，不能依赖桌面是否静止。
- Windows 与 macOS 1080p60：连续运动 60 秒，capture 和 encode output 均 ≥55 FPS。
- 1440p60：支持的 AMD/NVIDIA/Intel 与 Apple Silicon 设备均 ≥55 FPS；不支持的 Intel Mac/旧 GPU 必须显式降级而不是卡顿。
- capture→encoded P95 ≤25 ms，P99 ≤40 ms。
- latest-frame queue age P95 ≤33 ms。
- 连续运动时总 drop <1%；静态帧合并单独统计，不算异常 drop。
- 首帧可发送时间 ≤500 ms。
- Release 构建 CPU 占用目标：1080p60 ≤15%（记录测试 CPU 型号，不作为跨机器绝对门槛）。

### 10.2 稳定性矩阵

- 8 小时每次提交 smoke；发布候选执行 24 小时 soak。
- 窗口移动/缩放、最小化、遮挡、切换焦点、关闭源。
- 显示器插拔、分辨率/DPI/旋转/HDR 切换。
- Windows 锁屏/解锁、睡眠/恢复、快速用户切换。
- GPU 驱动重置/device removed。
- 实体 AMD 与当前多个虚拟显示适配器组合。
- Apple Silicon 至少覆盖 M 系列两代；仍在支持范围内时覆盖一台 Intel Mac。
- macOS 屏幕录制权限首次授权/撤销、SCStream source closed、Retina scale 变化、显示器空间切换和睡眠恢复。
- 验证 VideoToolbox 实际 encoder ID/硬件标志，不能只验证 H264 有输出。
- 网络限速、突发丢包、WebRTC PLI/FIR、接收端重连。
- 运行 24 小时 GPU/CPU 内存无持续增长；线程和句柄数量回到会话前基线。

### 10.3 示例 App 验收

- 同时显示 Capture、Encode、Transport、Render FPS 与每段 drop。
- 使用 `HTMLVideoElement.requestVideoFrameCallback` 统计真实渲染帧，不以 stats polling 推断画面流畅度。
- 提供静态桌面、滚动文字、60 FPS 动画、窗口拖动四个测试模式。

## 11. 分阶段迁移计划

### Phase 0：确定性反馈环与指标修正

- 新增 D3D/Win32 与 Metal/CoreAnimation 动画测试源，修复静态桌面导致的双平台性能基准不确定性。
- 将 Windows 入口 `capacity == 0` 和 macOS latest dispatcher replacement/SCFrameStatus skip 计入分阶段统计。
- 拆分 capture/encode/transport/render FPS。
- 记录 active encoder、active capture backend、adapter。
- 完成条件：同一命令连续 10 次误差 ≤5%，能够稳定复现当前 CPU 路径瓶颈。

### Phase 1：跨平台 session/transport 边界

- 引入 `CaptureSessionEngine` 与 `EncodedVideoSampleSink`，`WebRtcPublisher` 拆分为 platform encoder/session 与 transport。
- Windows 建立独立编码 worker；macOS 建立串行 media command queue 与异步 VideoToolbox callback 边界。
- 暂时保持 CPU BGRA fallback，先验证双平台生命周期、线程和背压契约。
- 完成条件：Debug 慢编码不会阻塞 capture worker；所有跳帧都有阶段归属。

### Phase 2A：Windows GPU media session 骨架

- 建立 D3D11 device、adapter/LUID 选择、texture pool 和 media thread。
- Display 实现 DXGI，Window 适配 WGC GPU surface。
- 完成 GPU resize/BGRA→NV12；暂用 readback 与现有 OpenH264 对照验证像素正确性。
- 完成条件：GPU processor 1080p60、无逐帧分配、device-lost 可恢复。

### Phase 2B：macOS 零拷贝 media session

- `SCStream` sample 保持 `CMSampleBuffer`/`CVPixelBuffer`，删除正常路径的 BGRA Vec copy。
- 验证 ScreenCaptureKit 直接 NV12；必要时实现 IOSurface→Metal/VT pixel transfer→NV12。
- 重构 VideoToolbox 为异步提交/回调，不逐帧 `CompleteFrames`，实现 force-keyframe 与 encoder telemetry。
- 完成条件：Apple Silicon 1080p60、无逐帧整帧 CPU copy、VideoToolbox 硬件标志可验证、睡眠/显示器变化可恢复。

### Phase 3：Windows AMD AMF 与 macOS VideoToolbox 验收

- 直接加载 AMD driver AMF Runtime，输入 NV12 D3D11 surface。
- 实现低延迟 CBR、无 B 帧、GOP、IDR、SPS/PPS、动态码率。
- 同时完成 macOS VideoToolbox RealTime、ExpectedFrameRate、MaxFrameDelayCount、无重排、码率/GOP 和 IDR 验收。
- 完成条件：当前 AMD 机器与 Apple Silicon Mac 的 1080p60/支持时 1440p60 通过性能、画质和 8 小时 soak。

### Phase 4：跨厂商与通用降级

- 实现 Media Foundation Hardware MFT。
- 按测试硬件加入 NVENC、oneVPL/QSV 动态后端。
- 建立 vendor/device capability cache 与 fallback state machine。
- 完善 macOS VideoToolbox software/降规格 fallback 与 encoder capability cache。
- 完成条件：AMD/NVIDIA/Intel 各至少一台 Windows 实体设备，以及 Apple Silicon/支持范围内 Intel Mac，通过 1080p60 和恢复矩阵。

### Phase 5：自适应与长期运行

- 基于 encode latency、queue age、WebRTC feedback 调节 bitrate/FPS/resolution。
- 完成 24 小时 soak、睡眠恢复、显示器热插拔和设备重置。
- 根据数据决定是否需要 Windows Media Foundation/DXVA 或 macOS VideoToolbox/Metal 原生接收渲染；默认继续使用 WebView2/WKWebView。

## 12. 决策门

在把本方案标记为 ADOPTED 前必须确认：

1. 是否接受 Windows 快速路径绕过现有通用 CPU `CapturePipeline`，由 `CaptureSessionEngine` 保留统一生命周期。
2. 是否接受 macOS 同样绕过 CPU `VideoFrame`，由 `MacOsMediaSession` 直接连接 ScreenCaptureKit surface 与 VideoToolbox。
3. Windows 第一硬件后端是否确定为 AMD AMF、Media Foundation 为第二后端；macOS 确定为 VideoToolbox hardware、software/降规格 fallback。
4. 第一版是否优先保证 Windows/macOS WebRTC，Agora 继续走 CPU fallback。
5. 首批验收目标是否为双平台 1080p60 必须、1440p60 在支持硬件上必须通过并允许旧设备显式降级。
6. 是否允许新增扩展 stats API，同时保持旧 `CaptureStats` 兼容。

## 13. 参考来源

- OBS 官方：[Hardware Encoding](https://obsproject.com/kb/hardware-encoding)。OBS 推荐 NVENC、AMF、QSV 等 GPU 专用编码器，以降低 CPU 和渲染影响。
- OBS 官方仓库讨论：[Why OBS can capture at any FPS?](https://github.com/obsproject/obs-studio/discussions/11486)。OBS 维护者说明 Display Capture 使用 DXGI Desktop Duplication，Game Capture 使用注入直接取得进程图形帧。
- Microsoft Learn：[Desktop Duplication API](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api)。桌面帧以 DXGI surface 提供，并包含 dirty/move rect 与鼠标元数据，可继续使用 GPU 处理。
- Microsoft Learn：[IDXGIOutput1::DuplicateOutput](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutput1-duplicateoutput)。D3D device 必须从目标输出所属 adapter 创建；多显示器需要分别处理。
- Microsoft Learn：[Windows.Graphics.Capture](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)。现代 Windows 屏幕/窗口捕获与 frame pool 生命周期说明。
- Microsoft Learn：[MFCreateDXGISurfaceBuffer](https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mfcreatedxgisurfacebuffer)。可把 `ID3D11Texture2D`/DXGI surface 包装为 Media Foundation buffer，避免 CPU copy。
- Microsoft Learn：[MFTEnumEx](https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mftenumex)。枚举并激活硬件视频编码 MFT。
- Microsoft Learn：[Hardware MFTs](https://learn.microsoft.com/en-us/windows/win32/medfound/hardware-mfts)。硬件 encoder、decoder 和 video processor 的异步模型与硬件内数据交换。
- Microsoft Learn：[CODECAPI_AVLowLatencyMode](https://learn.microsoft.com/en-us/windows/win32/medfound/codecapi-avlowlatencymode)。实时通信编码/解码的低延迟模式。
- Apple Developer：[ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit)。高性能屏幕/窗口捕获，以 `CMSampleBuffer` 和关联元数据交付媒体。
- Apple Developer：[SCStreamConfiguration.minimumFrameInterval](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/minimumframeinterval)。目标帧率使用 `1/fps`，并配合有界 `queueDepth`。
- Apple Developer：[VideoToolbox](https://developer.apple.com/documentation/videotoolbox)。直接访问硬件视频编码/解码以及 CoreVideo pixel buffer 处理。
- Apple Developer：[VTCompressionSession](https://developer.apple.com/documentation/videotoolbox/vtcompressionsession-api-collection)。异步提交 `CVImageBuffer` 并从 compression callback 接收输出。
- Apple Developer：[EnableHardwareAcceleratedVideoEncoder](https://developer.apple.com/documentation/videotoolbox/kvtvideoencoderspecification_enablehardwareacceleratedvideoencoder)。允许 VideoToolbox 为实时编码选择硬件路径。
- Apple Developer：[RealTime compression property](https://developer.apple.com/documentation/videotoolbox/kvtcompressionpropertykey_realtime)。实时编码及最大帧延迟相关属性。

## 14. 与既有方案的关系

本方案建立在已经采纳的事件驱动 latest-frame、SIMD CPU fallback、OpenH264 fallback、macOS ScreenCaptureKit/VideoToolbox 和 Windows Overlay 生命周期方案之上，不修改它们的历史结论。CPU 路径保留为回退与对照，但不再作为 Windows 或 macOS 长期高性能主路径。

本文件只描述“OBS 式全 GPU media pipeline”这一种候选方案。其他新候选方案如被正式评估，必须各自建立独立 Markdown，并单独标记 PROPOSED、ADOPTED 或 REJECTED。
