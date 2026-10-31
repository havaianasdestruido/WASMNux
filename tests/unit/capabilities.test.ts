import { describe, expect, it } from 'vitest';
import {
  detectBrowserCapabilities,
  requestPersistentStorage,
} from '../../src/runtime/capabilities';

describe('detectBrowserCapabilities', () => {
  it('selects webgpu and opfs when all modern browser capabilities are available', async () => {
    let destroyed = false;
    const fakeGpu = {
      requestAdapter: () =>
        Promise.resolve({
          info: {
            vendor: 'mesa',
            architecture: 'llvmpipe',
            device: 'CPU',
            description: 'Software Rasterizer',
          },
          requestDevice: () =>
            Promise.resolve({
              destroy: () => {
                destroyed = true;
              },
            }),
        }),
    } as unknown as GPU;

    const fakeStorage = {
      getDirectory: () => Promise.resolve({}),
      persist: () => Promise.resolve(true),
      persisted: () => Promise.resolve(true),
      estimate: () => Promise.resolve({ usage: 10 * 1024 * 1024, quota: 1024 * 1024 * 1024 }),
    } as unknown as StorageManager;

    const report = await detectBrowserCapabilities({
      hasWebAssembly: true,
      hasWorker: true,
      hasSharedArrayBuffer: true,
      crossOriginIsolated: true,
      gpu: fakeGpu,
      createCanvas2d: () => true,
      storage: fakeStorage,
      hasIndexedDb: true,
      hasClipboard: true,
      hasServiceWorker: true,
    });

    expect(destroyed).toBe(true);
    expect(report.webAssembly).toBe(true);
    expect(report.webWorker).toBe(true);
    expect(report.sharedArrayBuffer).toBe(true);
    expect(report.webGpuAvailable).toBe(true);
    expect(report.webGpuAdapterDescription).toContain('mesa');
    expect(report.preferredGraphicsBackend).toBe('webgpu');
    expect(report.opfsAvailable).toBe(true);
    expect(report.preferredStorageBackend).toBe('opfs');
    expect(report.storagePersisted).toBe(true);
    expect(report.quota?.availableBytes).toBe((1024 - 10) * 1024 * 1024);
    expect(report.warnings).toHaveLength(0);
  });

  it('falls back cleanly to canvas2d and indexeddb when WebGPU, OPFS, and COOP/COEP are missing', async () => {
    const fakeStorage = {
      getDirectory: () => Promise.reject(new Error('SecurityError')),
      persist: () => Promise.resolve(false),
      persisted: () => Promise.resolve(false),
      estimate: () => Promise.resolve({ usage: 50 * 1024 * 1024, quota: 64 * 1024 * 1024 }),
    } as unknown as StorageManager;

    const report = await detectBrowserCapabilities({
      hasWebAssembly: true,
      hasWorker: true,
      hasSharedArrayBuffer: true,
      crossOriginIsolated: false,
      gpu: null,
      createCanvas2d: () => true,
      storage: fakeStorage,
      hasIndexedDb: true,
      hasClipboard: false,
      hasServiceWorker: false,
    });

    expect(report.sharedArrayBuffer).toBe(false);
    expect(report.webGpuAvailable).toBe(false);
    expect(report.preferredGraphicsBackend).toBe('canvas2d');
    expect(report.opfsAvailable).toBe(false);
    expect(report.preferredStorageBackend).toBe('indexeddb');
    expect(report.warnings.some((w) => w.includes('Canvas 2D'))).toBe(true);
    expect(report.warnings.some((w) => w.includes('64 MiB'))).toBe(true);
  });

  it('handles storage.persist() rejection safely without throwing', async () => {
    const brokenStorage = {
      persist: () => Promise.reject(new Error('Permission denied')),
    } as unknown as StorageManager;
    expect(await requestPersistentStorage(brokenStorage)).toBe(false);
    expect(await requestPersistentStorage(null)).toBe(false);
  });
});
