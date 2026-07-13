# Tauri Plugin Screen Capture

面向 Tauri 2 桌面应用的屏幕与窗口捕获插件，提供共享源枚举、缩略图、系统权限、共享边框、采集会话控制、性能统计和本地 WebRTC H.264 发布。

插件由 Rust Core 和 JavaScript Guest bindings 两部分组成。默认发布链路完全位于应用进程内，不启动 FFmpeg 或其他编码器可执行文件。

## 功能

- 枚举显示器与应用窗口，可选返回 PNG Base64 缩略图。
- 过滤当前应用、系统 UI 和不适合共享的窗口，并支持调试查看过滤原因。
- 检查和请求系统屏幕录制权限。
- 启动、暂停、继续、停止独立捕获会话。
- 默认通过本地 WebRTC H.264 轨道发布，可直接连接到 WebView 的 `RTCPeerConnection`。
- Windows 使用 Windows Graphics Capture、D3D11 和 Media Foundation 硬件 H.264；硬件不可用时回退到 OpenH264。
- Windows GPU 路径支持零 CPU readback、按目标节拍续帧、关键帧恢复、动态 REMB 码率和等比例 letterbox。
- Windows 窗口共享边框跟随目标窗口层级；移动或调整大小时暂时隐藏，操作结束后重新计算。
- 支持注入自定义 Rust publisher，便于接入 Agora Native SDK 或其他实时传输方案。

## 支持平台

| 平台 | 状态 | 捕获实现 | H.264 实现 | 说明 |
| --- | --- | --- | --- | --- |
| Windows 10/11 | ✅ | Windows Graphics Capture | Media Foundation hardware MFT，OpenH264 fallback | 完整支持显示器、窗口、缩略图和共享边框 |
| macOS 14+ | ✅，需要 feature | ScreenCaptureKit | VideoToolbox | 启用 `macos-screencapturekit` feature |
| Linux | ❌ | 未实现 | 未实现 | `Capabilities` 标记为不支持；当前构建仍会暴露仅供测试的合成 dummy source，不得用于生产捕获 |
| Android | ❌ | 不支持 | 不支持 | 尚未实现移动端绑定 |
| iOS | ❌ | 不支持 | 不支持 | 尚未实现移动端绑定 |

本插件要求 Rust `1.77.2` 或更高版本，并面向 Tauri 2。

## 安装

插件需要同时安装 Rust crate 和 JavaScript bindings。

### 本地源码安装

在 Tauri 应用的 `src-tauri/Cargo.toml` 中添加 Rust 插件：

```toml
[dependencies]
tauri-plugin-screen-capture = { path = "../../path/to/tauri-plugin-screen-capture" }
```

macOS 构建需要启用 ScreenCaptureKit feature：

```toml
[dependencies]
tauri-plugin-screen-capture = {
  path = "../../path/to/tauri-plugin-screen-capture",
  features = ["macos-screencapturekit"]
}
```

使用项目所选的包管理器安装 JavaScript bindings：

```bash
pnpm add file:../../path/to/tauri-plugin-screen-capture
# 或
npm install ../../path/to/tauri-plugin-screen-capture
# 或
yarn add file:../../path/to/tauri-plugin-screen-capture
```

### 包发布后安装

Rust crate 和 npm 包发布后，可使用包名安装：

```bash
cargo add tauri-plugin-screen-capture
pnpm add tauri-plugin-screen-capture-api
```

当前仓库未配置公开 Git 仓库地址，因此本文档不提供 Git dependency 示例。

## 注册插件

在 Tauri 应用中注册 Rust 插件：

`src-tauri/src/lib.rs`

```rust
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_screen_capture::init())
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
```

默认 `init()` 创建本地 WebRTC publisher。如果需要接入自定义 Native SDK，可以使用 `tauri_plugin_screen_capture::Builder::new().publisher_factory(...).build()` 注入 `CapturePublisherFactory`。

## 配置权限

Tauri 2 默认通过 Capability 控制插件命令访问。将 `screen-capture:default` 添加到使用插件的 capability：

`src-tauri/capabilities/default.json`

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Default desktop capability",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "screen-capture:default"
  ]
}
```

`screen-capture:default` 包含以下权限：

| 权限 | JavaScript API |
| --- | --- |
| `screen-capture:allow-get-capabilities` | `getCapabilities()` |
| `screen-capture:allow-check-permission` | `checkPermission()` |
| `screen-capture:allow-request-permission` | `requestPermission()` |
| `screen-capture:allow-list-sources` | `listSources()` |
| `screen-capture:allow-start-capture` | `startCapture()` |
| `screen-capture:allow-pause-capture` | `pauseCapture()` |
| `screen-capture:allow-resume-capture` | `resumeCapture()` |
| `screen-capture:allow-stop-capture` | `stopCapture()` |
| `screen-capture:allow-get-capture-session` | `getCaptureSession()` |
| `screen-capture:allow-get-capture-stats` | `getCaptureStats()` |
| `screen-capture:allow-create-webrtc-offer` | `createWebRtcOffer()` |
| `screen-capture:allow-accept-webrtc-answer` | `acceptWebRtcAnswer()` |
| `screen-capture:allow-add-webrtc-ice-candidate` | `addWebRtcIceCandidate()` |

需要最小权限时，可以不使用 `screen-capture:default`，只声明业务实际调用的 `allow-*` 权限。

## 快速开始

### 获取权限和共享源

```ts
import {
  checkPermission,
  getCapabilities,
  listSources,
  requestPermission,
} from 'tauri-plugin-screen-capture-api'

const capabilities = await getCapabilities()
if (!capabilities.supportsDisplayCapture) {
  throw new Error(`Screen capture is unavailable on ${capabilities.platform}`)
}

let permission = await checkPermission()
if (permission !== 'granted') {
  permission = await requestPermission()
}
if (permission !== 'granted') {
  throw new Error(`Screen recording permission is ${permission}`)
}

const sources = await listSources({
  kinds: ['display', 'window'],
  includeThumbnails: true,
  includeCurrentApp: false,
  includeSystemUi: false,
})

const source = sources[0]
if (!source) throw new Error('No shareable source found')
```

缩略图位于 `source.thumbnailBase64`，显示时需要添加 Data URL 前缀：

```ts
const thumbnailUrl = source.thumbnailBase64
  ? `data:image/png;base64,${source.thumbnailBase64}`
  : null
```

### 启动捕获

```ts
import { startCapture } from 'tauri-plugin-screen-capture-api'

const session = await startCapture({
  sourceId: source.id,
  sourceKind: source.kind,
  width: 1920,
  height: 1080,
  fps: 60,
  captureCursor: true,
})
```

`StartCaptureOptions`：

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `sourceId` | `string` | 必填 | 使用当前 `listSources()` 返回的来源 ID |
| `sourceKind` | `'display' \| 'window'` | 必填 | 必须与所选来源类型一致 |
| `width` | `number` | `1280` | 输出画布宽度，自动归一为正偶数 |
| `height` | `number` | `720` | 输出画布高度，自动归一为正偶数 |
| `fps` | `number` | `30` | 目标帧率，限制在 `1..=60` |
| `captureCursor` | `boolean` | `true` | 是否捕获鼠标指针 |
| `publisher` | `PublisherOptions` | WebRTC loopback | 选择发布器；原生 Agora 尚未链接 |

输出画布尺寸在会话期间保持固定。Windows 窗口宽高比变化时，内容会等比例缩放并居中补黑边，不会拉伸；macOS 当前仅保证按请求的输出宽高采集，尚未声明相同的动态 letterbox 语义。

### 在 WebView 播放 WebRTC 视频

```ts
import {
  acceptWebRtcAnswer,
  addWebRtcIceCandidate,
  createWebRtcOffer,
} from 'tauri-plugin-screen-capture-api'

export async function connectCaptureVideo(sessionId: string, video: HTMLVideoElement) {
  const pc = new RTCPeerConnection()

  pc.ontrack = (event) => {
    const [stream] = event.streams
    video.srcObject = stream ?? new MediaStream([event.track])
    void video.play()
  }

  const offer = await createWebRtcOffer(sessionId)
  await pc.setRemoteDescription({ type: 'offer', sdp: offer.sdp })

  pc.onicecandidate = (event) => {
    if (!event.candidate) return
    void addWebRtcIceCandidate(sessionId, {
      candidate: event.candidate.candidate,
      sdpMid: event.candidate.sdpMid,
      sdpMlineIndex: event.candidate.sdpMLineIndex,
    })
  }

  const answer = await pc.createAnswer()
  await pc.setLocalDescription(answer)
  await acceptWebRtcAnswer(sessionId, {
    sdp: answer.sdp ?? '',
    sdpType: answer.type,
  })

  return pc
}
```

```ts
const peerConnection = await connectCaptureVideo(session.sessionId, videoElement)
```

### 会话控制和统计

```ts
import {
  getCaptureSession,
  getCaptureStats,
  pauseCapture,
  resumeCapture,
  stopCapture,
} from 'tauri-plugin-screen-capture-api'

await pauseCapture(session.sessionId)
await resumeCapture(session.sessionId)

const currentSession = await getCaptureSession(session.sessionId)
const stats = await getCaptureStats(session.sessionId)

console.log({
  status: currentSession.status,
  captureFps: stats.captureFps,
  publishFps: stats.publishFps,
  bitrateKbps: stats.bitrateKbps,
  encoderBackend: stats.encoderBackend,
  cpuReadback: stats.framesCpuReadback,
})

await stopCapture(session.sessionId)
peerConnection.close()
```

## JavaScript API

| API | 返回值 | 说明 |
| --- | --- | --- |
| `getCapabilities()` | `Promise<Capabilities>` | 获取当前平台的捕获和 WebRTC 能力 |
| `checkPermission()` | `Promise<PermissionStatus>` | 检查系统屏幕录制权限 |
| `requestPermission()` | `Promise<PermissionStatus>` | 请求系统屏幕录制权限 |
| `listSources(options?)` | `Promise<CaptureSource[]>` | 枚举显示器和窗口 |
| `startCapture(options)` | `Promise<CaptureSession>` | 创建并启动独立捕获会话 |
| `pauseCapture(sessionId)` | `Promise<void>` | 暂停捕获与发布 |
| `resumeCapture(sessionId)` | `Promise<void>` | 继续已暂停的会话 |
| `stopCapture(sessionId)` | `Promise<void>` | 停止并释放会话资源 |
| `getCaptureSession(sessionId)` | `Promise<CaptureSession>` | 获取会话状态和最近错误 |
| `getCaptureStats(sessionId)` | `Promise<CaptureStats>` | 获取捕获、发布、丢帧、码率和后端统计 |
| `createWebRtcOffer(sessionId)` | `Promise<WebRtcOffer>` | 获取原生 WebRTC peer 的 SDP offer |
| `acceptWebRtcAnswer(sessionId, answer)` | `Promise<void>` | 提交 WebView peer 的 SDP answer |
| `addWebRtcIceCandidate(sessionId, candidate)` | `Promise<void>` | 提交 WebView peer 的 ICE candidate |

## 捕获源选项

`listSources()` 默认枚举显示器和窗口，不生成缩略图，也不会返回当前应用或系统 UI。

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `kinds` | `['display', 'window']` | 需要枚举的来源类型 |
| `includeThumbnails` | `false` | 是否返回 PNG Base64 缩略图 |
| `includeCurrentApp` | `false` | 是否允许共享当前 Tauri 应用 |
| `includeSystemUi` | `false` | 是否包含任务栏、桌面外壳等系统窗口 |
| `debugRawSources` | `false` | 保留被过滤来源，并通过 `filteredReason` 说明原因 |

缩略图和窗口标题可能包含敏感信息。只应在用户明确打开共享选择器时获取，并在不再需要后及时释放前端引用。

## 性能统计

`CaptureStats` 提供以下关键指标：

| 字段 | 说明 |
| --- | --- |
| `framesCaptured` | 平台捕获后接收的帧数 |
| `framesPublished` | 已交给实时发布链路的帧数 |
| `framesDropped` | 聚合丢帧数 |
| `framesCaptureDropped` | 捕获阶段丢帧数 |
| `framesPipelineDropped` | latest-frame 管线替换的过期帧数 |
| `framesEncoderDropped` | 编码或编码样本交接阶段丢帧数 |
| `framesCpuReadback` | Windows GPU 路径发生 CPU readback 的次数；理想值为 `0` |
| `captureFps` | 平台实际捕获帧率 |
| `publishFps` / `fps` | 实际发布帧率 |
| `bitrateKbps` | 编码器当前应用的目标码率 |
| `encoderBackend` | 实际编码后端，例如 `media-foundation-d3d11-surface` |
| `started` | 发布器当前是否处于运行状态 |

静态窗口的 `captureFps` 可能很低，这是 Windows Graphics Capture 只提交变化帧的正常行为；应同时观察 `publishFps`，Windows 编码 worker 会按目标节拍续发最后一个 GPU surface。

## 手动清晰度档位

插件接收明确的输出宽高和帧率，不强制固定清晰度。示例 App 提供以下经过实机验证的档位：

| 档位 | 输出边界 | 目标帧率 |
| --- | --- | --- |
| 720p HD | 1280 × 720 | 60 FPS |
| 1080p Full HD | 1920 × 1080 | 60 FPS |
| 2K QHD | 2560 × 1440 | 60 FPS |
| 4K UHD | 3840 × 2160 | 30 FPS |

分辨率选择实现位于 [`examples/tauri-app/src/main.js`](examples/tauri-app/src/main.js)，完整决策见[手动共享清晰度档位方案](docs/media-solutions/2026-07-13-manual-capture-quality-presets-adopted.md)。

## Publisher

### WebRTC loopback

`startCapture()` 默认使用 `webRtcLoopback` publisher。Windows 优先使用 Media Foundation D3D11 surface 输入，此硬件路径不进行常规 BGRA CPU readback；硬件 MFT 不可用而回退到 OpenH264 时可能发生 GPU surface readback 和颜色转换。macOS 使用 VideoToolbox H.264。

### Agora

公共 API 已保留 `publisher.kind = 'agora'` 和 `AgoraPublisherOptions`，Rust 侧也提供 `AgoraPublisher`、`AgoraVideoSink` 与 `CapturePublisherFactory` 扩展边界。

仓库当前没有链接 Agora Native SDK。直接选择原生 Agora publisher 会在平台捕获开始前返回结构化 `publisherUnsupported` 错误。示例 App 中的 Agora Web SDK 发布属于示例层功能，不代表插件已经内置 Agora Native SDK。

## 错误

JavaScript API 拒绝时返回结构化错误：

```ts
interface CaptureErrorPayload {
  code: string
  message: string
  recoverable: boolean
  details?: unknown
}
```

| 错误码 | 说明 |
| --- | --- |
| `permissionDenied` | 用户拒绝屏幕录制权限 |
| `permissionNotDetermined` | 尚未完成系统权限选择 |
| `unsupportedPlatform` | 当前平台不支持捕获 |
| `sourceNotFound` | 指定共享源不存在 |
| `sourceUnavailable` | 共享源已关闭或暂时不可用 |
| `captureStartFailed` | 捕获会话启动失败 |
| `captureRuntimeFailed` | 运行中的平台捕获失败 |
| `publisherUnsupported` | 所选 publisher 未链接或不受支持 |
| `webrtcNegotiationFailed` | SDP 或 ICE 协商失败 |
| `webrtcTrackFailed` | H.264 轨道或编码发布失败 |
| `invalidSession` | 会话 ID 无效或状态不允许当前操作 |
| `internal` | 插件内部错误 |

`recoverable` 表示调用方是否可以通过重新选择来源、重试或重新创建会话恢复，不代表插件会自动重试所有操作。

## 平台说明

### Windows

- Windows 通过 `GraphicsCaptureSession::IsSupported()` 检查能力，不显示额外权限弹窗。
- 硬件 H.264 使用系统/驱动提供的 Media Foundation MFT，不依赖外部编码进程。
- 共享窗口移动或调整大小时，共享边框暂时隐藏并停止几何刷新；操作结束后恢复。
- 目标窗口失去焦点或被其他窗口遮挡时，共享边框与目标窗口保持相同层级，不覆盖无关窗口。

Windows 媒体实现见 [`src/platform/windows/media/README.md`](src/platform/windows/media/README.md)，独立方案和采纳记录位于 [`docs/media-solutions`](docs/media-solutions/)。

### macOS

- 必须启用 Cargo feature `macos-screencapturekit`，否则来源列表为空且启动捕获返回 `captureStartFailed`。
- 首次调用 `requestPermission()` 会触发系统屏幕录制授权。授权变化后通常需要重新启动应用才能被系统完整应用。
- 当前架构使用 ScreenCaptureKit 获取帧，并通过 VideoToolbox 发布 H.264。

## 示例

完整示例位于 [`examples/tauri-app`](examples/tauri-app)，包含：

- 显示器与窗口选择器；
- 共享源缩略图；
- 720p / 1080p / 2K / 4K 清晰度选择；
- 本地 WebRTC 视频预览；
- 捕获、发布、丢帧、码率、CPU readback 和编码后端统计；
- 可选 Agora Web SDK 外部视频轨道发布。

```bash
cd examples/tauri-app
npm install
npm run tauri dev
```

开发端口 `1420` 被占用时，请先停止已有示例进程，或者修改示例的 Vite/Tauri 开发端口配置。

## 开发

```bash
# 构建 JavaScript bindings
npm run build

# Rust 格式、静态检查和测试
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets

# 构建示例前端和 Tauri Rust 应用
npm --prefix examples/tauri-app run build
cargo check --manifest-path examples/tauri-app/src-tauri/Cargo.toml
```

Windows 交互桌面和硬件编码测试默认标记为 `ignored`，可以在支持 D3D11 和 Media Foundation hardware MFT 的 Windows 桌面上单独运行：

```bash
cargo test --test windows_capture_performance -- --ignored --nocapture --test-threads=1
cargo test --test windows_encoder_performance -- --ignored --nocapture --test-threads=1
```

## 贡献

提交变更前请确保 Rust、JavaScript bindings 和示例应用均构建通过。修改捕获节拍、GPU 处理、编码、WebRTC、共享边框或公开 API 时，应同步补充自动测试和独立 Markdown 方案文档。

## 许可证

当前仓库尚未在 `Cargo.toml`、`package.json` 或根目录许可证文件中声明许可证。正式发布前应补充明确的开源或商业许可证，并保持 Rust crate、npm 包和仓库声明一致。
