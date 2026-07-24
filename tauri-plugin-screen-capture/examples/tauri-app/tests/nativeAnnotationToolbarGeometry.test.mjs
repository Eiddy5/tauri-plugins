import assert from "node:assert/strict"
import test from "node:test"

import {
  applyNativeAnnotationToolbarGeometry,
  createNativeAnnotationToolbarSynchronizer,
} from "../src/lib/nativeAnnotationToolbarGeometry.js"

test("physical target bounds center the native toolbar above the shared content", async () => {
  const calls = []
  const window = fakeWindow(calls)
  const dpi = fakeDpi()

  await applyNativeAnnotationToolbarGeometry(
    window,
    { x: -1920, y: 0, width: 1920, height: 1080, coordinateSpace: "physical" },
    dpi,
    { width: 460, height: 64, topInset: 12 },
  )

  assert.deepEqual(calls, [
    ["position", { kind: "physical-position", x: -1190, y: 12 }],
    ["size", { kind: "physical-size", width: 460, height: 64 }],
    ["show"],
  ])
})

test("logical target bounds use logical units and missing targets hide input", async () => {
  const calls = []
  const window = fakeWindow(calls)
  const dpi = fakeDpi()

  await applyNativeAnnotationToolbarGeometry(
    window,
    { x: 10, y: 20, width: 800, height: 600, coordinateSpace: "logical" },
    dpi,
    { width: 460, height: 64, topInset: 12 },
  )
  await applyNativeAnnotationToolbarGeometry(window, null, dpi, {
    width: 460,
    height: 64,
    topInset: 12,
  })

  assert.deepEqual(calls, [
    ["position", { kind: "logical-position", x: 180, y: 32 }],
    ["size", { kind: "logical-size", width: 460, height: 64 }],
    ["show"],
    ["hide"],
  ])
})

test("target synchronization focuses once and resets focus after hiding", async () => {
  const calls = []
  const window = fakeWindow(calls)
  const sync = createNativeAnnotationToolbarSynchronizer(window, fakeDpi(), {
    width: 460,
    height: 64,
    topInset: 12,
  })
  const target = {
    x: 10,
    y: 20,
    width: 800,
    height: 600,
    coordinateSpace: "logical",
  }

  await sync(target)
  await sync(target)
  await sync(null)
  await sync(target)

  assert.deepEqual(calls.filter(([kind]) => kind === "focus"), [
    ["focus"],
    ["focus"],
  ])
})

function fakeWindow(calls) {
  return {
    async setPosition(value) { calls.push(["position", { ...value }]) },
    async setSize(value) { calls.push(["size", { ...value }]) },
    async show() { calls.push(["show"]) },
    async hide() { calls.push(["hide"]) },
    async setFocus() { calls.push(["focus"]) },
  }
}

function fakeDpi() {
  return {
    PhysicalPosition: class {
      constructor(x, y) { Object.assign(this, { kind: "physical-position", x, y }) }
    },
    PhysicalSize: class {
      constructor(width, height) { Object.assign(this, { kind: "physical-size", width, height }) }
    },
    LogicalPosition: class {
      constructor(x, y) { Object.assign(this, { kind: "logical-position", x, y }) }
    },
    LogicalSize: class {
      constructor(width, height) { Object.assign(this, { kind: "logical-size", width, height }) }
    },
  }
}
