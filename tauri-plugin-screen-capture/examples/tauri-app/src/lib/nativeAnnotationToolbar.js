export function createNativeAnnotationToolbar({
  sessionId,
  elements,
  setInteraction,
  setTool,
  undo,
  clear,
  closeWindow,
  onError = console.error,
}) {
  let activeTool = "pen"
  let running = false
  let stopping = null

  const pen = () => ({
    kind: "pen",
    color: elements.color?.value ?? "#ff3b30",
    width: 4,
  })
  const eraser = () => ({ kind: "eraser", width: 24 })
  const toolValue = () => activeTool === "eraser" ? eraser() : pen()
  const report = (promise) => void Promise.resolve(promise).catch(onError)

  const render = () => {
    for (const button of elements.tools ?? []) {
      const active = button.dataset.nativeTool === activeTool
      button.classList?.toggle("active", active)
      button.setAttribute("aria-pressed", String(active))
    }
  }

  const selectTool = async (kind) => {
    activeTool = kind
    render()
    await setTool(sessionId, toolValue())
  }

  for (const button of elements.tools ?? []) {
    button.addEventListener("click", () => {
      report(selectTool(button.dataset.nativeTool))
    })
  }
  elements.color?.addEventListener("change", () => {
    report(selectTool("pen"))
  })
  elements.undo?.addEventListener("click", () => report(undo(sessionId)))
  elements.clear?.addEventListener("click", () => report(clear(sessionId)))
  elements.close?.addEventListener("click", () => {
    report(Promise.resolve(stop()).finally(closeWindow))
  })

  const start = async () => {
    if (running) return
    await setTool(sessionId, toolValue())
    await setInteraction(sessionId, true)
    running = true
    render()
  }

  const stop = async () => {
    if (!running) return stopping
    running = false
    stopping = Promise.resolve(setInteraction(sessionId, false)).finally(() => {
      stopping = null
    })
    return stopping
  }

  render()

  return {
    start,
    stop,
    get running() {
      return running
    },
  }
}
