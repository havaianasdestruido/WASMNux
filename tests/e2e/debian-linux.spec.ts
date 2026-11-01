import { expect, test } from '@playwright/test';

test('boots Official Debian 13.7 ("trixie") riscv64 in Chromium Web Worker, installs .deb via real apt-get over VirtIO-Net, and persists dpkg state across reload', async ({
  page,
}) => {
  test.setTimeout(180_000);
  await page.goto('/');
  await expect(page.locator('main[data-ready="true"]')).toBeVisible();

  const boot1 = await page.evaluate(async () => {
    const StorageApi = window.__WASMNUX_STORAGE_API__;
    const OrchestratorCtor = window.__WASMNUX_BOOT_ORCHESTRATOR_CTOR__;
    if (!StorageApi || !OrchestratorCtor) {
      throw new Error('Missing browser boot APIs');
    }

    const storage = await StorageApi.createAuto();
    await storage.resetProfileDisk('debian', 256 * 1024 * 1024);

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

    const waitForText = async (needle: string, timeoutMs = 90_000): Promise<void> => {
      const deadline = performance.now() + timeoutMs;
      while (!serialLog.includes(needle)) {
        if (performance.now() > deadline) {
          throw new Error(`Timed out waiting for "${needle}". Tail:\n${serialLog.slice(-1200)}`);
        }
        await new Promise((r) => setTimeout(r, 40));
      }
    };

    const bootInfo = await orchestrator.bootProfile('debian', { networkEnabled: true });
    await waitForText('WASMNUX_DEBIAN_BOOT_COMPLETE');
    await waitForText('# ');

    await orchestrator.sendSerialInput(
      [
        '(',
        'set -ex',
        'apt-get update',
        'DEBIAN_FRONTEND=noninteractive apt-get install -y wasmnux-hello',
        'wasmnux-hello chromium-e2e',
        'sync',
        'printf "E2E_APT_%s\\n" "INSTALLED"',
        ')',
      ].join('\n') + '\n',
    );
    await waitForText('E2E_APT_INSTALLED', 90_000);

    await orchestrator.sendSerialInput('poweroff\n');
    await poweredOffPromise;
    await orchestrator.shutdown();

    return {
      storageMode: storage.mode,
      bootReadyMs: bootInfo.bootReadyMs,
      ranHello: serialLog.includes(
        'WASMNUX_HELLO_OK kernel=Linux-7.3.1 arch=riscv64 arg[1]=chromium-e2e',
      ),
    };
  });

  expect(boot1.storageMode).toBe('opfs+idb');
  expect(boot1.ranHello).toBe(true);

  // Full page reload -> fresh Dedicated Worker + OPFS/IndexedDB recovery with network DISABLED (offline!)
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

    const waitForText = async (needle: string, timeoutMs = 60_000): Promise<void> => {
      const deadline = performance.now() + timeoutMs;
      while (!serialLog.includes(needle)) {
        if (performance.now() > deadline) {
          throw new Error(`Timed out waiting for "${needle}". Tail:\n${serialLog.slice(-1000)}`);
        }
        await new Promise((r) => setTimeout(r, 40));
      }
    };

    await orchestrator.bootProfile('debian', { networkEnabled: false });
    await waitForText('WASMNUX_DEBIAN_BOOT_COMPLETE');
    await waitForText('# ');

    await orchestrator.sendSerialInput(
      'dpkg -s wasmnux-hello && wasmnux-hello reloaded-offline && printf "RELOAD_DPKG_%s\\n" "OK"\n',
    );
    await waitForText('RELOAD_DPKG_OK');
    await orchestrator.shutdown();

    return {
      dpkgInstalled: serialLog.includes('Status: install ok installed'),
      helloPersisted: serialLog.includes(
        'WASMNUX_HELLO_OK kernel=Linux-7.3.1 arch=riscv64 arg[1]=reloaded-offline',
      ),
    };
  });

  expect(boot2.dpkgInstalled).toBe(true);
  expect(boot2.helloPersisted).toBe(true);
});
