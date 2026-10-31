export interface BrowserCapabilities {
  secureContext: boolean;
  webAssembly: boolean;
  worker: boolean;
  indexedDB: boolean;
  opfs: boolean;
  persistentStorage: boolean;
  webGPU: boolean;
  sharedArrayBuffer: boolean;
}

/** Feature availability only: this does not claim that an adapter, permission or device works. */
export function detectCapabilities(): BrowserCapabilities {
  return {
    secureContext: globalThis.isSecureContext === true,
    webAssembly: typeof WebAssembly === "object",
    worker: typeof Worker === "function",
    indexedDB: typeof indexedDB !== "undefined",
    opfs: typeof navigator !== "undefined" && typeof navigator.storage?.getDirectory === "function",
    persistentStorage:
      typeof navigator !== "undefined" && typeof navigator.storage?.persisted === "function",
    webGPU: typeof navigator !== "undefined" && navigator.gpu != null,
    sharedArrayBuffer:
      typeof SharedArrayBuffer === "function" && globalThis.crossOriginIsolated === true,
  };
}

export type PersistenceStatus = "persisted" | "best-effort" | "unavailable" | "error";

/** Query only; requesting durable storage requires an explicit user gesture. */
export async function queryPersistence(): Promise<PersistenceStatus> {
  if (typeof navigator === "undefined" || typeof navigator.storage?.persisted !== "function") {
    return "unavailable";
  }
  try {
    return (await navigator.storage.persisted()) ? "persisted" : "best-effort";
  } catch {
    return "error";
  }
}
