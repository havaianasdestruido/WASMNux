import { defineConfig } from "vite";

// COOP/COEP enable cross-origin isolation (SharedArrayBuffer) when the
// deployment allows it; the app never requires it (capability detection
// reports it and threaded WASM stays an optional upgrade).
const isolationHeaders = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

export default defineConfig({
  server: {
    host: "0.0.0.0",
    port: 5173,
    allowedHosts: true,
    headers: isolationHeaders,
  },
  preview: {
    host: "0.0.0.0",
    port: 4173,
    allowedHosts: true,
    headers: isolationHeaders,
  },
  worker: {
    format: "es",
  },
  build: {
    target: "es2022",
    sourcemap: true,
  },
});
