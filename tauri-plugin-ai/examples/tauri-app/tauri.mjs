import { createRequire } from 'node:module'

// Keep the official CLI's argument handling and process lifecycle.
const require = createRequire(import.meta.url)
require('@tauri-apps/cli/tauri.js')
