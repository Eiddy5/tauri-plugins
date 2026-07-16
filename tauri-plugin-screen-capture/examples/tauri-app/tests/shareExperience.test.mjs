import assert from "node:assert/strict"
import test from "node:test"

import { shareExperienceMode } from "../src/lib/shareExperience.js"

test("share experience moves from landing to picker to immersive sharing", () => {
  assert.equal(shareExperienceMode({ pickerOpen: false, session: null }), "landing")
  assert.equal(shareExperienceMode({ pickerOpen: true, session: null }), "picker")
  assert.equal(
    shareExperienceMode({ pickerOpen: true, session: { sessionId: "session-1" } }),
    "sharing",
  )
})
