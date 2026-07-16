import assert from "node:assert/strict"
import test from "node:test"

import { createAnnotationTargetWindowController } from "../src/lib/annotationTargetWindow.js"

class FakeToggle extends EventTarget {
  constructor() {
    super()
    this.disabled = false
    this.attributes = new Map()
    this.textContent = ""
  }

  click() {
    this.dispatchEvent(new Event("click"))
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value))
  }

  getAttribute(name) {
    return this.attributes.get(name) ?? null
  }
}

const nextTask = () => new Promise((resolve) => setImmediate(resolve))

test("board toggle opens a target overlay and resets when it closes", async () => {
  const toggle = new FakeToggle()
  const created = []
  const overlay = fakeOverlayWindow()
  const controller = createAnnotationTargetWindowController({
    toggle,
    createWindow(label, options) {
      created.push({ label, options })
      return overlay
    },
  })

  assert.equal(toggle.disabled, true)
  controller.attach({ sessionId: "session-1", width: 1920, height: 1080 })
  assert.equal(toggle.disabled, false)

  toggle.click()
  await nextTask()

  assert.equal(created[0].label, "annotation-session-1")
  assert.match(created[0].options.url, /annotationOverlay=1/)
  assert.match(created[0].options.url, /sessionId=session-1/)
  assert.equal(created[0].options.transparent, true)
  assert.equal(created[0].options.decorations, false)
  assert.equal(toggle.getAttribute("aria-pressed"), "true")

  overlay.destroy()
  await nextTask()
  assert.equal(toggle.getAttribute("aria-pressed"), "false")
})

test("detaching closes the overlay and disables board input", async () => {
  const toggle = new FakeToggle()
  const overlay = fakeOverlayWindow()
  const controller = createAnnotationTargetWindowController({
    toggle,
    createWindow: () => overlay,
  })

  controller.attach({ sessionId: "session-2", width: 1280, height: 720 })
  toggle.click()
  await nextTask()
  await controller.detach()

  assert.equal(overlay.closed, true)
  assert.equal(toggle.disabled, true)
  assert.equal(toggle.getAttribute("aria-pressed"), "false")
})

function fakeOverlayWindow() {
  let destroyed
  return {
    closed: false,
    once(event, listener) {
      assert.equal(event, "tauri://destroyed")
      destroyed = listener
      return Promise.resolve(() => {})
    },
    async close() {
      this.closed = true
      destroyed?.()
    },
    destroy() {
      destroyed?.()
    },
  }
}
