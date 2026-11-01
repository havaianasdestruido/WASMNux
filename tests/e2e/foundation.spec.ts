import { expect, test } from '@playwright/test';

test('loads compiled Rust WASM runtime, executes RV64 guest inside Web Worker, and reports capabilities', async ({
  page,
}) => {
  const pageErrors: Error[] = [];
  page.on('pageerror', (err) => pageErrors.push(err));

  await page.goto('/');
  await expect(page.locator('main[data-ready="true"]')).toBeVisible();

  const reportText = await page.locator('#capability-report').textContent();
  expect(reportText).toBeTruthy();

  const parsed = JSON.parse(reportText ?? '{}') as {
    abiVersion: number;
    workerSmoke: {
      transportMode: string;
      serialBanner: string;
      dirtyScanlineObserved: boolean;
    };
    capabilities: {
      webAssembly: boolean;
      webWorker: boolean;
      canvas2dAvailable: boolean;
      webGpuAvailable: boolean;
      opfsAvailable: boolean;
      indexedDbAvailable: boolean;
      crossOriginIsolated: boolean;
    };
  };

  expect(parsed.abiVersion).toBe(1);
  expect(parsed.workerSmoke.transportMode).toBe('worker');
  expect(parsed.workerSmoke.serialBanner).toBe('WORKER-RV64-OK');
  expect(parsed.workerSmoke.dirtyScanlineObserved).toBe(true);
  expect(parsed.capabilities.webAssembly).toBe(true);
  expect(parsed.capabilities.webWorker).toBe(true);
  expect(parsed.capabilities.canvas2dAvailable).toBe(true);
  expect(parsed.capabilities.indexedDbAvailable).toBe(true);
  expect(parsed.capabilities.opfsAvailable).toBe(true);
  expect(parsed.capabilities.crossOriginIsolated).toBe(true);
  expect(parsed.capabilities.webGpuAvailable).toBe(true);
  expect(pageErrors).toHaveLength(0);
});
