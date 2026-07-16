import assert from "node:assert/strict"
import test from "node:test"

import {
  applyAnnotationTargetGeometry,
  createAnnotationTargetSynchronizer,
} from "../src/lib/annotationTargetGeometry.js"

test("physical target bounds move and size the overlay in physical pixels", async () => {
  const calls = []
  const window = fakeWindow(calls)
  const dpi = fakeDpi()

  await applyAnnotationTargetGeometry(
    window,
    { x: -1920, y: 0, width: 1920, height: 1080, coordinateSpace: "physical" },
    dpi,
  )

  assert.deepEqual(calls, [
    ["position", { kind: "physical-position", x: -1920, y: 0 }],
    ["size", { kind: "physical-size", width: 1920, height: 1080 }],
    ["show"],
  ])
})

test("logical target bounds use logical units and missing targets hide input", async () => {
  const calls = []
  const window = fakeWindow(calls)
  const dpi = fakeDpi()

  await applyAnnotationTargetGeometry(
    window,
    { x: 10, y: 20, width: 800, height: 600, coordinateSpace: "logical" },
    dpi,
  )
  await applyAnnotationTargetGeometry(window, null, dpi)

  assert.deepEqual(calls, [
    ["position", { kind: "logical-position", x: 10, y: 20 }],
    ["size", { kind: "logical-size", width: 800, height: 600 }],
    ["show"],
    ["hide"],
  ])
})

test("target synchronization focuses once and resets focus after hiding", async () => {
  const calls = []
  const window = fakeWindow(calls)
  const sync = createAnnotationTargetSynchronizer(window, fakeDpi())
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
