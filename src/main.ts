import './styles.css';

import { FramebufferDisplayController } from './gpu/display-controller';
import { BootOrchestrator } from './runtime/boot-orchestrator';
import { detectBrowserCapabilities } from './runtime/capabilities';
import { VmClient } from './runtime/vm-client';
import { crc32 } from './storage/crc32';
import { PersistentStorageEngine } from './storage/storage-engine';
import { AppShell } from './ui/app-shell';
import { WasmVmRuntime } from './wasm/bindings';
import { buildRiscvMiniSmokeKernel } from './wasm/test-mini-kernel';

declare global {
  interface Window {
    __WASMNUX_STORAGE_API__?: {
      readonly createAuto: typeof PersistentStorageEngine.createAuto;
      readonly crc32: typeof crc32;
    };
    __WASMNUX_BOOT_ORCHESTRATOR_CTOR__?: typeof BootOrchestrator;
    __WASMNUX_DISPLAY_CONTROLLER_CTOR__?: typeof FramebufferDisplayController;
    __WASMNUX_APP_SHELL__?: AppShell;
  }
}

if (typeof window !== 'undefined') {
  window.__WASMNUX_STORAGE_API__ = {
    createAuto: (opts) => PersistentStorageEngine.createAuto(opts),
    crc32,
  };
  window.__WASMNUX_BOOT_ORCHESTRATOR_CTOR__ = BootOrchestrator;
  window.__WASMNUX_DISPLAY_CONTROLLER_CTOR__ = FramebufferDisplayController;
}

async function runWorkerSmokeCheck(wasmUrl: string): Promise<{
  readonly transportMode: 'worker' | 'cooperative-fallback';
  readonly serialBanner: string;
  readonly dirtyScanlineObserved: boolean;
}> {
  let serialBanner = '';
  let dirtyScanlineObserved = false;
  const decoder = new TextDecoder();

  let resolveDone: (() => void) | null = null;
  let rejectDone: ((err: Error) => void) | null = null;
  const donePromise = new Promise<void>((res, rej) => {
    resolveDone = res;
    rejectDone = rej;
  });

  const client = new VmClient({
    onSerialOutput: (bytes) => {
      serialBanner += decoder.decode(bytes);
    },
    onFramebufferDirty: (upd) => {
      if (upd.yMin <= 4 && upd.yMax >= 5) {
        const rowOffset = (4 - upd.yMin) * upd.width * 4;
        const dv = new DataView(upd.pixels.buffer, upd.pixels.byteOffset, upd.pixels.byteLength);
        if (dv.getUint32(rowOffset, true) === 0x0034_d000) {
          dirtyScanlineObserved = true;
        }
      }
    },
    onLifecycleChange: (state) => {
      if (state === 'powered_off') {
        resolveDone?.();
      }
    },
    onError: (err) => {
      rejectDone?.(err);
    },
  });

  try {
    await client.initVm({
      wasmUrl,
      profileId: 'worker-smoke',
      ramMib: 16,
      diskMib: 4,
    });
    await client.bootVm({
      kernelImage: buildRiscvMiniSmokeKernel('WORKER-RV64-OK'),
      bootargs: 'console=ttyS0',
      sliceSteps: 10_000,
    });
    await donePromise;
    await client.destroyVm();
    return {
      transportMode: client.transportMode,
      serialBanner,
      dirtyScanlineObserved,
    };
  } finally {
    client.dispose();
  }
}

function registerPwaServiceWorker(): void {
  if (typeof navigator !== 'undefined' && 'serviceWorker' in navigator) {
    window.addEventListener('load', () => {
      void navigator.serviceWorker.register('./sw.js').catch(() => {
        // Ignore ServiceWorker registration failure in restricted/file contexts
      });
    });
  }
}

async function bootstrap(): Promise<void> {
  const root = document.getElementById('app');
  if (!root) {
    return;
  }

  registerPwaServiceWorker();
  const caps = await detectBrowserCapabilities();
  const wasmUrl = new URL('./wasm/wasmnux_wasm.wasm', window.location.href).href;
  const rt = await WasmVmRuntime.fromUrl(wasmUrl);
  const abiVersion = rt.abiVersion;
  const workerSmoke = await runWorkerSmokeCheck(wasmUrl);

  const shell = new AppShell(root, {
    abiVersion,
    wasmUrl,
    capabilities: caps,
    workerSmoke,
  });
  await shell.mount();
  window.__WASMNUX_APP_SHELL__ = shell;
}

void bootstrap();
