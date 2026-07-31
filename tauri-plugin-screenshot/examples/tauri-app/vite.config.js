import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createRequire } from "node:module";
import { fileURLToPath, URL } from "node:url";

const host = process.env.TAURI_DEV_HOST;
const require = createRequire(import.meta.url);

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  resolve: {
    alias: {
      "@tauri-apps/api/core": require.resolve("@tauri-apps/api/core"),
      "tauri-plugin-screenshot-api": fileURLToPath(
        new URL("../../guest-js/index.ts", import.meta.url),
      ),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  // prevent Vite from obscuring rust errors
  clearScreen: false,
  // tauri expects a fixed port, fail if that port is not available
  server: {
    host: host || "127.0.0.1",
    port: 1420,
    strictPort: true,
    hmr: host ? {
      protocol: 'ws',
      host,
      port: 1421
    } : undefined,
  },
})
