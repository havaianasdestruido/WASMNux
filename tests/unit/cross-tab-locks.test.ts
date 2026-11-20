import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { crc32, StorageValidationError } from '../../src/storage/crc32';
import { isOpfsTransientError } from '../../src/storage/opfs-backend';
import { isQuotaExceededError, normalizeStorageError } from '../../src/storage/idb-backend';
import { MemoryStorageAdapter, PersistentStorageEngine } from '../../src/storage/storage-engine';
import { DISK_CHUNK_BYTES } from '../../src/wasm/bindings';

function makeChunk(
  index: number,
  fillByte: number,
): {
  chunkIndex: number;
  crc32: number;
  data: Uint8Array;
} {
  const data = new Uint8Array(DISK_CHUNK_BYTES);
  data.fill(fillByte);
  data[0] = index & 0xff;
  return { chunkIndex: index, crc32: crc32(data), data };
}

/**
 * Minimal but faithful in-memory implementation of the Web Locks API: requests are queued per lock
 * name and granted one at a time while the callback is running, exactly like the browser.
 */
class FakeWebLocks {
  private readonly chains = new Map<string, Promise<unknown>>();
  public readonly requestLog: Array<{ name: string; mode: string }> = [];
  private active = 0;
  public maxConcurrent = 0;

  public request<T>(
    name: string,
    options: { readonly mode?: 'exclusive' | 'shared' },
    callback: (lock: unknown) => Promise<T>,
  ): Promise<T> {
    this.requestLog.push({ name, mode: options.mode ?? 'exclusive' });
    const prev = this.chains.get(name) ?? Promise.resolve();
    const run = prev.then(async () => {
      this.active += 1;
      this.maxConcurrent = Math.max(this.maxConcurrent, this.active);
      try {
        return await callback({ name, mode: options.mode ?? 'exclusive' });
      } finally {
        this.active -= 1;
      }
    });
    this.chains.set(
      name,
      run.then(
        () => undefined,
        () => undefined,
      ),
    );
    return run;
  }
}

/** Adapter whose writes fail with a genuine browser `QuotaExceededError` DOMException. */
class QuotaFillingAdapter extends MemoryStorageAdapter {
  public budgetChunks = 2;
  public writes = 0;

  public override async writeChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    data: Uint8Array,
    crc?: number,
  ): Promise<number> {
    if (this.writes >= this.budgetChunks) {
      const err = new Error('The quota has been exceeded.');
      err.name = 'QuotaExceededError';
      throw err;
    }
    this.writes += 1;
    return super.writeChunk(profileId, chunkIndex, generation, data, crc);
  }
}

describe('Web Locks cross-tab locking & real browser quota exhaustion (Phase 4)', () => {
  it('detects the browser QuotaExceededError DOMException shapes and normalizes them', () => {
    const domErr = new Error('quota');
    domErr.name = 'QuotaExceededError';
    expect(isQuotaExceededError(domErr)).toBe(true);

    const legacyCode = Object.assign(new Error('code 22'), { code: 22 });
    expect(isQuotaExceededError(legacyCode)).toBe(true);

    const firefoxLegacy = Object.assign(new Error('ns'), { name: 'NS_ERROR_DOM_QUOTA_REACHED' });
    expect(isQuotaExceededError(firefoxLegacy)).toBe(true);

    expect(isQuotaExceededError(new Error('disk on fire'))).toBe(false);
    expect(isQuotaExceededError('quota')).toBe(false);

    // Real Chromium OPFS refusals surface as `DOMException`, which is NOT `instanceof Error`.
    class FakeDomException {
      public readonly name = 'QuotaExceededError';
      public readonly message = '';
      public readonly code = 22;
    }
    expect(isQuotaExceededError(new FakeDomException())).toBe(true);
    expect(
      (normalizeStorageError(new FakeDomException(), 'commit') as StorageValidationError).code,
    ).toBe('quota_exceeded');

    const normalized = normalizeStorageError(domErr, 'commit of profile vanilla');
    expect(normalized).toBeInstanceOf(StorageValidationError);
    expect((normalized as StorageValidationError).code).toBe('quota_exceeded');
    expect((normalized as Error).message).toContain('browser storage quota exhausted');

    // Non-quota errors are passed through untouched.
    const other = new Error('io');
    expect(normalizeStorageError(other, 'ctx')).toBe(other);
  });

  it('serializes concurrent commits of one profile under an exclusive cross-tab lock', async () => {
    const fake = new FakeWebLocks();
    const previous = (globalThis as Record<string, unknown>)['navigator'];
    // Install a `navigator.locks` shim for the duration of this test.
    Object.defineProperty(globalThis, 'navigator', {
      value: { locks: fake },
      configurable: true,
      writable: true,
    });
    try {
      const engine = new PersistentStorageEngine(new MemoryStorageAdapter());
      expect(engine.crossTabLockMode).toBe('web-locks');

      const lease = await engine.acquireProfileLock('vanilla');
      expect(lease.mode).toBe('web-locks');
      expect(fake.requestLog).toContainEqual({
        name: 'wasmnux-profile-vanilla',
        mode: 'exclusive',
      });
      // Acquiring twice in the same realm is idempotent and does not deadlock.
      const again = await engine.acquireProfileLock('vanilla');
      expect(again).toBe(lease);

      // Eight concurrent commits must never overlap inside the lock (maxConcurrent === 1).
      const cap = 64 * 1024 * 1024;
      await Promise.all(
        Array.from({ length: 8 }, (_, i) =>
          engine.commitDirtyChunks('vanilla', cap, [makeChunk(i, 0x10 + i)]),
        ),
      );
      expect(fake.maxConcurrent).toBe(1);
      const recovered = await engine.openAndRecoverProfile('vanilla', cap);
      expect(recovered.overlayChunks).toHaveLength(8);

      engine.releaseProfileLock('vanilla');
      const report = await engine.inspectQuotaHealth();
      expect(report.crossTabLockMode).toBe('web-locks');
      expect(report.lastQuotaExhaustion).toBeNull();
    } finally {
      Object.defineProperty(globalThis, 'navigator', {
        value: previous,
        configurable: true,
        writable: true,
      });
    }
  });

  it('degrades to an in-process lock when the Web Locks API is unavailable, and still serializes', async () => {
    const previous = (globalThis as Record<string, unknown>)['navigator'];
    Object.defineProperty(globalThis, 'navigator', {
      value: {},
      configurable: true,
      writable: true,
    });
    try {
      const engine = new PersistentStorageEngine(new MemoryStorageAdapter());
      expect(engine.crossTabLockMode).toBe('in-process');
      const lease = await engine.acquireProfileLock('vanilla');
      expect(lease.mode).toBe('in-process');

      const cap = 64 * 1024 * 1024;
      await Promise.all(
        Array.from({ length: 4 }, (_, i) =>
          engine.commitDirtyChunks('vanilla', cap, [makeChunk(i, 0x20 + i)]),
        ),
      );
      const recovered = await engine.openAndRecoverProfile('vanilla', cap);
      expect(recovered.overlayChunks).toHaveLength(4);
      lease.release();
      const report = await engine.inspectQuotaHealth();
      expect(report.crossTabLockMode).toBe('in-process');
    } finally {
      Object.defineProperty(globalThis, 'navigator', {
        value: previous,
        configurable: true,
        writable: true,
      });
    }
  });

  it('surfaces genuine browser quota exhaustion as a typed error and keeps committed state intact', async () => {
    const adapter = new QuotaFillingAdapter();
    const engine = new PersistentStorageEngine(adapter);
    const cap = 64 * 1024 * 1024;

    // First commit writes exactly one chunk inside the 2-chunk budget.
    await engine.commitDirtyChunks('vanilla', cap, [makeChunk(0, 0x11)]);
    const before = await engine.openAndRecoverProfile('vanilla', cap);
    expect(before.overlayChunks).toHaveLength(1);
    const beforeGen = before.manifest.committedGeneration;

    // Second commit writes chunk 1 (budget exhausted afterwards).
    await engine.commitDirtyChunks('vanilla', cap, [makeChunk(1, 0x22)]);

    // Third commit is refused by the browser itself -> typed quota_exceeded, no partial writes.
    const err = await engine
      .commitDirtyChunks('vanilla', cap, [makeChunk(2, 0x33), makeChunk(3, 0x44)])
      .then(
        () => null,
        (e: unknown) => e,
      );
    expect(err).toBeInstanceOf(StorageValidationError);
    expect((err as StorageValidationError).code).toBe('quota_exceeded');

    const after = await engine.openAndRecoverProfile('vanilla', cap);
    expect(after.manifest.committedGeneration).toBeGreaterThanOrEqual(beforeGen);
    expect(after.overlayChunks.map((c) => c.chunkIndex).sort()).toEqual([0, 1]);
    expect(after.recoveredOrphanChunks).toBe(0);

    const report = await engine.inspectQuotaHealth();
    expect(report.lastQuotaExhaustion).not.toBeNull();
    expect(report.lastQuotaExhaustion?.profileId).toBe('vanilla');
  });

  it('classifies transient OPFS errors and uses per-probe unique filenames', () => {
    // Chromium raises these transiently while another tab mutates the same OPFS tree.
    const notFound = Object.assign(new Error('nf'), { name: 'NotFoundError' });
    const typeMismatch = Object.assign(new Error('tm'), { name: 'TypeMismatchError' });
    expect(isOpfsTransientError(notFound)).toBe(true);
    expect(isOpfsTransientError(typeMismatch)).toBe(true);
    expect(isOpfsTransientError(new Error('boom'))).toBe(false);
    expect(isOpfsTransientError(null)).toBe(false);

    // The support probe must not share one filename across tabs (that race silently downgraded a
    // second tab to `idb-only`); assert the implementation derives a unique name per probe.
    const source = readFileSync(resolve(process.cwd(), 'src/storage/opfs-backend.ts'), 'utf8');
    expect(source).not.toContain("'.opfs-probe'");
    expect(source).toContain('.opfs-probe-');
    expect(source).toContain('Math.random()');
  });

  it('probes real origin quota with scratch chunks and always cleans up the probe profile', async () => {
    const engine = new PersistentStorageEngine(new MemoryStorageAdapter());
    const result = await engine.probeQuotaExhaustion(3);
    expect(result.attempted).toBe(3);
    expect(result.written).toBe(3);
    expect(result.exhausted).toBe(false);
    expect(result.error).toBeNull();

    // The probe profile is removed, so no guest-visible profile gains chunks.
    const vanilla = await engine.openAndRecoverProfile('vanilla', 64 * 1024 * 1024);
    expect(vanilla.overlayChunks).toHaveLength(0);

    // A budget-limited adapter reports genuine exhaustion mid-probe.
    const tight = new QuotaFillingAdapter();
    tight.budgetChunks = 1;
    const tightEngine = new PersistentStorageEngine(tight);
    const tightResult = await tightEngine.probeQuotaExhaustion(4);
    expect(tightResult.written).toBe(1);
    expect(tightResult.exhausted).toBe(true);
    expect(tightResult.error?.message).toContain('quota');
  });
});
