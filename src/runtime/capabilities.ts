export type GraphicsBackendKind = 'webgpu' | 'canvas2d' | 'none';
export type StorageBackendKind = 'opfs' | 'indexeddb' | 'memory';

export interface StorageQuotaReport {
  readonly usageBytes: number;
  readonly quotaBytes: number;
  readonly availableBytes: number;
  readonly usageRatio: number;
}

export interface BrowserCapabilities {
  readonly webAssembly: boolean;
  readonly webWorker: boolean;
  readonly sharedArrayBuffer: boolean;
  readonly crossOriginIsolated: boolean;
  readonly webGpuAvailable: boolean;
  readonly webGpuAdapterDescription: string | null;
  readonly canvas2dAvailable: boolean;
  readonly preferredGraphicsBackend: GraphicsBackendKind;
  readonly opfsAvailable: boolean;
  readonly indexedDbAvailable: boolean;
  readonly preferredStorageBackend: StorageBackendKind;
  readonly storagePersistSupported: boolean;
  readonly storagePersisted: boolean;
  readonly quota: StorageQuotaReport | null;
  readonly clipboardSupported: boolean;
  readonly serviceWorkerSupported: boolean;
  readonly warnings: readonly string[];
}

export interface CapabilityProbeEnv {
  readonly hasWebAssembly?: boolean;
  readonly hasWorker?: boolean;
  readonly hasSharedArrayBuffer?: boolean;
  readonly crossOriginIsolated?: boolean;
  readonly gpu?: GPU | null;
  readonly createCanvas2d?: () => boolean;
  readonly storage?: Partial<StorageManager> | null;
  readonly hasIndexedDb?: boolean;
  readonly hasClipboard?: boolean;
  readonly hasServiceWorker?: boolean;
}

function defaultCanvas2dCheck(): boolean {
  if (typeof document === 'undefined') {
    return false;
  }
  try {
    const canvas = document.createElement('canvas');
    canvas.width = 1;
    canvas.height = 1;
    return canvas.getContext('2d') !== null;
  } catch {
    return false;
  }
}

export async function detectBrowserCapabilities(
  overrides?: CapabilityProbeEnv,
): Promise<BrowserCapabilities> {
  const warnings: string[] = [];

  const webAssembly =
    overrides?.hasWebAssembly ??
    (typeof WebAssembly === 'object' && typeof WebAssembly.instantiate === 'function');
  if (!webAssembly) {
    warnings.push('WebAssembly is unavailable in this browser environment.');
  }

  const webWorker = overrides?.hasWorker ?? typeof Worker === 'function';
  if (!webWorker) {
    warnings.push(
      'Web Workers are unavailable; VM execution will fall back to cooperative slices.',
    );
  }

  const crossIsolated =
    overrides?.crossOriginIsolated ??
    (typeof globalThis.crossOriginIsolated === 'boolean' ? globalThis.crossOriginIsolated : false);

  const sharedArrayBuffer =
    (overrides?.hasSharedArrayBuffer ?? typeof SharedArrayBuffer === 'function') && crossIsolated;

  if (!sharedArrayBuffer) {
    warnings.push(
      'SharedArrayBuffer / Cross-Origin Isolation unavailable; using structured Worker message transport.',
    );
  }

  let webGpuAvailable = false;
  let webGpuAdapterDescription: string | null = null;
  const navGpu: GPU | null =
    overrides?.gpu !== undefined
      ? overrides.gpu
      : typeof navigator !== 'undefined' && 'gpu' in navigator
        ? navigator.gpu
        : null;

  if (navGpu !== null) {
    try {
      const adapter = await navGpu.requestAdapter();
      if (adapter !== null) {
        const device = await adapter.requestDevice();
        webGpuAvailable = true;
        const info = adapter.info;
        const parts = [info.vendor, info.architecture, info.device, info.description].filter(
          (item) => item.trim().length > 0,
        );
        webGpuAdapterDescription = parts.length > 0 ? parts.join(' / ') : 'WebGPU Adapter';
        device.destroy();
      }
    } catch {
      webGpuAvailable = false;
    }
  }

  const canvas2dAvailable = overrides?.createCanvas2d
    ? overrides.createCanvas2d()
    : defaultCanvas2dCheck();

  const preferredGraphicsBackend: GraphicsBackendKind = webGpuAvailable
    ? 'webgpu'
    : canvas2dAvailable
      ? 'canvas2d'
      : 'none';

  if (!webGpuAvailable) {
    warnings.push(
      canvas2dAvailable
        ? 'WebGPU unavailable; falling back to Canvas 2D framebuffer renderer.'
        : 'Neither WebGPU nor Canvas 2D is available; graphical framebuffer output disabled.',
    );
  }

  const storageMgr: Partial<StorageManager> | null =
    overrides?.storage !== undefined
      ? overrides.storage
      : typeof navigator !== 'undefined'
        ? navigator.storage
        : null;

  let opfsAvailable = false;
  if (storageMgr !== null && typeof storageMgr.getDirectory === 'function') {
    try {
      await storageMgr.getDirectory();
      opfsAvailable = true;
    } catch {
      opfsAvailable = false;
    }
  }

  const indexedDbAvailable =
    overrides?.hasIndexedDb ??
    (typeof indexedDB !== 'undefined' && typeof indexedDB.open === 'function');

  const preferredStorageBackend: StorageBackendKind = opfsAvailable
    ? 'opfs'
    : indexedDbAvailable
      ? 'indexeddb'
      : 'memory';

  if (preferredStorageBackend === 'memory') {
    warnings.push(
      'Persistent storage (OPFS and IndexedDB) unavailable; filesystem modifications will be volatile.',
    );
  }

  const persistFn = storageMgr?.persist;
  const persistedFn = storageMgr?.persisted;
  const storagePersistSupported =
    typeof persistFn === 'function' && typeof persistedFn === 'function';

  let storagePersisted = false;
  if (storageMgr !== null && typeof persistedFn === 'function') {
    try {
      storagePersisted = await persistedFn.call(storageMgr);
    } catch {
      storagePersisted = false;
    }
  }

  let quota: StorageQuotaReport | null = null;
  if (storageMgr !== null && typeof storageMgr.estimate === 'function') {
    try {
      const estimate = await storageMgr.estimate();
      const usageBytes = typeof estimate.usage === 'number' ? estimate.usage : 0;
      const quotaBytes = typeof estimate.quota === 'number' ? estimate.quota : 0;
      const availableBytes = Math.max(0, quotaBytes - usageBytes);
      const usageRatio = quotaBytes > 0 ? usageBytes / quotaBytes : 0;
      quota = {
        usageBytes,
        quotaBytes,
        availableBytes,
        usageRatio,
      };
      if (quotaBytes > 0 && availableBytes < 64 * 1024 * 1024) {
        warnings.push('Browser storage quota has fewer than 64 MiB remaining.');
      }
    } catch {
      quota = null;
    }
  }

  const clipboardSupported =
    overrides?.hasClipboard ??
    (typeof navigator !== 'undefined' &&
      'clipboard' in navigator &&
      typeof navigator.clipboard.writeText === 'function');

  const serviceWorkerSupported =
    overrides?.hasServiceWorker ??
    (typeof navigator !== 'undefined' && 'serviceWorker' in navigator);

  return {
    webAssembly,
    webWorker,
    sharedArrayBuffer,
    crossOriginIsolated: crossIsolated,
    webGpuAvailable,
    webGpuAdapterDescription,
    canvas2dAvailable,
    preferredGraphicsBackend,
    opfsAvailable,
    indexedDbAvailable,
    preferredStorageBackend,
    storagePersistSupported,
    storagePersisted,
    quota,
    clipboardSupported,
    serviceWorkerSupported,
    warnings,
  };
}

export async function requestPersistentStorage(
  storage?: Partial<StorageManager> | null,
): Promise<boolean> {
  const mgr: Partial<StorageManager> | null =
    storage !== undefined ? storage : typeof navigator !== 'undefined' ? navigator.storage : null;
  if (mgr === null || typeof mgr.persist !== 'function') {
    return false;
  }
  try {
    return await mgr.persist();
  } catch {
    return false;
  }
}
