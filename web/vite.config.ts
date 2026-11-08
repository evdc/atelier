/// <reference types="vitest/config" />
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// In `pnpm dev` (browser or `cargo tauri dev`), connect to the local atelierd
// using the token it wrote on first run. Production builds get the server
// URL and token from the host (Tauri) at runtime instead.
function devServer() {
  const home = process.env.ATELIER_HOME ?? join(homedir(), ".atelier");
  let token = "";
  try {
    token = readFileSync(join(home, "token"), "utf8").trim();
  } catch {
    // atelierd hasn't run yet; the UI will show it can't connect.
  }
  const url = process.env.ATELIER_URL ?? "ws://127.0.0.1:7747/ws";
  return { url, token };
}

export default defineConfig(({ command }) => ({
  plugins: [solid()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  define: {
    __ATELIER_DEV__: command === "serve" ? JSON.stringify(devServer()) : "null",
  },
  build: { target: "safari16", sourcemap: true },
  // Folds and models are pure; UI is exercised in the running app.
  test: { environment: "node" },
}));
