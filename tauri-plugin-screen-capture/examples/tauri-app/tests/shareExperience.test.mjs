import assert from "node:assert/strict"
import test from "node:test"

import {
  returnToPickerAfterSharing,
  shareExperienceMode,
} from "../src/lib/shareExperience.js"

test("share experience moves from landing to picker to immersive sharing", () => {
  assert.equal(shareExperienceMode({ pickerOpen: false, session: null }), "landing")
  assert.equal(shareExperienceMode({ pickerOpen: true, session: null }), "picker")
  assert.equal(
    shareExperienceMode({ pickerOpen: true, session: { sessionId: "session-1" } }),
    "sharing",
  )
})

test("ending a share returns to the populated source picker", () => {
  const state = {
    sources: [{ id: "display:1", kind: "display" }],
    selected: { id: "display:1", kind: "display" },
    activeKind: "display",
    pickerOpen: false,
    session: { sessionId: "session-1" },
    stats: { framesCaptured: 12 },
    videoReady: true,
  }

  returnToPickerAfterSharing(state)

  assert.equal(state.pickerOpen, true)
  assert.equal(state.session, null)
  assert.equal(state.stats, null)
  assert.equal(state.videoReady, false)
  assert.equal(shareExperienceMode(state), "picker")
  assert.deepEqual(state.sources, [{ id: "display:1", kind: "display" }])
  assert.deepEqual(state.selected, { id: "display:1", kind: "display" })
  assert.equal(state.activeKind, "display")
})
