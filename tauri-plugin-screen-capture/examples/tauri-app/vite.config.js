import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

const host = process.env.TAURI_DEV_HOST;
const appRoot = fileURLToPath(new URL(".", import.meta.url));
const pluginRoot = fileURLToPath(new URL("../..", import.meta.url));

// https://vite.dev/config/
export default defineConfig({
  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  // prevent Vite from obscuring rust errors
  clearScreen: false,
  // tauri expects a fixed port, fail if that port is not available
  server: {
    host: host || false,
    port: 1420,
    strictPort: true,
    hmr: host ? {
      protocol: 'ws',
      host,
      port: 1421
    } : undefined,
    fs: {
      allow: [appRoot, pluginRoot],
    },
  },
  resolve: {
    alias: {
      "@tauri-apps/api": fileURLToPath(
        new URL("./node_modules/@tauri-apps/api", import.meta.url)
      ),
      "tauri-plugin-screen-capture-api": fileURLToPath(
        new URL("../../guest-js/index.ts", import.meta.url)
      ),
    },
  },
})
