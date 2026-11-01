import { expect, test } from '@playwright/test';

test('boots genuine Vanilla Linux 7.3.1 in Chromium Web Worker and persists ext4 files across page reload', async ({
  page,
}) => {
  test.setTimeout(90_000);
  await page.goto('/');
  await expect(page.locator('main[data-ready="true"]')).toBeVisible();

  // Boot 1: Boot real Vanilla Linux 7.3.1 in Web Worker, execute BusyBox commands, write /root/e2e.txt, poweroff
  const boot1 = await page.evaluate(async () => {
    const StorageApi = window.__WASMNUX_STORAGE_API__;
    const OrchestratorCtor = window.__WASMNUX_BOOT_ORCHESTRATOR_CTOR__;
    if (!StorageApi || !OrchestratorCtor) {
      throw new Error('Missing browser boot APIs');
    }

    const storage = await StorageApi.createAuto();
    await storage.resetProfileDisk('vanilla', 64 * 1024 * 1024);

    let serialLog = '';
    const dec = new TextDecoder();
    let poweredOffResolve: (() => void) | null = null;
    const poweredOffPromise = new Promise<void>((resolve) => {
      poweredOffResolve = resolve;
    });

    const orchestrator = new OrchestratorCtor({
      storage,
      events: {
        onSerialOutput: (bytes) => {
          serialLog += dec.decode(bytes, { stream: true });
        },
        onLifecycleChange: (state) => {
          if (state === 'powered_off' && poweredOffResolve) {
            poweredOffResolve();
          }
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

    const bootInfo = await orchestrator.bootProfile('vanilla');
    await waitForText('wasmnux-vanilla:~#');

    await orchestrator.sendSerialInput(
      'uname -srm && echo "CHROMIUM_OPFS_EXT4_OK" > /root/e2e.txt && sync && printf "WROTE_%s\\n" "DONE"\n',
    );
    await waitForText('WROTE_DONE');

    await orchestrator.sendSerialInput('poweroff\n');
    await poweredOffPromise;
    await orchestrator.shutdown();

    return {
      storageMode: storage.mode,
      bootReadyMs: bootInfo.bootReadyMs,
      hasKernel731: serialLog.includes('Linux 7.3.1 riscv64'),
    };
  });

  expect(boot1.storageMode).toBe('opfs+idb');
  expect(boot1.hasKernel731).toBe(true);

  // Full page reload -> fresh Worker + OPFS/IndexedDB recovery!
  await page.reload();
  await expect(page.locator('main[data-ready="true"]')).toBeVisible();

  const boot2 = await page.evaluate(async () => {
    const StorageApi = window.__WASMNUX_STORAGE_API__;
    const OrchestratorCtor = window.__WASMNUX_BOOT_ORCHESTRATOR_CTOR__;
    if (!StorageApi || !OrchestratorCtor) {
      throw new Error('Missing browser boot APIs');
    }

    const storage = await StorageApi.createAuto();
    let serialLog = '';
    const dec = new TextDecoder();

    const orchestrator = new OrchestratorCtor({
      storage,
      events: {
        onSerialOutput: (bytes) => {
          serialLog += dec.decode(bytes, { stream: true });
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

    await orchestrator.sendSerialInput('cat /root/e2e.txt && printf "READ_%s\\n" "DONE"\n');
    await waitForText('READ_DONE');
    await orchestrator.shutdown();

    return {
      persistedContentFound: serialLog.includes('CHROMIUM_OPFS_EXT4_OK'),
    };
  });

  expect(boot2.persistedContentFound).toBe(true);
});
