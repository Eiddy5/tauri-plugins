import "./style.css"
import {
  checkPermission,
  getCaptureStats,
  listSources,
  onCaptureSessionEnded,
  pauseCapture,
  requestPermission,
  resumeCapture,
  startCapture,
  stopCapture,
} from "tauri-plugin-screen-capture-api"
import { publishAgoraScreenTrack } from "./lib/agoraPublisher.js"
import { connectVideo } from "./lib/screenCapture.js"

const captureQualityPresets = {
  "720p": { label: "720p HD", maxWidth: 1280, maxHeight: 720, fps: 60 },
  "1080p": { label: "1080p Full HD（推荐）", maxWidth: 1920, maxHeight: 1080, fps: 60 },
  "2k": { label: "2K QHD", maxWidth: 2560, maxHeight: 1440, fps: 60 },
  "4k": { label: "4K UHD", maxWidth: 3840, maxHeight: 2160, fps: 30 },
}
const storedCaptureQuality = localStorage.getItem("screenCapture.quality")
const defaultCaptureQuality = Object.hasOwn(captureQualityPresets, storedCaptureQuality)
  ? storedCaptureQuality
  : "1080p"
const state = {
  sources: [],
  selected: null,
  session: null,
  stats: null,
  error: "",
  loading: false,
  debugRawSources: false,
  includeCurrentApp: false,
  includeSystemUi: false,
  captureQuality: defaultCaptureQuality,
  activeKind: "display",
  peerConnection: null,
  captureVideoTrack: null,
  agoraPublication: null,
  agoraEnabled: false,
  agoraPublishing: false,
  agoraAppId: localStorage.getItem("screenCapture.agora.appId") ?? "",
  agoraChannel: localStorage.getItem("screenCapture.agora.channel") ?? "screen-share-demo",
  agoraToken: localStorage.getItem("screenCapture.agora.token") ?? "",
  agoraUid: localStorage.getItem("screenCapture.agora.uid") ?? "",
  pollTimer: null,
}

const app = document.querySelector("#app")

app.innerHTML = `
  <main class="app-shell">
    <aside class="sidebar">
      <header class="sidebar-header">
        <div>
          <h1>选择共享内容</h1>
          <p data-source-summary>0 个可共享来源</p>
        </div>
        <button type="button" class="refresh-button" data-refresh>刷新</button>
      </header>

      <div class="source-tabs" role="tablist" aria-label="共享类型">
        <button type="button" role="tab" data-kind="display">共享屏幕 <span data-display-count>0</span></button>
        <button type="button" role="tab" data-kind="window">共享窗口 <span data-window-count>0</span></button>
      </div>

      <label class="quality-control">
        <span>共享清晰度</span>
        <select data-quality aria-label="共享清晰度">
          ${Object.entries(captureQualityPresets)
            .map(
              ([value, preset]) =>
                `<option value="${value}">${preset.label} · ${preset.fps} FPS</option>`,
            )
            .join("")}
        </select>
      </label>

      <div class="options">
        <label><input type="checkbox" data-option="debugRawSources" /> Debug raw</label>
        <label><input type="checkbox" data-option="includeCurrentApp" /> Current app</label>
        <label><input type="checkbox" data-option="includeSystemUi" /> System UI</label>
      </div>

      <section class="agora-panel">
        <label class="agora-toggle">
          <input type="checkbox" data-agora-enabled />
          <span>发布到 Agora</span>
        </label>
        <label>
          <span>App ID</span>
          <input type="text" data-agora-field="agoraAppId" autocomplete="off" spellcheck="false" />
        </label>
        <label>
          <span>Channel</span>
          <input type="text" data-agora-field="agoraChannel" autocomplete="off" spellcheck="false" />
        </label>
        <label>
          <span>Token</span>
          <input type="text" data-agora-field="agoraToken" autocomplete="off" spellcheck="false" />
        </label>
        <label>
          <span>UID</span>
          <input type="number" min="0" step="1" data-agora-field="agoraUid" />
        </label>
      </section>

      <section class="source-list" data-source-list></section>
    </aside>

    <section class="preview">
      <div class="toolbar">
        <div>
          <strong data-selected-title>未选择共享源</strong>
          <span data-session-status>idle</span>
        </div>
        <div class="actions">
          <button type="button" data-start>开始共享</button>
          <button type="button" data-pause>暂停</button>
          <button type="button" data-resume>继续</button>
          <button type="button" class="danger" data-stop>停止</button>
        </div>
      </div>

      <div class="canvas-wrap">
        <div class="preview-idle" data-preview-idle>
          <strong data-preview-title>请选择左侧来源</strong>
          <span data-preview-subtitle>选择后点击开始共享</span>
        </div>
        <video data-video autoplay playsinline muted></video>
      </div>

      <div class="status-strip">
        <span data-captured>Captured 0</span>
        <span data-published>Published 0</span>
        <span data-dropped>Dropped 0</span>
        <span data-fps>FPS 0.0</span>
        <span data-streaming>Stopped</span>
        <span data-agora-status>Agora off</span>
      </div>

      <div class="error" data-error hidden></div>
    </section>
  </main>
`

const elements = {
  sourceSummary: app.querySelector("[data-source-summary]"),
  refresh: app.querySelector("[data-refresh]"),
  kindButtons: [...app.querySelectorAll("[data-kind]")],
  quality: app.querySelector("[data-quality]"),
  displayCount: app.querySelector("[data-display-count]"),
  windowCount: app.querySelector("[data-window-count]"),
  optionInputs: [...app.querySelectorAll("[data-option]")],
  agoraEnabled: app.querySelector("[data-agora-enabled]"),
  agoraFields: [...app.querySelectorAll("[data-agora-field]")],
  sourceList: app.querySelector("[data-source-list]"),
  selectedTitle: app.querySelector("[data-selected-title]"),
  sessionStatus: app.querySelector("[data-session-status]"),
  start: app.querySelector("[data-start]"),
  pause: app.querySelector("[data-pause]"),
  resume: app.querySelector("[data-resume]"),
  stop: app.querySelector("[data-stop]"),
  previewIdle: app.querySelector("[data-preview-idle]"),
  previewTitle: app.querySelector("[data-preview-title]"),
  previewSubtitle: app.querySelector("[data-preview-subtitle]"),
  video: app.querySelector("[data-video]"),
  captured: app.querySelector("[data-captured]"),
  published: app.querySelector("[data-published]"),
  dropped: app.querySelector("[data-dropped]"),
  fps: app.querySelector("[data-fps]"),
  streaming: app.querySelector("[data-streaming]"),
  agoraStatus: app.querySelector("[data-agora-status]"),
  error: app.querySelector("[data-error]"),
}

elements.refresh.addEventListener("click", refreshSources)
elements.start.addEventListener("click", start)
elements.pause.addEventListener("click", pause)
elements.resume.addEventListener("click", resume)
elements.stop.addEventListener("click", () => void stop())
elements.quality.addEventListener("change", () => {
  if (!Object.hasOwn(captureQualityPresets, elements.quality.value)) return
  state.captureQuality = elements.quality.value
  localStorage.setItem("screenCapture.quality", state.captureQuality)
  render()
})

for (const button of elements.kindButtons) {
  button.addEventListener("click", () => selectKind(button.dataset.kind))
}

for (const input of elements.optionInputs) {
  input.addEventListener("change", () => {
    state[input.dataset.option] = input.checked
    render()
  })
}

elements.agoraEnabled.addEventListener("change", () => {
  state.agoraEnabled = elements.agoraEnabled.checked
  render()
})

for (const input of elements.agoraFields) {
  input.addEventListener("input", () => {
    state[input.dataset.agoraField] = input.value
    persistAgoraConfig(input.dataset.agoraField, input.value)
  })
}

void onCaptureSessionEnded(({ payload }) => {
  if (payload.sessionId !== state.session?.sessionId) return
  void handleCaptureSessionEnded(payload)
}).catch((error) => {
  console.error("[screen-capture] failed to listen for session ended events", error)
})

render()

async function refreshSources() {
  state.loading = true
  state.error = ""
  render()

  try {
    console.info("[screen-capture] refreshing sources")
    state.sources = await listSources({
      kinds: ["display", "window"],
      includeThumbnails: true,
      includeCurrentApp: state.includeCurrentApp,
      includeSystemUi: state.includeSystemUi,
      debugRawSources: state.debugRawSources,
    })
    console.info("[screen-capture] sources refreshed", {
      total: state.sources.length,
      displays: displays().length,
      windows: windows().length,
    })
    if (!state.selected || !state.sources.some((source) => source.id === state.selected.id)) {
      state.selected =
        state.sources.find((source) => source.kind === state.activeKind) ?? state.sources[0] ?? null
    }
  } catch (err) {
    state.error = errorMessage(err)
    console.error("[screen-capture] refresh sources failed", err)
  } finally {
    state.loading = false
    render()
  }
}

async function start() {
  if (!state.selected || state.session) return
  state.error = ""
  render()

  try {
    const currentPermission = await checkPermission()
    console.info("[screen-capture] permission before request", currentPermission)
    const permission = await requestPermission()
    console.info("[screen-capture] permission after request", permission)
    if (permission !== "granted") {
      throw new Error(`Screen recording permission is ${permission}`)
    }

    const quality = selectedQualityPreset()
    const captureSize = scaledCaptureSize(state.selected, quality.maxWidth, quality.maxHeight)
    console.info("[screen-capture] starting capture", {
      sourceId: state.selected.id,
      sourceKind: state.selected.kind,
      name: state.selected.name,
      quality: state.captureQuality,
      outputSize: captureSize,
    })
    state.session = await startCapture({
      sourceId: state.selected.id,
      sourceKind: state.selected.kind,
      fps: quality.fps,
      width: captureSize.width,
      height: captureSize.height,
      captureCursor: true,
    })
    console.info("[screen-capture] capture session started", state.session)
    const videoConnection = await connectVideo(state.session, elements.video)
    state.peerConnection = videoConnection.peerConnection
    console.info("[screen-capture] WebRTC connected")
    state.captureVideoTrack = await videoConnection.videoTrackReady
    if (state.agoraEnabled) {
      await publishToAgora()
    }
    state.pollTimer = setInterval(updateStats, 1000)
    await updateStats()
  } catch (err) {
    state.error = errorMessage(err)
    console.error("[screen-capture] start failed", err)
    await stop()
  } finally {
    render()
  }
}

async function updateStats() {
  if (!state.session) return
  state.stats = await getCaptureStats(state.session.sessionId)
  console.info("[screen-capture] stats", state.stats)
  renderStats()
}

async function publishToAgora() {
  if (!state.captureVideoTrack) {
    throw new Error("Capture video track is not ready for Agora")
  }
  state.agoraPublishing = true
  renderStats()
  state.agoraPublication = await publishAgoraScreenTrack(
    {
      appId: state.agoraAppId,
      channel: state.agoraChannel,
      token: state.agoraToken,
      uid: state.agoraUid,
    },
    state.captureVideoTrack,
  )
  console.info("[screen-capture] Agora published", {
    uid: state.agoraPublication.uid,
    channel: state.agoraPublication.channel,
  })
  renderStats()
}

async function pause() {
  if (!state.session) return
  await pauseCapture(state.session.sessionId)
  await updateStats()
}

async function resume() {
  if (!state.session) return
  await resumeCapture(state.session.sessionId)
  await updateStats()
}

async function handleCaptureSessionEnded(event) {
  await stop({ stopBackend: false })
  window.alert(event.error.message || "被共享窗口已关闭")
}

async function stop({ stopBackend = true } = {}) {
  const activeSession = state.session
  state.session = null
  state.stats = null
  if (state.pollTimer) clearInterval(state.pollTimer)
  state.pollTimer = null
  if (state.agoraPublication) {
    try {
      await state.agoraPublication.stop()
    } catch (err) {
      state.error = errorMessage(err)
    }
  }
  state.agoraPublication = null
  state.agoraPublishing = false
  state.captureVideoTrack = null
  if (state.peerConnection) state.peerConnection.close()
  state.peerConnection = null
  elements.video.srcObject = null

  if (activeSession && stopBackend) {
    try {
      await stopCapture(activeSession.sessionId)
    } catch (err) {
      state.error = errorMessage(err)
    }
  }
  render()
}

function selectKind(kind) {
  if (state.session) return
  state.activeKind = kind
  if (state.selected?.kind !== kind) {
    state.selected = state.sources.find((source) => source.kind === kind) ?? null
  }
  render()
}

function selectSource(sourceId) {
  if (state.session) return
  state.selected = state.sources.find((source) => source.id === sourceId) ?? null
  render()
}

function render() {
  const displaySources = displays()
  const windowSources = windows()
  const activeSources = state.activeKind === "display" ? displaySources : windowSources
  const hasSession = Boolean(state.session)

  elements.sourceSummary.textContent = state.loading
    ? "正在更新可共享列表"
    : `${state.sources.length} 个可共享来源`
  elements.refresh.textContent = state.loading ? "刷新中" : "刷新"
  elements.refresh.disabled = state.loading || hasSession
  elements.displayCount.textContent = String(displaySources.length)
  elements.windowCount.textContent = String(windowSources.length)

  for (const button of elements.kindButtons) {
    const active = button.dataset.kind === state.activeKind
    button.classList.toggle("active", active)
    button.setAttribute("aria-selected", String(active))
    button.disabled = hasSession
  }

  for (const input of elements.optionInputs) {
    input.checked = Boolean(state[input.dataset.option])
    input.disabled = hasSession
  }

  elements.quality.value = state.captureQuality
  elements.quality.disabled = hasSession

  elements.agoraEnabled.checked = state.agoraEnabled
  elements.agoraEnabled.disabled = hasSession
  for (const input of elements.agoraFields) {
    input.value = state[input.dataset.agoraField]
    input.disabled = hasSession || !state.agoraEnabled
  }

  renderSourceList(activeSources)
  renderPreview()
  renderStats()

  elements.start.disabled = !state.selected || hasSession
  elements.pause.disabled = !hasSession
  elements.resume.disabled = !hasSession
  elements.stop.disabled = !hasSession

  elements.error.hidden = !state.error
  elements.error.textContent = state.error
}

function renderSourceList(activeSources) {
  elements.sourceList.setAttribute(
    "aria-label",
    state.activeKind === "display" ? "共享屏幕列表" : "共享窗口列表",
  )

  if (activeSources.length === 0) {
    elements.sourceList.innerHTML = `
      <div class="empty-state">
        <strong>暂无可共享${state.activeKind === "display" ? "屏幕" : "窗口"}</strong>
        <span>点击刷新重新获取列表</span>
      </div>
    `
    return
  }

  elements.sourceList.innerHTML = `
    <div class="source-grid">
      ${activeSources.map(sourceCardTemplate).join("")}
    </div>
  `

  for (const card of elements.sourceList.querySelectorAll("[data-source-id]")) {
    card.addEventListener("click", () => selectSource(card.dataset.sourceId))
  }
}

function sourceCardTemplate(source) {
  const selected = state.selected?.id === source.id
  const thumbnail = previewSrc(source)
    ? `<img src="${escapeAttribute(previewSrc(source))}" alt="" loading="lazy" />`
    : `<span class="thumbnail-placeholder"><span>${source.kind === "display" ? "Screen" : "Window"}</span></span>`
  const filteredReason = source.filteredReason
    ? `<em>${escapeHtml(source.filteredReason)}</em>`
    : ""

  return `
    <button
      type="button"
      class="source-card${selected ? " selected" : ""}"
      data-source-id="${escapeAttribute(source.id)}"
    >
      <span class="thumbnail">${thumbnail}</span>
      <span class="source-meta">
        <strong title="${escapeAttribute(source.name)}">${escapeHtml(source.name)}</strong>
        <span>${escapeHtml(sourceSubtitle(source))}</span>
        ${filteredReason}
      </span>
      <span class="selected-mark" aria-hidden="true"></span>
    </button>
  `
}

function renderPreview() {
  elements.selectedTitle.textContent = state.selected ? state.selected.name : "未选择共享源"
  elements.sessionStatus.textContent = state.session ? state.session.status : "idle"
  elements.previewIdle.hidden = Boolean(state.session)
  elements.previewTitle.textContent = state.selected ? state.selected.name : "请选择左侧来源"
  elements.previewSubtitle.textContent = state.selected
    ? `${sourceSubtitle(state.selected)} · 输出 ${captureSizeLabel(state.selected)}`
    : "选择后点击开始共享"
}

function renderStats() {
  const stats = state.stats ?? {}
  elements.captured.textContent = `Captured ${stats.framesCaptured ?? 0} @ ${(stats.captureFps ?? 0).toFixed(1)}`
  elements.published.textContent = `Published ${stats.framesPublished ?? 0} @ ${(stats.publishFps ?? 0).toFixed(1)}`
  elements.dropped.textContent = `Dropped ${stats.framesDropped ?? 0} C/P/E ${stats.framesCaptureDropped ?? 0}/${stats.framesPipelineDropped ?? 0}/${stats.framesEncoderDropped ?? 0}`
  elements.fps.textContent = `FPS ${(stats.fps ?? 0).toFixed(1)} ${(stats.bitrateKbps ?? 0)}kbps readback ${stats.framesCpuReadback ?? 0} ${stats.encoderBackend ?? "no-encoder"}`
  elements.streaming.textContent = state.stats?.started ? "Streaming" : "Stopped"
  elements.agoraStatus.textContent = state.agoraPublication
    ? `Agora ${state.agoraPublication.channel} / ${state.agoraPublication.uid}`
    : state.agoraPublishing
      ? "Agora publishing"
      : state.agoraEnabled
        ? "Agora ready"
        : "Agora off"
}

function displays() {
  return state.sources.filter((source) => source.kind === "display")
}

function windows() {
  return state.sources.filter((source) => source.kind === "window")
}

function previewSrc(source) {
  return source.thumbnailBase64 ? `data:image/png;base64,${source.thumbnailBase64}` : null
}

function sourceSubtitle(source) {
  const size = `${source.width} x ${source.height}`
  if (source.kind === "window") return `${source.appName ?? "Application"} · ${size}`
  return size
}

function scaledCaptureSize(source, maxWidth, maxHeight) {
  const scale = Math.min(
    1,
    maxWidth / Math.max(source.width, 1),
    maxHeight / Math.max(source.height, 1),
  )
  return {
    width: Math.max(1, Math.round(source.width * scale)),
    height: Math.max(1, Math.round(source.height * scale)),
  }
}

function selectedQualityPreset() {
  return captureQualityPresets[state.captureQuality] ?? captureQualityPresets["1080p"]
}

function captureSizeLabel(source) {
  const quality = selectedQualityPreset()
  const size = scaledCaptureSize(source, quality.maxWidth, quality.maxHeight)
  return `${size.width} × ${size.height} @ ${quality.fps} FPS`
}

function persistAgoraConfig(field, value) {
  const keyByField = {
    agoraAppId: "screenCapture.agora.appId",
    agoraChannel: "screenCapture.agora.channel",
    agoraToken: "screenCapture.agora.token",
    agoraUid: "screenCapture.agora.uid",
  }
  const key = keyByField[field]
  if (!key) return
  localStorage.setItem(key, value)
}

function errorMessage(err) {
  if (err && typeof err === "object" && "message" in err) return err.message
  return typeof err === "string" ? err : JSON.stringify(err)
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;")
}

function escapeAttribute(value) {
  return escapeHtml(value)
}
