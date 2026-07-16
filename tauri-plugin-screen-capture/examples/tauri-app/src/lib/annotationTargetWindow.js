export function createAnnotationTargetWindowController({
  toggle,
  createWindow,
  onError = console.error,
}) {
  let session = null
  let overlay = null

  const render = () => {
    toggle.disabled = !session
    toggle.setAttribute("aria-pressed", String(Boolean(overlay)))
    toggle.textContent = overlay ? "关闭画板" : "开启画板"
  }

  const close = async () => {
    const activeOverlay = overlay
    overlay = null
    render()
    await activeOverlay?.close()
  }

  const open = async () => {
    if (!session || overlay) return
    const label = `annotation-${session.sessionId}`.replace(/[^a-zA-Z0-9-/:_]/g, "-")
    const params = new URLSearchParams({
      annotationOverlay: "1",
      sessionId: session.sessionId,
      width: String(session.width),
      height: String(session.height),
    })
    const window = createWindow(label, {
      url: `/?${params}`,
      title: `TAURI_SCREEN_CAPTURE_OVERLAY:${session.sessionId}`,
      transparent: true,
      decorations: false,
      shadow: false,
      alwaysOnTop: true,
      skipTaskbar: true,
      resizable: false,
      focus: true,
      visible: false,
    })
    overlay = window
    render()
    await window.once("tauri://destroyed", () => {
      if (overlay === window) {
        overlay = null
        render()
      }
    })
  }

  toggle.addEventListener("click", () => {
    void (overlay ? close() : open()).catch(onError)
  })

  render()

  return {
    attach(nextSession) {
      session = { ...nextSession }
      render()
    },
    async detach() {
      session = null
      await close()
      render()
    },
    close,
    get opened() {
      return Boolean(overlay)
    },
  }
}
