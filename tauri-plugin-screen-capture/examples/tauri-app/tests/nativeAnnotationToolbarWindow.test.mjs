import assert from "node:assert/strict"
import test from "node:test"

import { createNativeAnnotationToolbarWindow } from "../src/lib/nativeAnnotationToolbarWindow.js"

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

test("board toggle opens the native toolbar and disables Rust input if it is destroyed", async () => {
  const toggle = new FakeToggle()
  const created = []
  const interactions = []
  const overlay = fakeOverlayWindow()
  const controller = createNativeAnnotationToolbarWindow({
    toggle,
    createWindow(label, options) {
      created.push({ label, options })
      return overlay
    },
    setInteraction: async (...args) => interactions.push(args),
  })

  assert.equal(toggle.disabled, true)
  controller.attach({ sessionId: "session-1", width: 1920, height: 1080 })
  assert.equal(toggle.disabled, false)

  toggle.click()
  await nextTask()

  assert.equal(created[0].label, "annotation-session-1")
  assert.match(created[0].options.url, /nativeAnnotationToolbar=1/)
  assert.match(created[0].options.url, /sessionId=session-1/)
  assert.equal(created[0].options.transparent, true)
  assert.equal(created[0].options.decorations, false)
  assert.equal(created[0].options.width, 460)
  assert.equal(created[0].options.height, 64)
  assert.equal(toggle.getAttribute("aria-pressed"), "true")

  overlay.destroy()
  await nextTask()
  assert.equal(toggle.getAttribute("aria-pressed"), "false")
  assert.deepEqual(interactions, [["session-1", false]])
})

test("detaching closes the overlay and disables board input", async () => {
  const toggle = new FakeToggle()
  const interactions = []
  const overlay = fakeOverlayWindow()
  const controller = createNativeAnnotationToolbarWindow({
    toggle,
    createWindow: () => overlay,
    setInteraction: async (...args) => interactions.push(args),
  })

  controller.attach({ sessionId: "session-2", width: 1280, height: 720 })
  toggle.click()
  await nextTask()
  await controller.detach()

  assert.equal(overlay.closed, true)
  assert.equal(toggle.disabled, true)
  assert.equal(toggle.getAttribute("aria-pressed"), "false")
  assert.deepEqual(interactions, [["session-2", false]])
})

test("detaching still closes the toolbar when disabling native input fails", async () => {
  const toggle = new FakeToggle()
  const overlay = fakeOverlayWindow()
  const controller = createNativeAnnotationToolbarWindow({
    toggle,
    createWindow: () => overlay,
    setInteraction: async () => {
      throw new Error("disable failed")
    },
  })

  controller.attach({ sessionId: "session-3" })
  toggle.click()
  await nextTask()

  await assert.rejects(controller.detach(), /disable failed/)
  assert.equal(overlay.closed, true)
  assert.equal(toggle.disabled, true)
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
