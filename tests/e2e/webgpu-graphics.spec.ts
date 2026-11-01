import { expect, test } from '@playwright/test';

test('renders real Linux 7.3.1 /dev/fb0 SimpleFB frames via WebGPU compute+render pipeline, recovers from GPU device loss, and handles resize', async ({
  page,
}) => {
  test.setTimeout(90_000);
  await page.goto('/');
  await expect(page.locator('main[data-ready="true"]')).toBeVisible();

  const result = await page.evaluate(async () => {
    const StorageApi = window.__WASMNUX_STORAGE_API__;
    const OrchestratorCtor = window.__WASMNUX_BOOT_ORCHESTRATOR_CTOR__;
    const DisplayCtor = window.__WASMNUX_DISPLAY_CONTROLLER_CTOR__;
    if (!StorageApi || !OrchestratorCtor || !DisplayCtor) {
      throw new Error('Missing browser runtime constructors');
    }

    const mount = document.createElement('div');
    mount.style.width = '640px';
    mount.style.height = '400px';
    document.body.appendChild(mount);

    const display = new DisplayCtor({ container: mount, preferredMode: 'webgpu' });
    const initialBackend = await display.init();

    const storage = await StorageApi.createAuto();
    let serialLog = '';
    const dec = new TextDecoder();

    const orchestrator = new OrchestratorCtor({
      storage,
      events: {
        onSerialOutput: (bytes) => {
          serialLog += dec.decode(bytes, { stream: true });
        },
        onFramebufferDirty: (upd) => {
          display.applyDirtyUpdate(upd);
        },
      },
    });

    const waitForText = async (needle: string, timeoutMs = 30_000): Promise<void> => {
      const deadline = performance.now() + timeoutMs;
      while (!serialLog.includes(needle)) {
        if (performance.now() > deadline) {
          throw new Error(`Timed out waiting for "${needle}". Tail:\n${serialLog.slice(-600)}`);
        }
        await new Promise((r) => setTimeout(r, 25));
      }
    };

    await orchestrator.bootProfile('vanilla');
    await waitForText('wasmnux-vanilla:~#');

    // Run full-screen /dev/fb0 test pattern inside booted Linux 7.3.1 guest
    await orchestrator.sendSerialInput('/usr/bin/wasmnux-fbtest && printf "FB_%s\\n" "PAINTED"\n');
    await waitForText('FB_PAINTED');
    await new Promise((r) => setTimeout(r, 50));

    // Read back GPU-converted rgba8unorm texture directly from WebGPU device!
    // Sample (x=100, y=0) outside fbcon tty1 cursor cell (0..7, 0..15): cyan accent border 0x0038BDF8
    const rgbaBeforeLoss = await display.readRgbaPixels();
    const topIdx = (0 * 640 + 100) * 4;
    const topPixelBefore = [
      rgbaBeforeLoss[topIdx] ?? -1,
      rgbaBeforeLoss[topIdx + 1] ?? -1,
      rgbaBeforeLoss[topIdx + 2] ?? -1,
      rgbaBeforeLoss[topIdx + 3] ?? -1,
    ];
    // Bottom SMPTE bar 0 (y=380, x=10) is 0x00EF4444 -> R=0xEF (239), G=0x44 (68), B=0x44 (68), A=255
    const barIdx = (380 * 640 + 10) * 4;
    const barPixelBefore = [
      rgbaBeforeLoss[barIdx] ?? -1,
      rgbaBeforeLoss[barIdx + 1] ?? -1,
      rgbaBeforeLoss[barIdx + 2] ?? -1,
      rgbaBeforeLoss[barIdx + 3] ?? -1,
    ];

    // Resize viewport + switch CRT filter + simulate WebGPU device loss (`device.destroy()`)
    mount.style.width = '960px';
    mount.style.height = '600px';
    display.setFilter('crt');
    await display.simulateDeviceLoss();

    const metricsAfterRecovery = display.getMetrics();
    const rgbaAfterLoss = await display.readRgbaPixels();
    const topPixelAfter = [
      rgbaAfterLoss[topIdx] ?? -1,
      rgbaAfterLoss[topIdx + 1] ?? -1,
      rgbaAfterLoss[topIdx + 2] ?? -1,
      rgbaAfterLoss[topIdx + 3] ?? -1,
    ];

    // Verify guest Linux is STILL alive and responsive after GPU device loss recovery!
    await orchestrator.sendSerialInput('printf "LINUX_STILL_%s\\n" "ALIVE"\n');
    await waitForText('LINUX_STILL_ALIVE');

    await orchestrator.shutdown();
    display.destroy();
    document.body.removeChild(mount);

    return {
      initialBackend,
      topPixelBefore,
      barPixelBefore,
      topPixelAfter,
      metricsAfterRecovery,
    };
  });

  expect(result.initialBackend).toBe('webgpu');
  expect(result.topPixelBefore).toEqual([0x38, 0xbd, 0xf8, 255]);
  expect(result.barPixelBefore).toEqual([0xef, 0x44, 0x44, 255]);
  expect(result.topPixelAfter).toEqual([0x38, 0xbd, 0xf8, 255]);
  expect(result.metricsAfterRecovery?.backend).toBe('webgpu');
  expect(result.metricsAfterRecovery?.deviceLostRecoveries).toBeGreaterThanOrEqual(1);
  expect(result.metricsAfterRecovery?.dirtyUploads).toBeGreaterThanOrEqual(1);
});

test('automatically falls back to Canvas 2D renderer with identical pixel accuracy when WebGPU is unavailable', async ({
  page,
}) => {
  test.setTimeout(90_000);
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'gpu', {
      value: undefined,
      configurable: true,
    });
  });

  await page.goto('/');
  await expect(page.locator('main[data-ready="true"]')).toBeVisible();

  const result = await page.evaluate(async () => {
    const StorageApi = window.__WASMNUX_STORAGE_API__;
    const OrchestratorCtor = window.__WASMNUX_BOOT_ORCHESTRATOR_CTOR__;
    const DisplayCtor = window.__WASMNUX_DISPLAY_CONTROLLER_CTOR__;
    if (!StorageApi || !OrchestratorCtor || !DisplayCtor) {
      throw new Error('Missing browser runtime constructors');
    }

    const mount = document.createElement('div');
    mount.style.width = '640px';
    mount.style.height = '400px';
    document.body.appendChild(mount);

    let fallbackReasonObserved = '';
    const display = new DisplayCtor({
      container: mount,
      preferredMode: 'auto',
      onBackendChanged: (_backend, _info, reason) => {
        if (reason) {
          fallbackReasonObserved = reason;
        }
      },
    });
    const activeBackend = await display.init();

    const storage = await StorageApi.createAuto();
    let serialLog = '';
    const dec = new TextDecoder();

    const orchestrator = new OrchestratorCtor({
      storage,
      events: {
        onSerialOutput: (bytes) => {
          serialLog += dec.decode(bytes, { stream: true });
        },
        onFramebufferDirty: (upd) => {
          display.applyDirtyUpdate(upd);
        },
      },
    });

    const waitForText = async (needle: string, timeoutMs = 30_000): Promise<void> => {
      const deadline = performance.now() + timeoutMs;
      while (!serialLog.includes(needle)) {
        if (performance.now() > deadline) {
          throw new Error(`Timed out waiting for "${needle}"`);
        }
        await new Promise((r) => setTimeout(r, 25));
      }
    };

    await orchestrator.bootProfile('vanilla');
    await waitForText('wasmnux-vanilla:~#');
    await orchestrator.sendSerialInput('/usr/bin/wasmnux-fbtest && printf "C2D_%s\\n" "OK"\n');
    await waitForText('C2D_OK');
    await new Promise((r) => setTimeout(r, 50));

    const rgba = await display.readRgbaPixels();
    const topIdx = (0 * 640 + 100) * 4;
    const topPixel = [
      rgba[topIdx] ?? -1,
      rgba[topIdx + 1] ?? -1,
      rgba[topIdx + 2] ?? -1,
      rgba[topIdx + 3] ?? -1,
    ];
    const barIdx = (380 * 640 + 10) * 4;
    const barPixel = [
      rgba[barIdx] ?? -1,
      rgba[barIdx + 1] ?? -1,
      rgba[barIdx + 2] ?? -1,
      rgba[barIdx + 3] ?? -1,
    ];
    const metrics = display.getMetrics();

    await orchestrator.shutdown();
    display.destroy();
    document.body.removeChild(mount);

    return { activeBackend, fallbackReasonObserved, topPixel, barPixel, metrics };
  });

  expect(result.activeBackend).toBe('canvas2d');
  expect(result.fallbackReasonObserved).toContain('navigator.gpu');
  expect(result.topPixel).toEqual([0x38, 0xbd, 0xf8, 255]);
  expect(result.barPixel).toEqual([0xef, 0x44, 0x44, 255]);
  expect(result.metrics?.framesPresented).toBeGreaterThanOrEqual(2);
});
