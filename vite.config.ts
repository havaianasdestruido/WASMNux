import { defineConfig } from "vitest/config";

export default defineConfig({
  // The Arena preview is proxied under a dynamic *.e2b.app hostname.
  server: { allowedHosts: [".e2b.app"] },
  test: { include: ["tests/**/*.test.ts"] },
});
