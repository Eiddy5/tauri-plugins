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
  const publish = (promise) => {
    renderCanvas()
    report(promise.then(renderCanvas))
  }

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
    renderCanvas()
  }

  toggle.addEventListener("click", () => {
    report(setEnabled(!enabled))
  })

  canvas.addEventListener("pointerdown", (event) => {
    if (!enabled || !controller || event.button !== 0) return
    const point = pointerPoint(event)
    if (!point) return
    if (activeTool === "eraser") {
      publish(controller.eraseAt(point))
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
    publish(controller.beginElement(activeElement))
  })

  canvas.addEventListener("pointermove", (event) => {
    if (event.pointerId !== activePointerId || !activeElement) return
    const point = pointerPoint(event)
    if (!point) return
    activeElement.points = updatedPoints(activeElement, point)
    publish(controller.updateElement(activeElement))
  })

  canvas.addEventListener("pointerup", finishGesture)
  canvas.addEventListener("pointercancel", finishGesture)

  for (const button of elements.tools ?? []) {
    button.addEventListener("click", () => {
      activeTool = button.dataset.tool
      renderTools(elements.tools, activeTool)
    })
  }
  elements.undo?.addEventListener("click", () => controller && publish(controller.undo()))
  elements.redo?.addEventListener("click", () => controller && publish(controller.redo()))
  elements.clear?.addEventListener("click", () => controller && publish(controller.clear()))

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
    publish(controller.commitElement(completed))
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
      renderCanvas()
    }
  }

  function renderCanvas() {
    const context = canvas.getContext?.("2d")
    if (!context) return
    const bounds = canvas.getBoundingClientRect()
    const ratio = globalThis.devicePixelRatio || 1
    const width = Math.max(1, Math.round(bounds.width * ratio))
    const height = Math.max(1, Math.round(bounds.height * ratio))
    if (canvas.width !== width) canvas.width = width
    if (canvas.height !== height) canvas.height = height
    context.setTransform(ratio, 0, 0, ratio, 0, 0)
    context.clearRect(0, 0, bounds.width, bounds.height)
    if (!videoSize || !controller?.document?.visible) return

    const content = videoContentRect(bounds, videoSize)
    for (const element of controller.document.elements) drawElement(context, element, content)
  }

  const resizeObserver = globalThis.ResizeObserver
    ? new ResizeObserver(renderCanvas)
    : null
  resizeObserver?.observe(canvas)

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
      renderCanvas()
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
        renderCanvas()
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

function videoContentRect(bounds, videoSize) {
  const containerAspect = bounds.width / bounds.height
  const videoAspect = videoSize.width / videoSize.height
  const width = videoAspect > containerAspect ? bounds.width : bounds.height * videoAspect
  const height = videoAspect > containerAspect ? bounds.width / videoAspect : bounds.height
  return {
    left: (bounds.width - width) / 2,
    top: (bounds.height - height) / 2,
    width,
    height,
  }
}

function drawElement(context, element, content) {
  if (!element.points.length) return
  const points = element.points.map((point) => ({
    x: content.left + point.x * content.width,
    y: content.top + point.y * content.height,
  }))
  const color = element.color
  context.strokeStyle = `rgba(${color.red}, ${color.green}, ${color.blue}, ${color.alpha / 255})`
  context.lineWidth = Math.max(1, element.width * Math.min(content.width, content.height))
  context.lineCap = "round"
  context.lineJoin = "round"

  const [start, end = start] = points
  context.beginPath()
  if (element.kind === "pen") {
    context.moveTo(start.x, start.y)
    for (const point of points.slice(1)) context.lineTo(point.x, point.y)
  } else if (element.kind === "rectangle") {
    context.rect(start.x, start.y, end.x - start.x, end.y - start.y)
  } else if (element.kind === "ellipse") {
    context.ellipse(
      (start.x + end.x) / 2,
      (start.y + end.y) / 2,
      Math.abs(end.x - start.x) / 2,
      Math.abs(end.y - start.y) / 2,
      0,
      0,
      Math.PI * 2,
    )
  } else {
    context.moveTo(start.x, start.y)
    context.lineTo(end.x, end.y)
    if (element.kind === "arrow") {
      const angle = Math.atan2(end.y - start.y, end.x - start.x)
      const head = Math.max(10, Math.min(30, Math.hypot(end.x - start.x, end.y - start.y) * 0.25))
      for (const offset of [2.6, -2.6]) {
        context.moveTo(end.x, end.y)
        context.lineTo(end.x + head * Math.cos(angle + offset), end.y + head * Math.sin(angle + offset))
      }
    }
  }
  context.stroke()
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
