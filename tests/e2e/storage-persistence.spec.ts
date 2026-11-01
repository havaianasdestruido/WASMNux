import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium, expect, test } from '@playwright/test';
import config from '../../playwright.config';

test('persists isolated profile chunks, backups, and preferences across reload AND full browser restart (OPFS + IDB)', async ({
  baseURL,
}) => {
  const userDataDir = mkdtempSync(join(tmpdir(), 'wasmnux-persist-e2e-'));
  const launchOpts = config.use?.launchOptions ?? {};
  const targetUrl = baseURL ?? 'http://127.0.0.1:4173';

  try {
    // Session 1: Launch real Chromium context on fresh userDataDir, write OPFS+IDB chunks + IDB fallback + prefs
    const ctx1 = await chromium.launchPersistentContext(userDataDir, {
      headless: true,
      executablePath: launchOpts.executablePath,
      args: launchOpts.args,
      env: launchOpts.env,
    });
    const page1 = await ctx1.newPage();
    await page1.goto(targetUrl);
    await expect(page1.locator('main[data-ready="true"]')).toBeVisible();

    const session1Result = await page1.evaluate(async () => {
      const api = window.__WASMNUX_STORAGE_API__;
      if (!api) {
        throw new Error('Missing storage API');
      }
      const engine = await api.createAuto();
      const cap = 8 * 1024 * 1024;

      const vData = new Uint8Array(65536);
      vData.fill(0x41);
      vData[0] = 0x99;
      const vCrc = api.crc32(vData);

      const dData = new Uint8Array(65536);
      dData.fill(0x62);
      const dCrc = api.crc32(dData);

      await engine.commitDirtyChunks('vanilla', cap, [{ chunkIndex: 2, crc32: vCrc, data: vData }]);
      await engine.commitDirtyChunks('debian', cap, [{ chunkIndex: 0, crc32: dCrc, data: dData }]);
      await engine.createNamedBackup('vanilla', 'snap-v1', 'Initial Vanilla Snapshot', cap);
      await engine.setPreference('terminal.fontSize', 15);

      // Also verify IDB-only fallback mode works cleanly
      const idbOnly = await api.createAuto({ forceIdbOnly: true });
      await idbOnly.commitDirtyChunks('idb-fallback-test', cap, [
        { chunkIndex: 1, crc32: vCrc, data: vData },
      ]);

      return {
        mode: engine.mode,
        idbOnlyMode: idbOnly.mode,
        vCrc,
        dCrc,
      };
    });

    expect(session1Result.mode).toBe('opfs+idb');
    expect(session1Result.idbOnlyMode).toBe('idb-only');

    // Verify survival across full page reload first
    await page1.reload();
    await expect(page1.locator('main[data-ready="true"]')).toBeVisible();

    const afterReload = await page1.evaluate(async () => {
      const api = window.__WASMNUX_STORAGE_API__;
      if (!api) {
        throw new Error('Missing storage API');
      }
      const engine = await api.createAuto();
      const v = await engine.openAndRecoverProfile('vanilla', 8 * 1024 * 1024);
      return {
        gen: v.manifest.committedGeneration,
        count: v.overlayChunks.length,
        firstByte: v.overlayChunks[0]?.data[0] ?? -1,
      };
    });
    expect(afterReload).toEqual({ gen: 1, count: 1, firstByte: 0x99 });

    // Close the ENTIRE Chromium browser process!
    await ctx1.close();

    // Session 2: Re-launch a brand-new Chromium browser process pointing to the same userDataDir
    const ctx2 = await chromium.launchPersistentContext(userDataDir, {
      headless: true,
      executablePath: launchOpts.executablePath,
      args: launchOpts.args,
      env: launchOpts.env,
    });
    const page2 = await ctx2.newPage();
    await page2.goto(targetUrl);
    await expect(page2.locator('main[data-ready="true"]')).toBeVisible();

    const afterBrowserRestart = await page2.evaluate(async () => {
      const api = window.__WASMNUX_STORAGE_API__;
      if (!api) {
        throw new Error('Missing storage API');
      }
      const engine = await api.createAuto();
      const cap = 8 * 1024 * 1024;
      const vanilla = await engine.openAndRecoverProfile('vanilla', cap);
      const debian = await engine.openAndRecoverProfile('debian', cap);
      const backups = await engine.listNamedBackups('vanilla');
      const fontSize = await engine.getPreference(
        'terminal.fontSize',
        (v): v is number => typeof v === 'number',
      );

      const idbOnly = await api.createAuto({ forceIdbOnly: true });
      const idbRecovered = await idbOnly.openAndRecoverProfile('idb-fallback-test', cap);

      return {
        mode: engine.mode,
        vanillaCrc: vanilla.overlayChunks[0]?.crc32 ?? 0,
        debianCrc: debian.overlayChunks[0]?.crc32 ?? 0,
        backupCount: backups.length,
        backupId: backups[0]?.snapshotId ?? '',
        fontSize,
        idbRecoveredCrc: idbRecovered.overlayChunks[0]?.crc32 ?? 0,
      };
    });

    expect(afterBrowserRestart.mode).toBe('opfs+idb');
    expect(afterBrowserRestart.vanillaCrc).toBe(session1Result.vCrc);
    expect(afterBrowserRestart.debianCrc).toBe(session1Result.dCrc);
    expect(afterBrowserRestart.backupCount).toBe(1);
    expect(afterBrowserRestart.backupId).toBe('snap-v1');
    expect(afterBrowserRestart.fontSize).toBe(15);
    expect(afterBrowserRestart.idbRecoveredCrc).toBe(session1Result.vCrc);

    await ctx2.close();
  } finally {
    rmSync(userDataDir, { recursive: true, force: true });
  }
});
