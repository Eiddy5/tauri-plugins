import "./nativeAnnotationToolbar.css"

import { invoke } from "@tauri-apps/api/core"
import {
  LogicalPosition,
  LogicalSize,
  PhysicalPosition,
  PhysicalSize,
} from "@tauri-apps/api/dpi"
import { getCurrentWindow } from "@tauri-apps/api/window"
import {
  clearAnnotations,
  getAnnotationInputTarget,
  setAnnotationInteraction,
  setAnnotationTool,
  undoAnnotation,
} from "tauri-plugin-screen-capture-api"

import { createNativeAnnotationToolbar } from "./lib/nativeAnnotationToolbar.js"
import { createNativeAnnotationToolbarSynchronizer } from "./lib/nativeAnnotationToolbarGeometry.js"

const params = new URLSearchParams(window.location.search)
const sessionId = params.get("sessionId")
const appWindow = getCurrentWindow()
const dpi = { LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize }

if (!sessionId) {
  throw new Error("Native annotation toolbar requires a capture session")
}

document.querySelector("#app").innerHTML = `
  <main class="native-annotation-toolbar" role="toolbar" aria-label="原生画板工具">
    <button type="button" data-native-tool="pen" aria-pressed="true">画笔</button>
    <button type="button" data-native-tool="eraser" aria-pressed="false">橡皮擦</button>
    <input type="color" value="#ff3b30" data-native-color aria-label="画笔颜色" />
    <button type="button" data-native-undo>撤销</button>
    <button type="button" data-native-clear>清空</button>
    <button type="button" class="close" data-native-close>关闭画板</button>
    <output class="native-annotation-error" data-native-error hidden></output>
  </main>
`

const root = document.querySelector("#app")
const errorOutput = root.querySelector("[data-native-error]")
const toolbar = createNativeAnnotationToolbar({
  sessionId,
  elements: {
    tools: [...root.querySelectorAll("[data-native-tool]")],
    color: root.querySelector("[data-native-color]"),
    undo: root.querySelector("[data-native-undo]"),
    clear: root.querySelector("[data-native-clear]"),
    close: root.querySelector("[data-native-close]"),
  },
  setInteraction: setAnnotationInteraction,
  setTool: setAnnotationTool,
  undo: undoAnnotation,
  clear: clearAnnotations,
  closeWindow: () => appWindow.close(),
  onError: reportError,
})

let stopped = false
const syncGeometry = createNativeAnnotationToolbarSynchronizer(appWindow, dpi, {
  width: 460,
  height: 64,
  topInset: 12,
})

await toolbar.start()
await invoke("prepare_annotation_overlay")
await syncTarget()

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") void closeToolbar()
})
window.addEventListener("beforeunload", () => {
  stopped = true
  void toolbar.stop()
})

async function syncTarget() {
  if (stopped) return
  try {
    const target = await getAnnotationInputTarget(sessionId)
    await syncGeometry(target)
  } catch (error) {
    reportError(error)
    await closeToolbar()
    return
  }
  window.setTimeout(syncTarget, 100)
}

async function closeToolbar() {
  if (stopped) return
  stopped = true
  try {
    await toolbar.stop()
  } finally {
    await appWindow.close()
  }
}

function reportError(error) {
  console.error("[native-annotation-toolbar]", error)
  errorOutput.textContent =
    error instanceof Error ? error.message : error?.message ?? String(error)
  errorOutput.hidden = false
}
