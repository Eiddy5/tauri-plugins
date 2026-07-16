import { createAnnotationController } from "tauri-plugin-screen-capture-api"

export function createAnnotationBoard({
  elements,
  createController = createAnnotationController,
  onError = console.error,
}) {
  const { toggle, toolbar, canvas } = elements
  let controller = null
  let enabled = false
  let videoSize = null
  let activeTool = "pen"
  let activePointerId = null
  let activeElement = null

  const report = (promise) => void promise.catch(onError)

  const render = () => {
    toggle.disabled = !controller
    toggle.setAttribute("aria-pressed", String(enabled))
    toggle.textContent = enabled ? "关闭画板" : "开启画板"
    toolbar.hidden = !enabled
    canvas.hidden = !enabled
    renderTools(elements.tools, activeTool)
  }

  const setEnabled = async (nextEnabled) => {
    if (!controller || enabled === nextEnabled) return
    if (!nextEnabled) await rollbackGesture()
    await controller.setVisible(nextEnabled)
    enabled = nextEnabled
    render()
  }

  toggle.addEventListener("click", () => {
    report(setEnabled(!enabled))
  })

  canvas.addEventListener("pointerdown", (event) => {
    if (!enabled || !controller || event.button !== 0) return
    const point = pointerPoint(event)
    if (!point) return
    if (activeTool === "eraser") {
      report(controller.eraseAt(point))
      return
    }

    activePointerId = event.pointerId
    canvas.setPointerCapture?.(event.pointerId)
    activeElement = {
      id: globalThis.crypto.randomUUID(),
      kind: activeTool,
      points: initialPoints(activeTool, point),
      color: annotationColor(elements.color?.value ?? "#ff3b30"),
      width: 0.006,
    }
    report(controller.beginElement(activeElement))
  })

  canvas.addEventListener("pointermove", (event) => {
    if (event.pointerId !== activePointerId || !activeElement) return
    const point = pointerPoint(event)
    if (!point) return
    activeElement.points = updatedPoints(activeElement, point)
    report(controller.updateElement(activeElement))
  })

  canvas.addEventListener("pointerup", finishGesture)
  canvas.addEventListener("pointercancel", finishGesture)

  for (const button of elements.tools ?? []) {
    button.addEventListener("click", () => {
      activeTool = button.dataset.tool
      renderTools(elements.tools, activeTool)
    })
  }
  elements.undo?.addEventListener("click", () => controller && report(controller.undo()))
  elements.redo?.addEventListener("click", () => controller && report(controller.redo()))
  elements.clear?.addEventListener("click", () => controller && report(controller.clear()))

  function pointerPoint(event) {
    return videoSize
      ? mapPointerToVideo(event, canvas.getBoundingClientRect(), videoSize)
      : null
  }

  function finishGesture(event) {
    if (event.pointerId !== activePointerId || !activeElement) return
    const point = pointerPoint(event)
    if (point) activeElement.points = updatedPoints(activeElement, point)
    const completed = activeElement
    cancelGesture()
    report(controller.commitElement(completed))
  }

  function cancelGesture() {
    if (activePointerId !== null) canvas.releasePointerCapture?.(activePointerId)
    activePointerId = null
    activeElement = null
  }

  async function rollbackGesture() {
    if (!activeElement) return
    try {
      await controller.cancelElement()
    } finally {
      cancelGesture()
    }
  }

  render()

  return {
    attach({ sessionId, width, height }) {
      controller = createController(sessionId, {
        videoWidth: width,
        videoHeight: height,
      })
      videoSize = { width, height }
      enabled = false
      render()
    },
    async detach() {
      try {
        if (controller && enabled) await setEnabled(false)
      } finally {
        controller = null
        videoSize = null
        enabled = false
        cancelGesture()
        render()
      }
    },
    setEnabled,
    get enabled() {
      return enabled
    },
  }
}

export function mapPointerToVideo(pointer, bounds, videoSize) {
  if (bounds.width <= 0 || bounds.height <= 0 || videoSize.width <= 0 || videoSize.height <= 0) {
    return null
  }
  const containerAspect = bounds.width / bounds.height
  const videoAspect = videoSize.width / videoSize.height
  const contentWidth = videoAspect > containerAspect ? bounds.width : bounds.height * videoAspect
  const contentHeight = videoAspect > containerAspect ? bounds.width / videoAspect : bounds.height
  const contentLeft = bounds.left + (bounds.width - contentWidth) / 2
  const contentTop = bounds.top + (bounds.height - contentHeight) / 2
  const x = (pointer.clientX - contentLeft) / contentWidth
  const y = (pointer.clientY - contentTop) / contentHeight
  if (x < 0 || x > 1 || y < 0 || y > 1) return null
  return { x, y }
}

function annotationColor(hex) {
  const value = hex.startsWith("#") ? hex.slice(1) : hex
  return {
    red: Number.parseInt(value.slice(0, 2), 16),
    green: Number.parseInt(value.slice(2, 4), 16),
    blue: Number.parseInt(value.slice(4, 6), 16),
    alpha: 255,
  }
}

function initialPoints(tool, point) {
  return tool === "pen" ? [point] : [point, point]
}

function updatedPoints(element, point) {
  return element.kind === "pen" ? [...element.points, point] : [element.points[0], point]
}

function renderTools(tools = [], activeTool) {
  for (const button of tools) {
    const active = button.dataset.tool === activeTool
    button.classList?.toggle("active", active)
    button.setAttribute("aria-pressed", String(active))
  }
}
