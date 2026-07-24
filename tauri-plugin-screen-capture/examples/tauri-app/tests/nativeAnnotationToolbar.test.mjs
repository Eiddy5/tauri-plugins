import assert from "node:assert/strict"
import test from "node:test"

import { createNativeAnnotationToolbar } from "../src/lib/nativeAnnotationToolbar.js"

class FakeElement extends EventTarget {
  constructor(value = "") {
    super()
    this.value = value
    this.disabled = false
    this.attributes = new Map()
    this.classList = {
      toggle() {},
    }
  }

  click() {
    this.dispatchEvent(new Event("click"))
  }

  change() {
    this.dispatchEvent(new Event("change"))
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value))
  }
}

test("native toolbar enables Rust input and routes every action through plugin commands", async () => {
  const calls = []
  const elements = {
    tools: [tool("pen"), tool("eraser")],
    color: new FakeElement("#34c759"),
    undo: new FakeElement(),
    clear: new FakeElement(),
    close: new FakeElement(),
  }
  const toolbar = createNativeAnnotationToolbar({
    sessionId: "session-1",
    elements,
    setInteraction: async (...args) => calls.push(["interaction", ...args]),
    setTool: async (...args) => calls.push(["tool", ...args]),
    undo: async (...args) => calls.push(["undo", ...args]),
    clear: async (...args) => calls.push(["clear", ...args]),
    closeWindow: async () => calls.push(["close"]),
  })

  await toolbar.start()
  assert.deepEqual(calls.slice(0, 2), [
    ["tool", "session-1", { kind: "pen", color: "#34c759", width: 4 }],
    ["interaction", "session-1", true],
  ])

  elements.tools[1].click()
  await nextTask()
  elements.undo.click()
  elements.clear.click()
  await nextTask()

  assert.deepEqual(calls.slice(2), [
    ["tool", "session-1", { kind: "eraser", width: 24 }],
    ["undo", "session-1"],
    ["clear", "session-1"],
  ])

  await toolbar.stop()
  await toolbar.stop()
  assert.deepEqual(calls.slice(-1), [["interaction", "session-1", false]])
})

test("closing the native toolbar disables Rust input before closing its window", async () => {
  const calls = []
  const elements = {
    tools: [tool("pen"), tool("eraser")],
    color: new FakeElement("#ff3b30"),
    undo: new FakeElement(),
    clear: new FakeElement(),
    close: new FakeElement(),
  }
  const toolbar = createNativeAnnotationToolbar({
    sessionId: "session-2",
    elements,
    setInteraction: async (...args) => calls.push(["interaction", ...args]),
    setTool: async () => {},
    undo: async () => {},
    clear: async () => {},
    closeWindow: async () => calls.push(["close"]),
  })

  await toolbar.start()
  elements.close.click()
  await nextTask()

  assert.deepEqual(calls, [
    ["interaction", "session-2", true],
    ["interaction", "session-2", false],
    ["close"],
  ])
})

test("closing the native toolbar still closes its window when disabling Rust input fails", async () => {
  const elements = {
    tools: [tool("pen")],
    color: new FakeElement("#ff3b30"),
    close: new FakeElement(),
  }
  let closed = false
  const errors = []
  const toolbar = createNativeAnnotationToolbar({
    sessionId: "session-3",
    elements,
    setInteraction: async (_sessionId, enabled) => {
      if (!enabled) throw new Error("disable failed")
    },
    setTool: async () => {},
    undo: async () => {},
    clear: async () => {},
    closeWindow: async () => {
      closed = true
    },
    onError: (error) => errors.push(error.message),
  })

  await toolbar.start()
  elements.close.click()
  await nextTask()

  assert.equal(closed, true)
  assert.deepEqual(errors, ["disable failed"])
})

function tool(kind) {
  const element = new FakeElement()
  element.dataset = { nativeTool: kind }
  return element
}

const nextTask = () => new Promise((resolve) => setImmediate(resolve))
