import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

// Standard Tauri + Vite setup: fixed port so tauri.conf.json's devUrl can
// point at it reliably, HMR over a distinct port so it survives Tauri's
// window reload, and ignoring src-tauri/ so Rust rebuilds don't trigger
// pointless Vite rewatch cycles.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    // This app only ships on Windows (WebView2/Chromium) - no need to guess
    // a target from Tauri's env vars, which also aren't set outside `tauri
    // build`/`tauri dev` (a plain `npm run build` was hitting the safari13
    // fallback and tripping an unrelated esbuild destructuring-transform bug).
    target: "chrome105",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    // Three entry points: the React main window, the small hover-overlay
    // popup kept as a plain static page (see overlay.html), and the in-call
    // copilot bar (copilot.html, React).
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        overlay: fileURLToPath(new URL("./overlay.html", import.meta.url)),
        copilot: fileURLToPath(new URL("./copilot.html", import.meta.url)),
        shield: fileURLToPath(new URL("./shield.html", import.meta.url)),
      },
    },
  },
});
