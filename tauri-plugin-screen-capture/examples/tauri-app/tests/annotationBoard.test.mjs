import assert from "node:assert/strict"
import test from "node:test"

import { createAnnotationBoard, mapPointerToVideo } from "../src/lib/annotationBoard.js"

class FakeElement extends EventTarget {
  constructor() {
    super()
    this.disabled = false
    this.hidden = false
    this.dataset = {}
    this.attributes = new Map()
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

  getBoundingClientRect() {
    return { left: 0, top: 0, width: 1000, height: 1000 }
  }

  setPointerCapture() {}

  releasePointerCapture() {}
}

const nextTask = () => new Promise((resolve) => setImmediate(resolve))

test("user can enable and disable the annotation board for an active share", async () => {
  const toggle = new FakeElement()
  const toolbar = new FakeElement()
  const canvas = new FakeElement()
  const visibility = []
  const board = createAnnotationBoard({
    elements: { toggle, toolbar, canvas },
    createController: () => ({
      setVisible: async (visible) => visibility.push(visible),
    }),
  })

  assert.equal(toggle.disabled, true)
  assert.equal(toolbar.hidden, true)
  assert.equal(canvas.hidden, true)

  board.attach({ sessionId: "session-1", width: 1920, height: 1080 })
  assert.equal(toggle.disabled, false)

  toggle.click()
  await nextTask()
  assert.deepEqual(visibility, [true])
  assert.equal(toggle.getAttribute("aria-pressed"), "true")
  assert.equal(toolbar.hidden, false)
  assert.equal(canvas.hidden, false)

  toggle.click()
  await nextTask()
  assert.deepEqual(visibility, [true, false])
  assert.equal(toggle.getAttribute("aria-pressed"), "false")
  assert.equal(toolbar.hidden, true)
  assert.equal(canvas.hidden, true)
})

test("pointer coordinates map only to the contained video image", () => {
  const bounds = { left: 0, top: 0, width: 1000, height: 1000 }
  const videoSize = { width: 1920, height: 1080 }

  assert.deepEqual(mapPointerToVideo({ clientX: 500, clientY: 500 }, bounds, videoSize), {
    x: 0.5,
    y: 0.5,
  })
  assert.deepEqual(mapPointerToVideo({ clientX: 250, clientY: 218.75 }, bounds, videoSize), {
    x: 0.25,
    y: 0,
  })
  assert.equal(mapPointerToVideo({ clientX: 500, clientY: 100 }, bounds, videoSize), null)
})

test("pen gestures submit normalized draft points to the annotation controller", async () => {
  const toggle = new FakeElement()
  const toolbar = new FakeElement()
  const canvas = new FakeElement()
  const calls = []
  const board = createAnnotationBoard({
    elements: { toggle, toolbar, canvas },
    createController: () => ({
      setVisible: async () => {},
      beginElement: async (element) => calls.push(["begin", structuredClone(element)]),
      updateElement: async (element) => calls.push(["update", structuredClone(element)]),
      commitElement: async (element) => calls.push(["commit", structuredClone(element)]),
    }),
  })
  board.attach({ sessionId: "session-1", width: 1920, height: 1080 })
  await board.setEnabled(true)

  canvas.dispatchEvent(pointerEvent("pointerdown", 500, 500, 1))
  canvas.dispatchEvent(pointerEvent("pointermove", 600, 500, 1))
  canvas.dispatchEvent(pointerEvent("pointerup", 700, 500, 1))
  await nextTask()

  assert.deepEqual(
    calls.map(([operation, element]) => [operation, element.kind, element.points]),
    [
      ["begin", "pen", [{ x: 0.5, y: 0.5 }]],
      ["update", "pen", [{ x: 0.5, y: 0.5 }, { x: 0.6, y: 0.5 }]],
      [
        "commit",
        "pen",
        [
          { x: 0.5, y: 0.5 },
          { x: 0.6, y: 0.5 },
          { x: 0.7, y: 0.5 },
        ],
      ],
    ],
  )
})

test("shape gestures publish a valid two-point draft from pointer down", async () => {
  const toggle = new FakeElement()
  const toolbar = new FakeElement()
  const canvas = new FakeElement()
  const lineTool = new FakeElement()
  lineTool.dataset.tool = "line"
  const drafts = []
  const board = createAnnotationBoard({
    elements: { toggle, toolbar, canvas, tools: [lineTool] },
    createController: () => ({
      setVisible: async () => {},
      beginElement: async (element) => drafts.push(structuredClone(element)),
      updateElement: async () => {},
      commitElement: async () => {},
    }),
  })
  board.attach({ sessionId: "session-1", width: 1920, height: 1080 })
  await board.setEnabled(true)
  lineTool.click()

  canvas.dispatchEvent(pointerEvent("pointerdown", 500, 500, 1))
  await nextTask()

  assert.deepEqual(drafts[0].points, [
    { x: 0.5, y: 0.5 },
    { x: 0.5, y: 0.5 },
  ])
})

function pointerEvent(type, clientX, clientY, pointerId) {
  const event = new Event(type)
  Object.assign(event, { clientX, clientY, pointerId, button: 0 })
  return event
}
