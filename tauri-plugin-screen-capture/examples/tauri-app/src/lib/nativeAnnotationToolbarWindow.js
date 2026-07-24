export function createNativeAnnotationToolbarWindow({
  toggle,
  createWindow,
  setInteraction,
  onError = console.error,
}) {
  let session = null
  let overlay = null
  let overlaySessionId = null
  let nativeInputMayBeEnabled = false

  const render = () => {
    toggle.disabled = !session
    toggle.setAttribute("aria-pressed", String(Boolean(overlay)))
    toggle.textContent = overlay ? "关闭画板" : "开启画板"
  }

  const close = async () => {
    const activeOverlay = overlay
    const activeSessionId = overlaySessionId
    overlay = null
    overlaySessionId = null
    render()
    try {
      if (nativeInputMayBeEnabled && activeSessionId) {
        nativeInputMayBeEnabled = false
        await setInteraction(activeSessionId, false)
      }
    } finally {
      await activeOverlay?.close()
    }
  }

  const open = async () => {
    if (!session || overlay) return
    const label = `annotation-${session.sessionId}`.replace(/[^a-zA-Z0-9-/:_]/g, "-")
    const params = new URLSearchParams({
      nativeAnnotationToolbar: "1",
      sessionId: session.sessionId,
    })
    const window = createWindow(label, {
      url: `/?${params}`,
      title: `TAURI_SCREEN_CAPTURE_OVERLAY:${session.sessionId}`,
      width: 460,
      height: 64,
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
    overlaySessionId = session.sessionId
    nativeInputMayBeEnabled = true
    render()
    await window.once("tauri://destroyed", () => {
      if (overlay === window) {
        const destroyedSessionId = overlaySessionId
        overlay = null
        overlaySessionId = null
        render()
        if (nativeInputMayBeEnabled && destroyedSessionId) {
          nativeInputMayBeEnabled = false
          void setInteraction(destroyedSessionId, false).catch(onError)
        }
      }
    })
  }

  toggle.addEventListener("click", () => {
    void (overlay ? close() : open()).catch(onError)
  })

  render()

  return {
    attach(nextSession) {
      session = { sessionId: nextSession.sessionId }
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
