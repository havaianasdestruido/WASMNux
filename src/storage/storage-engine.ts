/**
 * Unified crash-consistent Persistent Virtual Storage Engine (`OPFS` + `IndexedDB` + `Memory` fallback)
 * with two-phase generation journaling, strict profile isolation, quota monitoring, snapshots,
 * and `.wasmnux-disk` export/import.
 */

import { DISK_CHUNK_BYTES, type DirtyDiskChunk } from '../wasm/bindings';
import type { BaseChunkInput } from '../worker/protocol';
import {
  crc32,
  decodeAndValidateDiskOverlayBundle,
  encodeDiskOverlayBundle,
  StorageValidationError,
  validateStorageIdentifier,
} from './crc32';
import {
  IdbStorageBackend,
  type CommittedChunkSlot,
  type ProfileManifestRecord,
  type StoredSnapshotRecord,
} from './idb-backend';
import { OpfsChunkBackend } from './opfs-backend';

export type ActiveStorageMode = 'opfs+idb' | 'idb-only' | 'memory';

export interface StorageDriverAdapter {
  readonly mode: ActiveStorageMode;
  getManifest(profileId: string): Promise<ProfileManifestRecord | null>;
  putManifest(manifest: ProfileManifestRecord): Promise<void>;
  deleteManifest(profileId: string): Promise<void>;
  writeChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    data: Uint8Array,
    crc?: number,
  ): Promise<number>;
  readChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    expectedCrc?: number,
  ): Promise<{ chunkIndex: number; generation: number; crc32: number; data: Uint8Array }>;
  deleteChunk(profileId: string, chunkIndex: number, generation: number): Promise<void>;
  listChunks(profileId: string): Promise<ReadonlyArray<{ chunkIndex: number; generation: number }>>;
  clearProfileChunks(profileId: string): Promise<void>;
  saveSnapshot(
    profileId: string,
    snapshotId: string,
    label: string,
    payload: Uint8Array,
  ): Promise<StoredSnapshotRecord>;
  loadSnapshot(profileId: string, snapshotId: string): Promise<StoredSnapshotRecord>;
  listSnapshots(profileId: string): Promise<ReadonlyArray<Omit<StoredSnapshotRecord, 'payload'>>>;
  deleteSnapshot(profileId: string, snapshotId: string): Promise<void>;
  clearProfileSnapshots(profileId: string): Promise<void>;
  setPreference(key: string, value: unknown): Promise<void>;
  getPreference<T>(key: string, guard: (val: unknown) => val is T): Promise<T | null>;
}

/** Deterministic in-memory storage adapter for unit tests and restricted browser contexts. */
export class MemoryStorageAdapter implements StorageDriverAdapter {
  public readonly mode: ActiveStorageMode = 'memory';
  private readonly manifests = new Map<string, ProfileManifestRecord>();
  private readonly chunks = new Map<
    string,
    { profileId: string; chunkIndex: number; generation: number; crc32: number; data: Uint8Array }
  >();
  private readonly snapshots = new Map<string, StoredSnapshotRecord>();
  private readonly prefs = new Map<string, unknown>();

  /** Optional fault-injection hook invoked before each chunk write (for crash-recovery tests). */
  public onBeforeWriteChunk?: (profileId: string, chunkIndex: number, generation: number) => void;

  public getManifest(profileId: string): Promise<ProfileManifestRecord | null> {
    validateStorageIdentifier(profileId);
    const m = this.manifests.get(profileId);
    return Promise.resolve(m ? structuredClone(m) : null);
  }

  public putManifest(manifest: ProfileManifestRecord): Promise<void> {
    validateStorageIdentifier(manifest.profileId);
    this.manifests.set(manifest.profileId, structuredClone(manifest));
    return Promise.resolve();
  }

  public deleteManifest(profileId: string): Promise<void> {
    validateStorageIdentifier(profileId);
    this.manifests.delete(profileId);
    return Promise.resolve();
  }

  public writeChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    data: Uint8Array,
    precomputedCrc?: number,
  ): Promise<number> {
    validateStorageIdentifier(profileId);
    this.onBeforeWriteChunk?.(profileId, chunkIndex, generation);
    if (data.byteLength !== DISK_CHUNK_BYTES) {
      return Promise.reject(new StorageValidationError('corrupt_bundle', 'Invalid chunk length'));
    }
    const c = precomputedCrc ?? crc32(data);
    this.chunks.set(`${profileId}:${String(chunkIndex)}:${String(generation)}`, {
      profileId,
      chunkIndex,
      generation,
      crc32: c,
      data: data.slice(),
    });
    return Promise.resolve(c);
  }

  public readChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    expectedCrc?: number,
  ): Promise<{ chunkIndex: number; generation: number; crc32: number; data: Uint8Array }> {
    const row = this.chunks.get(`${profileId}:${String(chunkIndex)}:${String(generation)}`);
    if (!row) {
      return Promise.reject(
        new StorageValidationError('corrupt_bundle', 'Missing chunk in memory'),
      );
    }
    const actual = crc32(row.data);
    if (actual !== row.crc32 || (expectedCrc !== undefined && actual !== expectedCrc >>> 0)) {
      return Promise.reject(
        new StorageValidationError('checksum_mismatch', 'Memory chunk CRC mismatch'),
      );
    }
    return Promise.resolve({
      chunkIndex,
      generation,
      crc32: actual,
      data: row.data.slice(),
    });
  }

  public deleteChunk(profileId: string, chunkIndex: number, generation: number): Promise<void> {
    this.chunks.delete(`${profileId}:${String(chunkIndex)}:${String(generation)}`);
    return Promise.resolve();
  }

  public listChunks(
    profileId: string,
  ): Promise<ReadonlyArray<{ chunkIndex: number; generation: number }>> {
    const out: Array<{ chunkIndex: number; generation: number }> = [];
    for (const v of this.chunks.values()) {
      if (v.profileId === profileId) {
        out.push({ chunkIndex: v.chunkIndex, generation: v.generation });
      }
    }
    return Promise.resolve(out);
  }

  public clearProfileChunks(profileId: string): Promise<void> {
    for (const [k, v] of this.chunks.entries()) {
      if (v.profileId === profileId) {
        this.chunks.delete(k);
      }
    }
    return Promise.resolve();
  }

  public saveSnapshot(
    profileId: string,
    snapshotId: string,
    label: string,
    payload: Uint8Array,
  ): Promise<StoredSnapshotRecord> {
    validateStorageIdentifier(profileId);
    validateStorageIdentifier(snapshotId);
    const key = `${profileId}:${snapshotId}`;
    const rec: StoredSnapshotRecord = {
      key,
      profileId,
      snapshotId,
      label,
      createdAtIso: new Date().toISOString(),
      byteLength: payload.byteLength,
      crc32: crc32(payload),
      payload: payload.slice(),
    };
    this.snapshots.set(key, rec);
    return Promise.resolve(rec);
  }

  public loadSnapshot(profileId: string, snapshotId: string): Promise<StoredSnapshotRecord> {
    validateStorageIdentifier(profileId);
    validateStorageIdentifier(snapshotId);
    const rec = this.snapshots.get(`${profileId}:${snapshotId}`);
    if (!rec) {
      return Promise.reject(new StorageValidationError('corrupt_bundle', 'Snapshot not found'));
    }
    if (crc32(rec.payload) !== rec.crc32) {
      return Promise.reject(
        new StorageValidationError('checksum_mismatch', 'Snapshot CRC mismatch'),
      );
    }
    return Promise.resolve({ ...rec, payload: rec.payload.slice() });
  }

  public listSnapshots(
    profileId: string,
  ): Promise<ReadonlyArray<Omit<StoredSnapshotRecord, 'payload'>>> {
    validateStorageIdentifier(profileId);
    const out: Array<Omit<StoredSnapshotRecord, 'payload'>> = [];
    for (const r of this.snapshots.values()) {
      if (r.profileId === profileId) {
        out.push({
          key: r.key,
          profileId: r.profileId,
          snapshotId: r.snapshotId,
          label: r.label,
          createdAtIso: r.createdAtIso,
          byteLength: r.byteLength,
          crc32: r.crc32,
        });
      }
    }
    return Promise.resolve(out);
  }

  public deleteSnapshot(profileId: string, snapshotId: string): Promise<void> {
    validateStorageIdentifier(profileId);
    validateStorageIdentifier(snapshotId);
    this.snapshots.delete(`${profileId}:${snapshotId}`);
    return Promise.resolve();
  }

  public clearProfileSnapshots(profileId: string): Promise<void> {
    for (const [k, v] of this.snapshots.entries()) {
      if (v.profileId === profileId) {
        this.snapshots.delete(k);
      }
    }
    return Promise.resolve();
  }

  public setPreference(key: string, value: unknown): Promise<void> {
    this.prefs.set(key, structuredClone(value));
    return Promise.resolve();
  }

  public getPreference<T>(key: string, guard: (val: unknown) => val is T): Promise<T | null> {
    if (!this.prefs.has(key)) {
      return Promise.resolve(null);
    }
    const cloned: unknown = structuredClone(this.prefs.get(key));
    return Promise.resolve(guard(cloned) ? cloned : null);
  }
}

class BrowserStorageAdapter implements StorageDriverAdapter {
  public readonly mode: ActiveStorageMode;
  private readonly idb: IdbStorageBackend;
  private readonly opfs: OpfsChunkBackend | null;

  public constructor(idb: IdbStorageBackend, opfs: OpfsChunkBackend | null) {
    this.idb = idb;
    this.opfs = opfs;
    this.mode = opfs !== null ? 'opfs+idb' : 'idb-only';
  }

  public getManifest(profileId: string): Promise<ProfileManifestRecord | null> {
    return this.idb.getManifest(profileId);
  }

  public putManifest(manifest: ProfileManifestRecord): Promise<void> {
    return this.idb.putManifest(manifest);
  }

  public deleteManifest(profileId: string): Promise<void> {
    return this.idb.deleteManifest(profileId);
  }

  public writeChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    data: Uint8Array,
    crc?: number,
  ): Promise<number> {
    if (this.opfs !== null) {
      return this.opfs.writeChunk(profileId, chunkIndex, generation, data, crc);
    }
    return this.idb.writeChunk(profileId, chunkIndex, generation, data, crc);
  }

  public readChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    expectedCrc?: number,
  ): Promise<{ chunkIndex: number; generation: number; crc32: number; data: Uint8Array }> {
    if (this.opfs !== null) {
      return this.opfs.readChunk(profileId, chunkIndex, generation, expectedCrc);
    }
    return this.idb.readChunk(profileId, chunkIndex, generation, expectedCrc);
  }

  public deleteChunk(profileId: string, chunkIndex: number, generation: number): Promise<void> {
    if (this.opfs !== null) {
      return this.opfs.deleteChunk(profileId, chunkIndex, generation);
    }
    return this.idb.deleteChunk(profileId, chunkIndex, generation);
  }

  public listChunks(
    profileId: string,
  ): Promise<ReadonlyArray<{ chunkIndex: number; generation: number }>> {
    if (this.opfs !== null) {
      return this.opfs.listChunkFiles(profileId);
    }
    return this.idb.listChunks(profileId);
  }

  public async clearProfileChunks(profileId: string): Promise<void> {
    if (this.opfs !== null) {
      await this.opfs.clearProfile(profileId);
    }
    await this.idb.clearProfileChunks(profileId);
  }

  public saveSnapshot(
    profileId: string,
    snapshotId: string,
    label: string,
    payload: Uint8Array,
  ): Promise<StoredSnapshotRecord> {
    return this.idb.saveSnapshot(profileId, snapshotId, label, payload);
  }

  public loadSnapshot(profileId: string, snapshotId: string): Promise<StoredSnapshotRecord> {
    return this.idb.loadSnapshot(profileId, snapshotId);
  }

  public listSnapshots(
    profileId: string,
  ): Promise<ReadonlyArray<Omit<StoredSnapshotRecord, 'payload'>>> {
    return this.idb.listSnapshots(profileId);
  }

  public deleteSnapshot(profileId: string, snapshotId: string): Promise<void> {
    return this.idb.deleteSnapshot(profileId, snapshotId);
  }

  public clearProfileSnapshots(profileId: string): Promise<void> {
    return this.idb.clearProfileSnapshots(profileId);
  }

  public setPreference(key: string, value: unknown): Promise<void> {
    return this.idb.setPreference(key, value);
  }

  public getPreference<T>(key: string, guard: (val: unknown) => val is T): Promise<T | null> {
    return this.idb.getPreference(key, guard);
  }
}

export interface ProfileStorageStatus {
  readonly profileId: string;
  readonly capacityBytes: number;
  readonly committedGeneration: number;
  readonly overlayChunkCount: number;
  readonly overlayBytes: number;
  readonly snapshotCount: number;
  readonly updatedAtIso: string | null;
}

export interface QuotaHealthReport {
  readonly mode: ActiveStorageMode;
  readonly persisted: boolean;
  readonly browserUsageBytes: number;
  readonly browserQuotaBytes: number;
  readonly availableBytes: number;
  readonly lowSpaceWarning: boolean;
  readonly evictionRiskWarning: boolean;
}

export class PersistentStorageEngine {
  private readonly adapter: StorageDriverAdapter;
  private maxOverlayChunksPerProfile: number | null = null;
  /** Per-profile serialization lock preventing concurrent commit races. */
  private readonly commitChains = new Map<string, Promise<void>>();

  public constructor(adapter: StorageDriverAdapter) {
    this.adapter = adapter;
  }

  public static async createAuto(options?: {
    readonly forceIdbOnly?: boolean;
    readonly requestPersistence?: boolean;
  }): Promise<PersistentStorageEngine> {
    if (options?.requestPersistence !== false && typeof navigator !== 'undefined') {
      try {
        if (typeof navigator.storage.persist === 'function') {
          await navigator.storage.persist();
        }
      } catch {
        // Non-fatal
      }
    }

    if (IdbStorageBackend.isSupported()) {
      const idb = new IdbStorageBackend();
      const opfsSupported =
        options?.forceIdbOnly === true ? false : await OpfsChunkBackend.isSupported();
      const opfs = opfsSupported ? new OpfsChunkBackend() : null;
      return new PersistentStorageEngine(new BrowserStorageAdapter(idb, opfs));
    }

    return new PersistentStorageEngine(new MemoryStorageAdapter());
  }

  public get mode(): ActiveStorageMode {
    return this.adapter.mode;
  }

  public setMaxOverlayChunksPerProfile(limit: number | null): void {
    this.maxOverlayChunksPerProfile = limit;
  }

  private enqueueExclusive<T>(profileId: string, task: () => Promise<T>): Promise<T> {
    const prev = this.commitChains.get(profileId) ?? Promise.resolve();
    const run = prev.then(task, task);
    this.commitChains.set(
      profileId,
      run.then(
        () => undefined,
        () => undefined,
      ),
    );
    return run;
  }

  /**
   * Open or initialize a profile's persistent overlay disk, cleaning up any uncommitted orphan chunks
   * left behind by an interrupted previous session and returning verified overlay chunks ready for WASM hydration.
   */
  public async openAndRecoverProfile(
    profileId: string,
    defaultCapacityBytes: number,
  ): Promise<{
    readonly manifest: ProfileManifestRecord;
    readonly overlayChunks: readonly BaseChunkInput[];
    readonly recoveredOrphanChunks: number;
  }> {
    validateStorageIdentifier(profileId);
    return this.enqueueExclusive(profileId, async () => {
      let manifest = await this.adapter.getManifest(profileId);
      if (manifest === null) {
        manifest = {
          profileId,
          capacityBytes: defaultCapacityBytes,
          committedGeneration: 0,
          chunkBackend:
            this.adapter.mode === 'opfs+idb'
              ? 'opfs'
              : this.adapter.mode === 'idb-only'
                ? 'indexeddb'
                : 'memory',
          chunks: {},
          updatedAtIso: new Date().toISOString(),
        };
        await this.adapter.putManifest(manifest);
      }

      // 1. Purge any uncommitted or superseded chunk files not referenced by `manifest.chunks`
      const storedList = await this.adapter.listChunks(profileId);
      let recoveredOrphanChunks = 0;
      for (const item of storedList) {
        const slot = manifest.chunks[String(item.chunkIndex)];
        if (!slot || slot.generation !== item.generation) {
          await this.adapter.deleteChunk(profileId, item.chunkIndex, item.generation);
          recoveredOrphanChunks += 1;
        }
      }

      // 2. Load and verify every committed overlay chunk via CRC32
      const overlayChunks: BaseChunkInput[] = [];
      const sortedKeys = Object.keys(manifest.chunks)
        .map(Number)
        .sort((a, b) => a - b);

      for (const chunkIndex of sortedKeys) {
        const slot = manifest.chunks[String(chunkIndex)];
        if (!slot) {
          continue;
        }
        const rec = await this.adapter.readChunk(
          profileId,
          chunkIndex,
          slot.generation,
          slot.crc32,
        );
        overlayChunks.push({
          chunkIndex,
          crc32: rec.crc32,
          data: rec.data,
        });
      }

      return { manifest, overlayChunks, recoveredOrphanChunks };
    });
  }

  /**
   * Crash-consistent two-phase commit for dirty 64 KiB block chunks emitted by the VM:
   * Phase 1: Write all dirty chunks under `nextGeneration = committedGeneration + 1` without touching prior generation.
   * Phase 2: Atomically advance `manifest.committedGeneration` and `manifest.chunks`.
   * Phase 3: Reclaim superseded chunk slots from prior generations.
   */
  public async commitDirtyChunks(
    profileId: string,
    defaultCapacityBytes: number,
    dirtyChunks: readonly DirtyDiskChunk[],
  ): Promise<ProfileManifestRecord> {
    validateStorageIdentifier(profileId);
    return this.enqueueExclusive(profileId, async () => {
      let manifest = await this.adapter.getManifest(profileId);
      if (manifest === null) {
        manifest = {
          profileId,
          capacityBytes: defaultCapacityBytes,
          committedGeneration: 0,
          chunkBackend:
            this.adapter.mode === 'opfs+idb'
              ? 'opfs'
              : this.adapter.mode === 'idb-only'
                ? 'indexeddb'
                : 'memory',
          chunks: {},
          updatedAtIso: new Date().toISOString(),
        };
      }
      if (dirtyChunks.length === 0) {
        return manifest;
      }

      const nextChunks: Record<string, CommittedChunkSlot> = { ...manifest.chunks };
      const nextGen = manifest.committedGeneration + 1;
      const superseded: Array<{ chunkIndex: number; generation: number }> = [];

      // Check soft/configured overlay chunk quota BEFORE writing
      for (const c of dirtyChunks) {
        nextChunks[String(c.chunkIndex)] = { generation: nextGen, crc32: c.crc32 };
      }
      if (
        this.maxOverlayChunksPerProfile !== null &&
        Object.keys(nextChunks).length > this.maxOverlayChunksPerProfile
      ) {
        throw new StorageValidationError(
          'quota_exceeded',
          `Profile '${profileId}' exceeded storage quota limit of ${String(this.maxOverlayChunksPerProfile)} chunks`,
        );
      }

      // Phase 1: Write new generation chunks first (old generation remains untouched on crash!)
      const writtenInBatch: Array<{ chunkIndex: number; generation: number }> = [];
      try {
        for (const c of dirtyChunks) {
          const prevSlot = manifest.chunks[String(c.chunkIndex)];
          if (prevSlot && prevSlot.generation !== nextGen) {
            superseded.push({ chunkIndex: c.chunkIndex, generation: prevSlot.generation });
          }
          const verifiedCrc = await this.adapter.writeChunk(
            profileId,
            c.chunkIndex,
            nextGen,
            c.data,
            c.crc32,
          );
          nextChunks[String(c.chunkIndex)] = { generation: nextGen, crc32: verifiedCrc };
          writtenInBatch.push({ chunkIndex: c.chunkIndex, generation: nextGen });
        }
      } catch (err: unknown) {
        // Clean up partial uncommitted writes immediately while keeping committed generation intact
        for (const item of writtenInBatch) {
          await this.adapter.deleteChunk(profileId, item.chunkIndex, item.generation);
        }
        throw err;
      }

      // Phase 2: Advance committed generation pointer atomically
      const committed: ProfileManifestRecord = {
        ...manifest,
        committedGeneration: nextGen,
        chunks: nextChunks,
        updatedAtIso: new Date().toISOString(),
      };
      await this.adapter.putManifest(committed);

      // Phase 3: Reclaim superseded old-generation chunk files
      for (const old of superseded) {
        await this.adapter.deleteChunk(profileId, old.chunkIndex, old.generation);
      }
      return committed;
    });
  }

  /** Reset ONLY the specified profile's mutable overlay (leaving all other profiles 100% untouched). */
  public async resetProfileDisk(profileId: string, defaultCapacityBytes: number): Promise<void> {
    validateStorageIdentifier(profileId);
    return this.enqueueExclusive(profileId, async () => {
      await this.adapter.clearProfileChunks(profileId);
      const cleanManifest: ProfileManifestRecord = {
        profileId,
        capacityBytes: defaultCapacityBytes,
        committedGeneration: 0,
        chunkBackend:
          this.adapter.mode === 'opfs+idb'
            ? 'opfs'
            : this.adapter.mode === 'idb-only'
              ? 'indexeddb'
              : 'memory',
        chunks: {},
        updatedAtIso: new Date().toISOString(),
      };
      await this.adapter.putManifest(cleanManifest);
    });
  }

  public async exportProfileDiskBundle(
    profileId: string,
    defaultCapacityBytes: number,
  ): Promise<Uint8Array> {
    const recovered = await this.openAndRecoverProfile(profileId, defaultCapacityBytes);
    return encodeDiskOverlayBundle(
      profileId,
      recovered.manifest.capacityBytes,
      recovered.overlayChunks,
    );
  }

  public async importProfileDiskBundle(
    expectedProfileId: string,
    bundle: Uint8Array,
  ): Promise<ProfileManifestRecord> {
    validateStorageIdentifier(expectedProfileId);
    const parsed = decodeAndValidateDiskOverlayBundle(bundle, expectedProfileId);

    return this.enqueueExclusive(expectedProfileId, async () => {
      const prev = await this.adapter.getManifest(expectedProfileId);
      const nextGen = (prev?.committedGeneration ?? 0) + 1;
      const nextChunks: Record<string, CommittedChunkSlot> = {};

      if (
        this.maxOverlayChunksPerProfile !== null &&
        parsed.chunks.length > this.maxOverlayChunksPerProfile
      ) {
        throw new StorageValidationError(
          'quota_exceeded',
          'Imported bundle exceeds profile storage quota',
        );
      }

      for (const c of parsed.chunks) {
        const csum = await this.adapter.writeChunk(
          expectedProfileId,
          c.chunkIndex,
          nextGen,
          c.data,
          c.crc32,
        );
        nextChunks[String(c.chunkIndex)] = { generation: nextGen, crc32: csum };
      }

      const committed: ProfileManifestRecord = {
        profileId: expectedProfileId,
        capacityBytes: parsed.capacityBytes,
        committedGeneration: nextGen,
        chunkBackend:
          this.adapter.mode === 'opfs+idb'
            ? 'opfs'
            : this.adapter.mode === 'idb-only'
              ? 'indexeddb'
              : 'memory',
        chunks: nextChunks,
        updatedAtIso: new Date().toISOString(),
      };
      await this.adapter.putManifest(committed);

      // Remove chunks from older generations
      const all = await this.adapter.listChunks(expectedProfileId);
      for (const item of all) {
        if (item.generation !== nextGen) {
          await this.adapter.deleteChunk(expectedProfileId, item.chunkIndex, item.generation);
        }
      }
      return committed;
    });
  }

  /** Create a named point-in-time disk backup/snapshot (or full VM snapshot) for `profileId`. */
  public async createNamedBackup(
    profileId: string,
    snapshotId: string,
    label: string,
    defaultCapacityBytes: number,
    fullVmSnapshotPayload?: Uint8Array,
  ): Promise<StoredSnapshotRecord> {
    validateStorageIdentifier(profileId);
    validateStorageIdentifier(snapshotId);
    const payload =
      fullVmSnapshotPayload ??
      (await this.exportProfileDiskBundle(profileId, defaultCapacityBytes));
    return this.adapter.saveSnapshot(profileId, snapshotId, label, payload);
  }

  public async loadNamedBackup(
    profileId: string,
    snapshotId: string,
  ): Promise<StoredSnapshotRecord> {
    return this.adapter.loadSnapshot(profileId, snapshotId);
  }

  public async restoreDiskBackup(
    profileId: string,
    snapshotId: string,
  ): Promise<ProfileManifestRecord> {
    const rec = await this.adapter.loadSnapshot(profileId, snapshotId);
    return this.importProfileDiskBundle(profileId, rec.payload);
  }

  public async listNamedBackups(
    profileId: string,
  ): Promise<ReadonlyArray<Omit<StoredSnapshotRecord, 'payload'>>> {
    return this.adapter.listSnapshots(profileId);
  }

  public async deleteNamedBackup(profileId: string, snapshotId: string): Promise<void> {
    await this.adapter.deleteSnapshot(profileId, snapshotId);
  }

  public async getProfileStatus(
    profileId: string,
    defaultCapacityBytes: number,
  ): Promise<ProfileStorageStatus> {
    validateStorageIdentifier(profileId);
    const manifest = await this.adapter.getManifest(profileId);
    const snaps = await this.adapter.listSnapshots(profileId);
    const chunkCount = manifest ? Object.keys(manifest.chunks).length : 0;
    return {
      profileId,
      capacityBytes: manifest?.capacityBytes ?? defaultCapacityBytes,
      committedGeneration: manifest?.committedGeneration ?? 0,
      overlayChunkCount: chunkCount,
      overlayBytes: chunkCount * DISK_CHUNK_BYTES,
      snapshotCount: snaps.length,
      updatedAtIso: manifest?.updatedAtIso ?? null,
    };
  }

  public async inspectQuotaHealth(): Promise<QuotaHealthReport> {
    let persisted = false;
    let usage = 0;
    let quota = 0;
    if (typeof navigator !== 'undefined' && 'storage' in navigator) {
      try {
        if (typeof navigator.storage.persisted === 'function') {
          persisted = await navigator.storage.persisted();
        }
        if (typeof navigator.storage.estimate === 'function') {
          const est = await navigator.storage.estimate();
          usage = est.usage ?? 0;
          quota = est.quota ?? 0;
        }
      } catch {
        // Ignore estimate error
      }
    }
    const available = Math.max(0, quota - usage);
    return {
      mode: this.adapter.mode,
      persisted,
      browserUsageBytes: usage,
      browserQuotaBytes: quota,
      availableBytes: available,
      lowSpaceWarning: quota > 0 && available < 64 * 1024 * 1024,
      evictionRiskWarning: this.adapter.mode === 'memory' || !persisted,
    };
  }

  public setPreference(key: string, value: unknown): Promise<void> {
    return this.adapter.setPreference(key, value);
  }

  public getPreference<T>(key: string, guard: (val: unknown) => val is T): Promise<T | null> {
    return this.adapter.getPreference(key, guard);
  }
}
