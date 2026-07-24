import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

test("macOS annotation webview transparency is enabled", async () => {
  const configUrl = new URL("../src-tauri/tauri.conf.json", import.meta.url)
  const config = JSON.parse(await readFile(configUrl, "utf8"))

  assert.equal(config.app.macOSPrivateApi, true)
})
