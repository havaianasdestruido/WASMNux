/**
 * Browser capability detection. Every optional platform feature the VM can
 * use (WebGPU, OPFS, SharedArrayBuffer, persistence) is detected here and
 * surfaced honestly in the UI. Nothing in the app assumes a capability
 * exists without going through this module first.
 */

export interface Capabilities {
  /** WebAssembly available at all (the VM cannot run without it). */
  readonly webAssembly: boolean;
  /** Web Workers (the VM runs in one to keep the UI thread free). */
  readonly webWorkers: boolean;
  /** WebGPU API object present (adapter usability is probed on demand). */
  readonly webGpuApi: boolean;
  /** Origin Private File System entry point present. */
  readonly opfs: boolean;
  /** IndexedDB present (metadata + storage fallback). */
  readonly indexedDb: boolean;
  /** SharedArrayBuffer constructor exposed (needs cross-origin isolation). */
  readonly sharedArrayBuffer: boolean;
  /** Page is cross-origin isolated. */
  readonly crossOriginIsolated: boolean;
  /** Service workers available (PWA offline shell). */
  readonly serviceWorker: boolean;
  /** `navigator.storage.persist` API present. */
  readonly storagePersistApi: boolean;
}

interface EnvironmentProbe {
  readonly webAssembly: unknown;
  readonly worker: unknown;
  readonly gpu: unknown;
  readonly storage: { getDirectory?: unknown; persist?: unknown } | undefined;
  readonly indexedDB: unknown;
  readonly sharedArrayBuffer: unknown;
  readonly crossOriginIsolated: boolean;
  readonly serviceWorker: unknown;
}

/** Pure capability computation from probe values (unit-testable). */
export function computeCapabilities(probe: EnvironmentProbe): Capabilities {
  return {
    webAssembly: typeof probe.webAssembly === "object" && probe.webAssembly !== null,
    webWorkers: typeof probe.worker === "function",
    webGpuApi: typeof probe.gpu === "object" && probe.gpu !== null,
    opfs: typeof probe.storage?.getDirectory === "function",
    indexedDb: typeof probe.indexedDB === "object" && probe.indexedDB !== null,
    sharedArrayBuffer: typeof probe.sharedArrayBuffer === "function",
    crossOriginIsolated: probe.crossOriginIsolated,
    serviceWorker: typeof probe.serviceWorker === "object" && probe.serviceWorker !== null,
    storagePersistApi: typeof probe.storage?.persist === "function",
  };
}

/** Detects capabilities of the current browser environment. */
export function detectCapabilities(): Capabilities {
  const nav: Navigator | undefined = typeof navigator === "undefined" ? undefined : navigator;
  return computeCapabilities({
    webAssembly: typeof WebAssembly === "undefined" ? undefined : WebAssembly,
    worker: typeof Worker === "undefined" ? undefined : Worker,
    gpu: nav?.gpu,
    storage: nav?.storage,
    indexedDB: typeof indexedDB === "undefined" ? undefined : indexedDB,
    sharedArrayBuffer: typeof SharedArrayBuffer === "undefined" ? undefined : SharedArrayBuffer,
    crossOriginIsolated: typeof crossOriginIsolated === "boolean" ? crossOriginIsolated : false,
    serviceWorker: nav?.serviceWorker,
  });
}

export interface StorageSnapshot {
  /** Whether the origin's storage is persisted (eviction-resistant). */
  readonly persisted: boolean | undefined;
  /** Estimated bytes used by this origin. */
  readonly usageBytes: number | undefined;
  /** Estimated quota for this origin. */
  readonly quotaBytes: number | undefined;
}

/** Reads current persistence/quota state; undefined when the API is absent. */
export async function readStorageSnapshot(): Promise<StorageSnapshot> {
  const storage: StorageManager | undefined =
    typeof navigator === "undefined" ? undefined : navigator.storage;
  if (storage === undefined) {
    return { persisted: undefined, usageBytes: undefined, quotaBytes: undefined };
  }
  const persisted =
    typeof storage.persisted === "function" ? await storage.persisted() : undefined;
  const estimate =
    typeof storage.estimate === "function" ? await storage.estimate() : undefined;
  return {
    persisted,
    usageBytes: estimate?.usage,
    quotaBytes: estimate?.quota,
  };
}

/**
 * Requests persistent storage. Returns the granted state, or `undefined`
 * when the browser does not expose the API. Never throws away the answer:
 * a `false` result is shown in the UI as an eviction risk.
 */
export async function requestPersistence(): Promise<boolean | undefined> {
  const storage: StorageManager | undefined =
    typeof navigator === "undefined" ? undefined : navigator.storage;
  if (storage === undefined || typeof storage.persist !== "function") {
    return undefined;
  }
  return storage.persist();
}

/** Probes whether a WebGPU adapter+device can actually be acquired. */
export async function probeWebGpu(): Promise<{ ok: true } | { ok: false; reason: string }> {
  if (typeof navigator === "undefined" || navigator.gpu === undefined) {
    return { ok: false, reason: "navigator.gpu is not exposed by this browser" };
  }
  try {
    const adapter = await navigator.gpu.requestAdapter();
    if (adapter === null) {
      return { ok: false, reason: "requestAdapter() returned null (no suitable GPU)" };
    }
    const device = await adapter.requestDevice();
    device.destroy();
    return { ok: true };
  } catch (error) {
    return { ok: false, reason: error instanceof Error ? error.message : String(error) };
  }
}
