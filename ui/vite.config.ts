import { readFileSync } from "node:fs";
import solid from "vite-plugin-solid";
import { defineConfig } from "vitest/config";

const host = process.env.TAURI_DEV_HOST;
// One version for the whole app: `tauri.conf.json` (kept in step with Cargo.toml).
const version: string = JSON.parse(
  readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
).version;

export default defineConfig({
  plugins: [solid()],
  define: { __APP_VERSION__: JSON.stringify(version) },
  // Tauri expects a fixed port and must see Rust errors in the terminal.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2022",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
  resolve: {
    conditions: ["development", "browser"],
  },
});
