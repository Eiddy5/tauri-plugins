import "./style.css"
import {
  checkPermission,
  clearAnnotations,
  getAnnotationState,
  getCapabilities,
  getCaptureStats,
  listSources,
  onAnnotationStateChanged,
  onCaptureSessionEnded,
  pauseCapture,
  requestPermission,
  setAnnotationInteraction,
  setAnnotationTool,
  resumeCapture,
  startCapture,
  stopCapture,
  undoAnnotation,
} from "tauri-plugin-screen-capture-api"
import { WebviewWindow } from "@tauri-apps/api/webviewWindow"
import { publishAgoraScreenTrack } from "./lib/agoraPublisher.js"
import { createAnnotationTargetWindowController } from "./lib/annotationTargetWindow.js"
import { connectVideo } from "./lib/screenCapture.js"
import { shareExperienceMode } from "./lib/shareExperience.js"

const captureQualityPresets = {
  "720p": { label: "720p HD", maxWidth: 1280, maxHeight: 720, fps: 60 },
  "1080p": { label: "1080p Full HD", maxWidth: 1920, maxHeight: 1080, fps: 60 },
  "2k": { label: "2K QHD（推荐）", maxWidth: 2560, maxHeight: 1440, fps: 60 },
  "4k": { label: "4K UHD", maxWidth: 3840, maxHeight: 2160, fps: 30 },
}
const storedCaptureQuality = localStorage.getItem("screenCapture.quality")
const defaultCaptureQuality = Object.hasOwn(captureQualityPresets, storedCaptureQuality)
  ? storedCaptureQuality
  : "2k"
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
  pickerOpen: false,
  capabilities: null,
  annotation: null,
  annotationColor: "#ff3b30",
  annotationWidth: 4,
}

const app = document.querySelector("#app")

app.innerHTML = `
  <main class="app-shell" data-mode="landing">
    <section class="landing" data-landing>
      <div class="landing-card">
        <span class="app-mark" aria-hidden="true">S</span>
        <p class="eyebrow">SCREEN CAPTURE</p>
        <h1>共享你的屏幕</h1>
        <p>选择一个屏幕或窗口，将画面清晰、流畅地共享给其他人。</p>
        <button type="button" class="primary share-entry" data-open-picker>开始共享</button>
      </div>
    </section>

    <section class="share-stage" data-share-stage>
      <div class="canvas-wrap">
        <div class="preview-idle" data-preview-idle>
          <strong data-preview-title>正在准备共享画面</strong>
          <span data-preview-subtitle></span>
        </div>
        <video data-video autoplay playsinline muted></video>
      </div>

      <header class="share-topbar">
        <button type="button" class="stop-sharing" data-stop>结束共享</button>
      </header>

      <div class="share-bottom-dock">
        <button type="button" class="board-button" data-board-toggle aria-pressed="false">
          <span class="board-icon" aria-hidden="true">✎</span>
          <span>画板</span>
        </button>
        <div class="annotation-toolbar" data-annotation-toolbar hidden>
          <button type="button" data-annotation-mode>关闭画板</button>
          <button type="button" data-annotation-tool="pen">画笔</button>
          <button type="button" data-annotation-tool="eraser">橡皮擦</button>
          <input type="color" value="#ff3b30" data-annotation-color aria-label="标注颜色" />
          <input type="range" min="1" max="64" value="4" data-annotation-width aria-label="标注粗细" />
          <button type="button" data-annotation-undo>撤销</button>
          <button type="button" data-annotation-clear>清空</button>
        </div>
      </div>

      <div class="hidden-runtime-controls" hidden>
        <strong data-selected-title>未选择共享源</strong>
        <span data-session-status>idle</span>
        <button type="button" data-pause>暂停</button>
        <button type="button" data-resume>继续</button>
        <span data-captured>Captured 0</span>
        <span data-published>Published 0</span>
        <span data-dropped>Dropped 0</span>
        <span data-fps>FPS 0.0</span>
        <span data-streaming>Stopped</span>
        <span data-agora-status>Agora off</span>
      </div>
    </section>

    <div class="picker-backdrop" data-picker>
      <section class="picker-dialog" role="dialog" aria-modal="true" aria-labelledby="picker-title">
        <header class="picker-header">
          <div class="picker-heading">
            <h2 id="picker-title">选择共享内容</h2>
            <p>请选择要展示给其他人的屏幕或应用窗口</p>
          </div>
          <button type="button" class="icon-button close-button" data-close-picker aria-label="关闭">×</button>
        </header>

        <div class="picker-nav">
          <div class="source-tabs" role="tablist" aria-label="共享类型">
            <button type="button" role="tab" data-kind="display">
              <span class="tab-icon" aria-hidden="true">▣</span>
              <span>整个屏幕</span>
              <small data-display-count>0</small>
            </button>
            <button type="button" role="tab" data-kind="window">
              <span class="tab-icon" aria-hidden="true">▤</span>
              <span>应用窗口</span>
              <small data-window-count>0</small>
            </button>
          </div>
          <div class="source-tools">
            <span data-source-summary>0 个可共享来源</span>
            <button type="button" class="refresh-button" data-refresh aria-label="刷新共享内容">
              <span aria-hidden="true">↻</span>
              刷新
            </button>
          </div>
        </div>

        <section class="source-list" data-source-list></section>

        <footer class="picker-footer">
          <div class="picker-preferences">
            <label class="quality-control">
              <span>清晰度</span>
              <select data-quality aria-label="共享清晰度">
                ${Object.entries(captureQualityPresets)
                  .map(([value, preset]) => `<option value="${value}">${preset.label} · ${preset.fps} FPS</option>`)
                  .join("")}
              </select>
            </label>
            <details class="advanced-settings">
              <summary>更多设置</summary>
              <div class="advanced-popover">
                <div class="options">
                  <label><input type="checkbox" data-option="debugRawSources" /> 显示原始来源</label>
                  <label><input type="checkbox" data-option="includeCurrentApp" /> 包含当前应用</label>
                  <label><input type="checkbox" data-option="includeSystemUi" /> 包含系统窗口</label>
                </div>
                <section class="agora-panel">
                  <label class="agora-toggle">
                    <input type="checkbox" data-agora-enabled />
                    <span>发布到 Agora</span>
                  </label>
                  <label><span>App ID</span><input type="text" data-agora-field="agoraAppId" autocomplete="off" spellcheck="false" /></label>
                  <label><span>Channel</span><input type="text" data-agora-field="agoraChannel" autocomplete="off" spellcheck="false" /></label>
                  <label><span>Token</span><input type="text" data-agora-field="agoraToken" autocomplete="off" spellcheck="false" /></label>
                  <label><span>UID</span><input type="number" min="0" step="1" data-agora-field="agoraUid" /></label>
                </section>
              </div>
            </details>
          </div>
          <div class="picker-actions">
            <button type="button" class="cancel-button" data-close-picker>取消</button>
            <button type="button" class="primary share-button" data-start>
              <span aria-hidden="true">▶</span>
              开始共享
            </button>
          </div>
        </footer>
      </section>
    </div>

    <div class="error app-error" data-error hidden></div>
  </main>
`

const annotationElements = {
  toggle: app.querySelector("[data-board-toggle]"),
  toolbar: app.querySelector("[data-annotation-toolbar]"),
  mode: app.querySelector("[data-annotation-mode]"),
  tools: [...app.querySelectorAll("[data-annotation-tool]")],
  color: app.querySelector("[data-annotation-color]"),
  width: app.querySelector("[data-annotation-width]"),
  undo: app.querySelector("[data-annotation-undo]"),
  clear: app.querySelector("[data-annotation-clear]"),
}

const elements = {
  shell: app.querySelector(".app-shell"),
  openPicker: app.querySelector("[data-open-picker]"),
  closePicker: [...app.querySelectorAll("[data-close-picker]")],
  picker: app.querySelector("[data-picker]"),
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

const annotationBoard = createAnnotationTargetWindowController({
  toggle: annotationElements.toggle,
  createWindow: (label, options) => new WebviewWindow(label, options),
  onError(error) {
    state.error = errorMessage(error)
    render()
  },
})

annotationElements.toggle.addEventListener("click", () => {
  if (supportsNativeAnnotation()) toggleNativeAnnotationSafely()
})

elements.openPicker.addEventListener("click", openPicker)
for (const button of elements.closePicker) button.addEventListener("click", closePicker)
elements.picker.addEventListener("click", (event) => {
  if (event.target === elements.picker) closePicker()
})
window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && state.pickerOpen && !state.session) closePicker()
})
elements.refresh.addEventListener("click", refreshSources)
elements.start.addEventListener("click", start)
elements.pause.addEventListener("click", pause)
elements.resume.addEventListener("click", resume)
elements.stop.addEventListener("click", () => void stop())
annotationElements.mode.addEventListener("click", toggleNativeAnnotationSafely)
for (const button of annotationElements.tools) {
  button.addEventListener("click", () => void selectNativeAnnotationTool(button.dataset.annotationTool))
}
annotationElements.color.addEventListener("input", () => {
  state.annotationColor = annotationElements.color.value
  if (state.annotation?.tool.kind === "pen") void applyNativeAnnotationTool("pen")
})
annotationElements.width.addEventListener("change", () => {
  state.annotationWidth = Number(annotationElements.width.value)
  void applyNativeAnnotationTool(state.annotation?.tool.kind ?? "pen")
})
annotationElements.undo.addEventListener("click", () => void mutateNativeAnnotation(undoAnnotation))
annotationElements.clear.addEventListener("click", () => void mutateNativeAnnotation(clearAnnotations))
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

void onAnnotationStateChanged(({ payload }) => {
  if (payload.sessionId !== state.session?.sessionId) return
  state.annotation = payload.state
  renderAnnotationToolbar()
}).catch((error) => {
  console.error("[screen-capture] failed to listen for annotation events", error)
})

void getCapabilities()
  .then((capabilities) => {
    state.capabilities = capabilities
    render()
  })
  .catch((error) => console.error("[screen-capture] failed to read capabilities", error))

render()

function openPicker() {
  if (state.session) return
  state.pickerOpen = true
  render()
  void refreshSources()
}

function closePicker() {
  if (state.session) return
  state.pickerOpen = false
  render()
}

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
      annotations: { enabled: true },
    })
    if (supportsNativeAnnotation()) {
      state.annotation = await getAnnotationState(state.session.sessionId)
    }
    state.pickerOpen = false
    if (!supportsNativeAnnotation()) {
      annotationBoard.attach({
        sessionId: state.session.sessionId,
        width: captureSize.width,
        height: captureSize.height,
      })
    }
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
  state.annotation = null
  if (state.pollTimer) clearInterval(state.pollTimer)
  state.pollTimer = null
  try {
    if (!supportsNativeAnnotation()) await annotationBoard.detach()
  } catch (err) {
    state.error = errorMessage(err)
  }
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
  elements.shell.dataset.mode = shareExperienceMode(state)

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
  renderAnnotationToolbar()

  elements.start.disabled = !state.selected || hasSession
  elements.pause.disabled = !hasSession
  elements.resume.disabled = !hasSession
  elements.stop.disabled = !hasSession

  elements.error.hidden = !state.error
  elements.error.textContent = state.error
}

function supportsNativeAnnotation() {
  return Boolean(state.capabilities?.annotationTools?.includes("pen"))
}

async function toggleNativeAnnotation() {
  if (!state.session || !supportsNativeAnnotation()) return
  state.annotation = await setAnnotationInteraction(
    state.session.sessionId,
    !state.annotation?.interactionEnabled,
  )
  renderAnnotationToolbar()
}

function toggleNativeAnnotationSafely() {
  void toggleNativeAnnotation().catch((error) => {
    state.error = errorMessage(error)
    render()
  })
}

async function selectNativeAnnotationTool(kind) {
  if (!state.session) return
  await applyNativeAnnotationTool(kind)
}

async function applyNativeAnnotationTool(kind) {
  if (!state.session || !supportsNativeAnnotation()) return
  const tool = kind === "eraser"
    ? { kind: "eraser", width: Math.max(4, state.annotationWidth) }
    : { kind: "pen", color: state.annotationColor.toUpperCase(), width: state.annotationWidth }
  state.annotation = await setAnnotationTool(state.session.sessionId, tool)
  renderAnnotationToolbar()
}

async function mutateNativeAnnotation(command) {
  if (!state.session || !supportsNativeAnnotation()) return
  state.annotation = await command(state.session.sessionId)
  renderAnnotationToolbar()
}

function renderAnnotationToolbar() {
  const native = supportsNativeAnnotation()
  if (!native) {
    annotationElements.toolbar.hidden = true
    return
  }
  const annotation = state.annotation
  const enabled = Boolean(annotation?.interactionEnabled)
  annotationElements.toggle.hidden = false
  annotationElements.toggle.disabled = !state.session
  annotationElements.toggle.textContent = enabled ? "关闭画板" : "开启画板"
  annotationElements.toggle.setAttribute("aria-pressed", String(enabled))
  annotationElements.toolbar.hidden = !state.session || !enabled
  annotationElements.mode.classList.toggle("selected", enabled)
  annotationElements.mode.setAttribute("aria-pressed", String(enabled))
  for (const button of annotationElements.tools) {
    button.classList.toggle("selected", annotation?.tool.kind === button.dataset.annotationTool)
    button.disabled = !state.session
  }
  annotationElements.color.value = state.annotationColor
  annotationElements.width.value = String(state.annotationWidth)
  annotationElements.color.disabled = annotation?.tool.kind === "eraser"
  annotationElements.undo.disabled = !annotation?.canUndo
  annotationElements.clear.disabled = !annotation?.operationCount
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
