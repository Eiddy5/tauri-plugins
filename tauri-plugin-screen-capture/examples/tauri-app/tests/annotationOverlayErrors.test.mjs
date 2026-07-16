import assert from "node:assert/strict"
import test from "node:test"

import { createAnnotationOverlayErrorPolicy } from "../src/lib/annotationOverlayErrors.js"

test("drawing errors stay visible without closing the board", async () => {
  const output = { hidden: true, textContent: "" }
  const logged = []
  let closes = 0
  const errors = createAnnotationOverlayErrorPolicy({
    output,
    log: (error) => logged.push(error.message),
    closeOverlay: async () => { closes += 1 },
  })

  errors.reportBoardError(new Error("draft rejected"))

  assert.equal(closes, 0)
  assert.equal(output.hidden, false)
  assert.equal(output.textContent, "draft rejected")
  assert.deepEqual(logged, ["draft rejected"])

  await errors.closeFatal(new Error("session ended"))
  assert.equal(closes, 1)
})
