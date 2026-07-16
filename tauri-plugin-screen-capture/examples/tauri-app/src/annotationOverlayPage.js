import "./annotationOverlay.css"

import { invoke } from "@tauri-apps/api/core"
import {
  LogicalPosition,
  LogicalSize,
  PhysicalPosition,
  PhysicalSize,
} from "@tauri-apps/api/dpi"
import { getCurrentWindow } from "@tauri-apps/api/window"
import { getAnnotationInputTarget } from "tauri-plugin-screen-capture-api"

import { createAnnotationBoard } from "./lib/annotationBoard.js"
import { createAnnotationTargetSynchronizer } from "./lib/annotationTargetGeometry.js"

const params = new URLSearchParams(window.location.search)
const sessionId = params.get("sessionId")
const videoWidth = Number(params.get("width"))
const videoHeight = Number(params.get("height"))
const appWindow = getCurrentWindow()
const dpi = { LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize }

if (!sessionId || !Number.isFinite(videoWidth) || !Number.isFinite(videoHeight)) {
  throw new Error("Annotation overlay requires a session and video dimensions")
}

document.querySelector("#app").innerHTML = `
  <main class="annotation-overlay">
    <canvas data-board-canvas></canvas>
    <div class="annotation-toolbar" role="toolbar" aria-label="画板工具">
      <button type="button" data-board-tool="pen" aria-pressed="true">画笔</button>
      <button type="button" data-board-tool="line" aria-pressed="false">直线</button>
      <button type="button" data-board-tool="rectangle" aria-pressed="false">矩形</button>
      <button type="button" data-board-tool="ellipse" aria-pressed="false">椭圆</button>
      <button type="button" data-board-tool="arrow" aria-pressed="false">箭头</button>
      <button type="button" data-board-tool="eraser" aria-pressed="false">橡皮擦</button>
      <input type="color" value="#ff3b30" data-board-color aria-label="颜色" />
      <button type="button" data-board-undo>撤销</button>
      <button type="button" data-board-redo>重做</button>
      <button type="button" data-board-clear>清空</button>
      <button type="button" class="close" data-board-close>关闭画板</button>
    </div>
  </main>
`

const root = document.querySelector("#app")
const detachedToggle = document.createElement("button")
const board = createAnnotationBoard({
  elements: {
    toggle: detachedToggle,
    toolbar: root.querySelector(".annotation-toolbar"),
    canvas: root.querySelector("[data-board-canvas]"),
    tools: [...root.querySelectorAll("[data-board-tool]")],
    color: root.querySelector("[data-board-color]"),
    undo: root.querySelector("[data-board-undo]"),
    redo: root.querySelector("[data-board-redo]"),
    clear: root.querySelector("[data-board-clear]"),
  },
  onError: closeWithError,
})

board.attach({ sessionId, width: videoWidth, height: videoHeight })
await board.setEnabled(true)
await invoke("prepare_annotation_overlay")

let stopped = false
const syncGeometry = createAnnotationTargetSynchronizer(appWindow, dpi)

root.querySelector("[data-board-close]").addEventListener("click", closeOverlay)
window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") void closeOverlay()
})
window.addEventListener("beforeunload", () => {
  stopped = true
  void board.detach()
})

await syncTarget()

async function syncTarget() {
  if (stopped) return
  try {
    const target = await getAnnotationInputTarget(sessionId)
    await syncGeometry(target)
  } catch (error) {
    await closeWithError(error)
    return
  }
  window.setTimeout(syncTarget, 100)
}

async function closeOverlay() {
  if (stopped) return
  stopped = true
  try {
    await board.detach()
  } finally {
    await appWindow.close()
  }
}

async function closeWithError(error) {
  console.error("[annotation-overlay]", error)
  await closeOverlay()
}
