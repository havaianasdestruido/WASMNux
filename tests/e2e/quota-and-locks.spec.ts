import { expect, test } from '@playwright/test';

/**
 * Phase 4 browser-only evidence that cannot be produced in Node/Vitest:
 *
 *  1. Real origin quota exhaustion — keep writing 64 KiB chunks (through the production OPFS/IDB
 *     backend) until the browser itself refuses a write. The engine must surface that as a typed
 *     `quota_exceeded` `StorageValidationError`, keep previously committed data readable, and
 *     report it through `inspectQuotaHealth().lastQuotaExhaustion`.
 *  2. Real Web Locks — `navigator.locks` must be present, `crossTabLockMode === 'web-locks'`, and
 *     two *separate browser tabs* holding an exclusive lock on the same profile must serialize.
 */
test('exhausts the real browser origin quota and detects typed quota_exceeded without data loss', async ({
  page,
}) => {
  test.setTimeout(300_000);
  await page.goto('/');
  await expect(page.locator('main[data-ready="true"]')).toBeVisible();

  const result = await page.evaluate(async () => {
    const api = window.__WASMNUX_STORAGE_API__;
    if (!api) {
      throw new Error('Missing storage API');
    }
    const engine = await api.createAuto();
    const cap = 1024 * 1024 * 1024; // 1 GiB logical profile; quota is the binding constraint
    const chunk = new Uint8Array(65536);
    chunk.fill(0xa5);
    const checksum = api.crc32(chunk);

    // Seed a chunk that must survive the exhaustion.
    await engine.commitDirtyChunks('quota-e2e', cap, [
      { chunkIndex: 0, crc32: checksum, data: chunk },
    ]);

    const estimateBefore = await navigator.storage.estimate();
    let written = 0;
    let typedError: { name: string; code: string; message: string } | null = null;
    let otherError: string | null = null;

    // Fill until the browser refuses. Bounded so a very large quota cannot hang the test.
    //
    // The bound is deliberately far above the observed refusal point: Chromium's per-origin
    // bucket limit on this host is fluid (measured refusals at ~219 MiB and ~251 MiB of 64 KiB
    // chunks on the same machine, well below the ~1.07 GiB `estimate().quota`). A 256 MiB bound
    // sat inside that band and made the spec flake when the limit drifted up. 512 MiB leaves
    // ~2x headroom over the highest observed refusal while still terminating in well under the
    // test timeout if the browser never refuses.
    for (let i = 1; i <= 8192; i += 1) {
      try {
        await engine.commitDirtyChunks('quota-e2e', cap, [
          { chunkIndex: i, crc32: checksum, data: chunk },
        ]);
        written += 1;
      } catch (err: unknown) {
        const e = err as { name?: string; code?: string; message?: string };
        if (e.code === 'quota_exceeded') {
          typedError = {
            name: e.name ?? 'Error',
            code: e.code,
            message: e.message ?? '',
          };
        } else {
          otherError = `${e.name ?? 'Error'}: ${e.message ?? String(err)}`;
        }
        break;
      }
    }

    const report = await engine.inspectQuotaHealth();
    const recovered = await engine.openAndRecoverProfile('quota-e2e', cap);
    // Free the filled space again so the test leaves the origin usable.
    await engine.resetProfileDisk('quota-e2e', cap);
    const estimateAfter = await navigator.storage.estimate();

    return {
      written,
      typedError,
      otherError,
      crossTabLockMode: report.crossTabLockMode,
      lastQuotaExhaustion: report.lastQuotaExhaustion,
      committedChunkIndexes: recovered.overlayChunks.map((c) => c.chunkIndex).sort((a, b) => a - b),
      recoveredOrphanChunks: recovered.recoveredOrphanChunks,
      mode: engine.mode,
      quotaBefore: estimateBefore.quota ?? 0,
      quotaAfter: estimateAfter.quota ?? 0,
      hasWebLocks: typeof navigator.locks.request === 'function',
    };
  });

  // Real Web Locks support is present in Chromium, so the engine must use it.
  expect(result.hasWebLocks).toBe(true);
  expect(result.crossTabLockMode).toBe('web-locks');
  expect(result.mode).toBe('opfs+idb');

  // The browser genuinely refused a write and the engine typed it as quota_exceeded.
  expect(result.otherError).toBeNull();
  expect(result.typedError).not.toBeNull();
  expect(result.typedError?.code).toBe('quota_exceeded');
  expect(result.typedError?.message.length).toBeGreaterThan(0);
  expect(result.written).toBeGreaterThan(0);

  // Health reporting surfaces the exhaustion.
  expect(result.lastQuotaExhaustion).not.toBeNull();
  expect(result.lastQuotaExhaustion?.profileId).toBe('quota-e2e');

  // Previously committed data is intact. Recovery sweeps the uncommitted partial generation from
  // the refused write (that is the crash-consistency contract) and leaves no trace beyond it.
  expect(result.committedChunkIndexes).toContain(0);
  expect(result.recoveredOrphanChunks).toBeLessThanOrEqual(1);
});

test('serializes cross-tab access to one profile with real navigator.locks across two tabs', async ({
  context,
  page,
}) => {
  test.setTimeout(120_000);
  await page.goto('/');
  await expect(page.locator('main[data-ready="true"]')).toBeVisible();

  const secondPage = await context.newPage();
  await secondPage.goto('/');
  await expect(secondPage.locator('main[data-ready="true"]')).toBeVisible();

  // Both tabs are armed to request the same named lock; tab A acquires and holds it for 400 ms
  // while tab B is already waiting. Both requests are started *concurrently* (no awaiting tab A
  // before starting tab B) so the race is real and the winner is not predetermined.
  // Absolute wall-clock timestamps are returned (not per-tab relative ones) so the two tabs can be
  // compared directly: both run in the same browser process and share the clock.
  const probeLock = async (): Promise<{
    events: string[];
    acquiredAtEpochMs: number;
    releasedAtEpochMs: number;
  }> => {
    const events: string[] = [];
    let acquiredAtEpochMs = 0;
    let releasedAtEpochMs = 0;
    await navigator.locks.request('wasmnux-e2e-probe', { mode: 'exclusive' }, async () => {
      acquiredAtEpochMs = Date.now();
      events.push(`acquired:${String(acquiredAtEpochMs)}`);
      await new Promise((resolve) => setTimeout(resolve, 400));
      events.push('releasing');
      releasedAtEpochMs = Date.now();
    });
    events.push('done');
    return { events, acquiredAtEpochMs, releasedAtEpochMs };
  };

  const tabAPromise = page.evaluate(probeLock);
  await page.waitForTimeout(50); // deterministic head start for tab A
  const tabBPromise = secondPage.evaluate(probeLock);
  const [tabA, tabB] = await Promise.all([tabAPromise, tabBPromise]);

  expect(tabA.events).toContain('done');
  expect(tabB.events).toContain('done');
  // Tab A owns the lock first (it had a 50 ms head start) and holds it for 400 ms; tab B may only
  // enter after A releases. The two hold windows therefore cannot overlap at all.
  expect(tabA.acquiredAtEpochMs).toBeLessThan(tabB.acquiredAtEpochMs);
  expect(tabB.acquiredAtEpochMs).toBeGreaterThanOrEqual(tabA.releasedAtEpochMs);
  expect(tabA.releasedAtEpochMs - tabA.acquiredAtEpochMs).toBeGreaterThanOrEqual(380);

  // A second tab cannot mutate a profile that the first tab is actively driving: the engine's
  // bounded lock acquisition reports a typed `profile_locked` instead of silently racing. Tab B
  // holds the profile for ~1.5 s while tab A tries (and must fail) to acquire it.
  const tabBHoldsProfile = secondPage.evaluate(async () => {
    const api = window.__WASMNUX_STORAGE_API__;
    if (!api) {
      throw new Error('Missing storage API');
    }
    const engine = await api.createAuto();
    const lease = await engine.acquireProfileLock('contended-profile');
    await new Promise((resolve) => setTimeout(resolve, 1500));
    lease.release();
    return engine.crossTabLockMode;
  });
  await secondPage.waitForTimeout(250); // tab B owns the lock before tab A asks for it

  const lockBlocked = await page.evaluate(async () => {
    const api = window.__WASMNUX_STORAGE_API__;
    if (!api) {
      throw new Error('Missing storage API');
    }
    const engine = await api.createAuto();
    const start = Date.now();
    try {
      await engine.acquireProfileLock('contended-profile', 200);
      return { blocked: false, code: 'none', elapsed: Date.now() - start };
    } catch (err: unknown) {
      const e = err as { code?: string };
      return { blocked: true, code: e.code ?? 'unknown', elapsed: Date.now() - start };
    }
  });
  expect(await tabBHoldsProfile).toBe('web-locks');
  expect(lockBlocked.blocked).toBe(true);
  expect(lockBlocked.code).toBe('profile_locked');

  // Concurrent commits from two tabs land in one consistent committed generation set.
  const cap = 64 * 1024 * 1024;
  const commitFromTab = async (p: typeof page, indexes: readonly number[]): Promise<void> => {
    await p.evaluate(
      async ({ indexes, cap }: { indexes: readonly number[]; cap: number }) => {
        const api = window.__WASMNUX_STORAGE_API__;
        if (!api) {
          throw new Error('Missing storage API');
        }
        const engine = await api.createAuto();
        const data = new Uint8Array(65536);
        for (const idx of indexes) {
          data.fill((idx & 0xff) + 1);
          data[0] = idx & 0xff;
          await engine.commitDirtyChunks('cross-tab-e2e', cap, [
            { chunkIndex: idx, crc32: api.crc32(data), data: data.slice() },
          ]);
        }
      },
      { indexes, cap },
    );
  };
  await Promise.all([commitFromTab(page, [0, 1, 2, 3]), commitFromTab(secondPage, [4, 5, 6, 7])]);

  const finalChunks = await page.evaluate(async () => {
    const api = window.__WASMNUX_STORAGE_API__;
    if (!api) {
      throw new Error('Missing storage API');
    }
    const engine = await api.createAuto();
    const rec = await engine.openAndRecoverProfile('cross-tab-e2e', 64 * 1024 * 1024);
    const out = {
      indexes: rec.overlayChunks.map((c) => c.chunkIndex).sort((a, b) => a - b),
      orphans: rec.recoveredOrphanChunks,
    };
    await engine.resetProfileDisk('cross-tab-e2e', 64 * 1024 * 1024);
    return out;
  });
  expect(finalChunks.indexes).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
  expect(finalChunks.orphans).toBe(0);

  await secondPage.close();
});
