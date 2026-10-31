/**
 * IndexedDB transactional metadata, snapshot, preferences, and fallback chunk store (`wasmnux_storage_v1`).
 */

import { DISK_CHUNK_BYTES } from '../wasm/bindings';
import { crc32, StorageValidationError, validateStorageIdentifier } from './crc32';

const IDB_NAME = 'wasmnux_storage_v1';
const IDB_VERSION = 1;

const STORE_MANIFESTS = 'manifests';
const STORE_CHUNKS = 'chunks';
const STORE_SNAPSHOTS = 'snapshots';
const STORE_PREFS = 'preferences';

export interface CommittedChunkSlot {
  readonly generation: number;
  readonly crc32: number;
}

export interface ProfileManifestRecord {
  readonly profileId: string;
  readonly capacityBytes: number;
  readonly committedGeneration: number;
  readonly chunkBackend: 'opfs' | 'indexeddb' | 'memory';
  readonly chunks: Readonly<Record<string, CommittedChunkSlot>>;
  readonly updatedAtIso: string;
}

export interface StoredSnapshotRecord {
  readonly key: string;
  readonly profileId: string;
  readonly snapshotId: string;
  readonly label: string;
  readonly createdAtIso: string;
  readonly byteLength: number;
  readonly crc32: number;
  readonly payload: Uint8Array;
}

interface IdbChunkRow {
  readonly key: string;
  readonly profileId: string;
  readonly chunkIndex: number;
  readonly generation: number;
  readonly crc32: number;
  readonly data: Uint8Array;
}

function makeChunkKey(profileId: string, chunkIndex: number, generation: number): string {
  return `${profileId}:c${String(chunkIndex)}:g${String(generation)}`;
}

function makeSnapshotKey(profileId: string, snapshotId: string): string {
  return `${profileId}:${snapshotId}`;
}

function promisifyRequest<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = (): void => {
      resolve(req.result);
    };
    req.onerror = (): void => {
      reject(req.error ?? new Error('IndexedDB request failed'));
    };
  });
}

function promisifyTx(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = (): void => {
      resolve();
    };
    tx.onerror = (): void => {
      reject(tx.error ?? new Error('IndexedDB transaction failed'));
    };
    tx.onabort = (): void => {
      reject(tx.error ?? new Error('IndexedDB transaction aborted'));
    };
  });
}

export class IdbStorageBackend {
  private dbPromise: Promise<IDBDatabase> | null = null;

  public static isSupported(): boolean {
    return typeof indexedDB !== 'undefined' && typeof indexedDB.open === 'function';
  }

  private openDb(): Promise<IDBDatabase> {
    if (this.dbPromise !== null) {
      return this.dbPromise;
    }
    this.dbPromise = new Promise((resolve, reject) => {
      const req = indexedDB.open(IDB_NAME, IDB_VERSION);
      req.onupgradeneeded = (): void => {
        const db = req.result;
        if (!db.objectStoreNames.contains(STORE_MANIFESTS)) {
          db.createObjectStore(STORE_MANIFESTS, { keyPath: 'profileId' });
        }
        if (!db.objectStoreNames.contains(STORE_CHUNKS)) {
          const cs = db.createObjectStore(STORE_CHUNKS, { keyPath: 'key' });
          cs.createIndex('by_profile', 'profileId', { unique: false });
        }
        if (!db.objectStoreNames.contains(STORE_SNAPSHOTS)) {
          const ss = db.createObjectStore(STORE_SNAPSHOTS, { keyPath: 'key' });
          ss.createIndex('by_profile', 'profileId', { unique: false });
        }
        if (!db.objectStoreNames.contains(STORE_PREFS)) {
          db.createObjectStore(STORE_PREFS, { keyPath: 'key' });
        }
      };
      req.onsuccess = (): void => {
        resolve(req.result);
      };
      req.onerror = (): void => {
        reject(req.error ?? new Error('Failed opening IndexedDB'));
      };
    });
    return this.dbPromise;
  }

  public async getManifest(profileId: string): Promise<ProfileManifestRecord | null> {
    validateStorageIdentifier(profileId);
    const db = await this.openDb();
    const tx = db.transaction(STORE_MANIFESTS, 'readonly');
    const row = (await promisifyRequest(tx.objectStore(STORE_MANIFESTS).get(profileId))) as
      ProfileManifestRecord | undefined;
    return row ?? null;
  }

  public async putManifest(manifest: ProfileManifestRecord): Promise<void> {
    validateStorageIdentifier(manifest.profileId);
    const db = await this.openDb();
    const tx = db.transaction(STORE_MANIFESTS, 'readwrite');
    tx.objectStore(STORE_MANIFESTS).put(manifest);
    await promisifyTx(tx);
  }

  public async deleteManifest(profileId: string): Promise<void> {
    validateStorageIdentifier(profileId);
    const db = await this.openDb();
    const tx = db.transaction(STORE_MANIFESTS, 'readwrite');
    tx.objectStore(STORE_MANIFESTS).delete(profileId);
    await promisifyTx(tx);
  }

  public async writeChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    data: Uint8Array,
    precomputedCrc32?: number,
  ): Promise<number> {
    validateStorageIdentifier(profileId);
    if (data.byteLength !== DISK_CHUNK_BYTES) {
      throw new StorageValidationError('corrupt_bundle', 'IndexedDB chunk must be 64 KiB');
    }
    const checksum = precomputedCrc32 ?? crc32(data);
    const row: IdbChunkRow = {
      key: makeChunkKey(profileId, chunkIndex, generation),
      profileId,
      chunkIndex,
      generation,
      crc32: checksum,
      data: data.slice(),
    };
    const db = await this.openDb();
    const tx = db.transaction(STORE_CHUNKS, 'readwrite');
    tx.objectStore(STORE_CHUNKS).put(row);
    await promisifyTx(tx);
    return checksum;
  }

  public async readChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    expectedCrc32?: number,
  ): Promise<{ chunkIndex: number; generation: number; crc32: number; data: Uint8Array }> {
    const db = await this.openDb();
    const tx = db.transaction(STORE_CHUNKS, 'readonly');
    const key = makeChunkKey(profileId, chunkIndex, generation);
    const row = (await promisifyRequest(tx.objectStore(STORE_CHUNKS).get(key))) as
      IdbChunkRow | undefined;
    if (!row || row.data.byteLength !== DISK_CHUNK_BYTES) {
      throw new StorageValidationError('corrupt_bundle', `Missing IndexedDB chunk ${key}`);
    }
    const actualCrc = crc32(row.data);
    if (
      actualCrc !== row.crc32 ||
      (expectedCrc32 !== undefined && actualCrc !== expectedCrc32 >>> 0)
    ) {
      throw new StorageValidationError('checksum_mismatch', `IndexedDB chunk CRC mismatch ${key}`);
    }
    return {
      chunkIndex: row.chunkIndex,
      generation: row.generation,
      crc32: actualCrc,
      data: row.data.slice(),
    };
  }

  public async deleteChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
  ): Promise<void> {
    const db = await this.openDb();
    const tx = db.transaction(STORE_CHUNKS, 'readwrite');
    tx.objectStore(STORE_CHUNKS).delete(makeChunkKey(profileId, chunkIndex, generation));
    await promisifyTx(tx);
  }

  public async listChunks(
    profileId: string,
  ): Promise<ReadonlyArray<{ chunkIndex: number; generation: number }>> {
    const db = await this.openDb();
    const tx = db.transaction(STORE_CHUNKS, 'readonly');
    const idx = tx.objectStore(STORE_CHUNKS).index('by_profile');
    const rows = (await promisifyRequest(idx.getAll(profileId))) as IdbChunkRow[];
    return rows.map((r) => ({ chunkIndex: r.chunkIndex, generation: r.generation }));
  }

  public async clearProfileChunks(profileId: string): Promise<void> {
    const db = await this.openDb();
    const tx = db.transaction(STORE_CHUNKS, 'readwrite');
    const store = tx.objectStore(STORE_CHUNKS);
    const rows = (await promisifyRequest(
      store.index('by_profile').getAll(profileId),
    )) as IdbChunkRow[];
    for (const r of rows) {
      store.delete(r.key);
    }
    await promisifyTx(tx);
  }

  public async saveSnapshot(
    profileId: string,
    snapshotId: string,
    label: string,
    payload: Uint8Array,
  ): Promise<StoredSnapshotRecord> {
    validateStorageIdentifier(profileId);
    validateStorageIdentifier(snapshotId);
    const checksum = crc32(payload);
    const record: StoredSnapshotRecord = {
      key: makeSnapshotKey(profileId, snapshotId),
      profileId,
      snapshotId,
      label,
      createdAtIso: new Date().toISOString(),
      byteLength: payload.byteLength,
      crc32: checksum,
      payload: payload.slice(),
    };
    const db = await this.openDb();
    const tx = db.transaction(STORE_SNAPSHOTS, 'readwrite');
    tx.objectStore(STORE_SNAPSHOTS).put(record);
    await promisifyTx(tx);
    return record;
  }

  public async loadSnapshot(profileId: string, snapshotId: string): Promise<StoredSnapshotRecord> {
    validateStorageIdentifier(profileId);
    validateStorageIdentifier(snapshotId);
    const db = await this.openDb();
    const tx = db.transaction(STORE_SNAPSHOTS, 'readonly');
    const row = (await promisifyRequest(
      tx.objectStore(STORE_SNAPSHOTS).get(makeSnapshotKey(profileId, snapshotId)),
    )) as StoredSnapshotRecord | undefined;
    if (!row) {
      throw new StorageValidationError('corrupt_bundle', `Snapshot '${snapshotId}' not found`);
    }
    const actual = crc32(row.payload);
    if (actual !== row.crc32) {
      throw new StorageValidationError(
        'checksum_mismatch',
        `Snapshot '${snapshotId}' CRC32 mismatch`,
      );
    }
    return row;
  }

  public async listSnapshots(
    profileId: string,
  ): Promise<ReadonlyArray<Omit<StoredSnapshotRecord, 'payload'>>> {
    validateStorageIdentifier(profileId);
    const db = await this.openDb();
    const tx = db.transaction(STORE_SNAPSHOTS, 'readonly');
    const rows = (await promisifyRequest(
      tx.objectStore(STORE_SNAPSHOTS).index('by_profile').getAll(profileId),
    )) as StoredSnapshotRecord[];
    return rows.map((r) => ({
      key: r.key,
      profileId: r.profileId,
      snapshotId: r.snapshotId,
      label: r.label,
      createdAtIso: r.createdAtIso,
      byteLength: r.byteLength,
      crc32: r.crc32,
    }));
  }

  public async deleteSnapshot(profileId: string, snapshotId: string): Promise<void> {
    validateStorageIdentifier(profileId);
    validateStorageIdentifier(snapshotId);
    const db = await this.openDb();
    const tx = db.transaction(STORE_SNAPSHOTS, 'readwrite');
    tx.objectStore(STORE_SNAPSHOTS).delete(makeSnapshotKey(profileId, snapshotId));
    await promisifyTx(tx);
  }

  public async clearProfileSnapshots(profileId: string): Promise<void> {
    const db = await this.openDb();
    const tx = db.transaction(STORE_SNAPSHOTS, 'readwrite');
    const store = tx.objectStore(STORE_SNAPSHOTS);
    const rows = (await promisifyRequest(
      store.index('by_profile').getAll(profileId),
    )) as StoredSnapshotRecord[];
    for (const r of rows) {
      store.delete(r.key);
    }
    await promisifyTx(tx);
  }

  public async setPreference(key: string, value: unknown): Promise<void> {
    const db = await this.openDb();
    const tx = db.transaction(STORE_PREFS, 'readwrite');
    tx.objectStore(STORE_PREFS).put({ key, value });
    await promisifyTx(tx);
  }

  public async getPreference<T>(key: string, guard: (val: unknown) => val is T): Promise<T | null> {
    const db = await this.openDb();
    const tx = db.transaction(STORE_PREFS, 'readonly');
    const row = (await promisifyRequest(tx.objectStore(STORE_PREFS).get(key))) as
      { key: string; value: unknown } | undefined;
    if (!row || !guard(row.value)) {
      return null;
    }
    return row.value;
  }

  public close(): void {
    if (this.dbPromise !== null) {
      void this.dbPromise.then((db) => {
        db.close();
      });
      this.dbPromise = null;
    }
  }
}
