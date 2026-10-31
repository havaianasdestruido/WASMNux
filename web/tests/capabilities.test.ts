import { describe, expect, it } from "vitest";

import { computeCapabilities } from "../src/capabilities.js";

describe("computeCapabilities", () => {
  it("reports a fully capable browser", () => {
    const caps = computeCapabilities({
      webAssembly: {},
      worker: () => undefined,
      gpu: {},
      storage: { getDirectory: () => undefined, persist: () => undefined },
      indexedDB: {},
      sharedArrayBuffer: () => undefined,
      crossOriginIsolated: true,
      serviceWorker: {},
    });
    expect(caps).toEqual({
      webAssembly: true,
      webWorkers: true,
      webGpuApi: true,
      opfs: true,
      indexedDb: true,
      sharedArrayBuffer: true,
      crossOriginIsolated: true,
      serviceWorker: true,
      storagePersistApi: true,
    });
  });

  it("reports a minimal environment without claiming optional features", () => {
    const caps = computeCapabilities({
      webAssembly: undefined,
      worker: undefined,
      gpu: undefined,
      storage: undefined,
      indexedDB: undefined,
      sharedArrayBuffer: undefined,
      crossOriginIsolated: false,
      serviceWorker: undefined,
    });
    expect(Object.values(caps).every((v) => v === false)).toBe(true);
  });

  it("does not confuse presence of storage without OPFS", () => {
    const caps = computeCapabilities({
      webAssembly: {},
      worker: undefined,
      gpu: undefined,
      storage: { persist: () => undefined },
      indexedDB: undefined,
      sharedArrayBuffer: undefined,
      crossOriginIsolated: false,
      serviceWorker: undefined,
    });
    expect(caps.opfs).toBe(false);
    expect(caps.storagePersistApi).toBe(true);
  });
});
