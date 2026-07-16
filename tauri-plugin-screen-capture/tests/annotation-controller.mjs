import assert from 'node:assert/strict'

const writes = []
globalThis.window = {
  __TAURI_INTERNALS__: {
    invoke: async (_command, args) => {
      writes.push(args.document)
    },
  },
}

const { createAnnotationController } = await import('../dist-js/index.js')
const controller = createAnnotationController('session-without-dimensions')
const line = {
  id: 'line-1',
  kind: 'line',
  points: [
    { x: 0.1, y: 0.1 },
    { x: 0.2, y: 0.1 },
  ],
  color: { red: 255, green: 0, blue: 0, alpha: 255 },
  width: 0.01,
}

await controller.commitElement(line)
await controller.eraseAt({ x: 0.9, y: 0.9 })
assert.equal(controller.document.elements.length, 1, 'a distant eraser must not hit by default')

await controller.eraseAt({ x: 0.15, y: 0.1 })
assert.equal(controller.document.elements.length, 0, 'an eraser on the line should remove it')
assert.equal(writes.at(-1).elements.length, 0)

await controller.commitElement(line)
await controller.beginElement({
  ...line,
  id: 'unfinished',
  points: [{ x: 0.4, y: 0.4 }],
})
assert.equal(controller.document.elements.length, 2)
await controller.cancelElement()
assert.deepEqual(controller.document.elements.map((element) => element.id), ['line-1'])
