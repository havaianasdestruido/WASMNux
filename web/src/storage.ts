/**
 * Persistent virtual storage for WASMNux.
 *
 * Virtual disk blocks live in Origin Private File System (OPFS) files; VM
 * metadata, configs and snapshots live in IndexedDB. `navigator.storage`
 * is used to detect persistence status and quota. All guest storage is
 * sandboxed to the origin — the host filesystem is never exposed.
 */

import type { PersistenceStatus, StorageInfo } from "./types";

const DB_NAME = "wasmnux";
const DB_VERSION = 1;
const META_STORE = "metadata";
const SNAPSHOT_STORE = "snapshots";

export class StorageError extends Error {
  constructor(
    public code: number,
    message: string,
  ) {
    super(message);
    this.name = "StorageError";
  }

  static quota(): StorageError {
    return new StorageError(-1, "storage quota exhausted");
  }
}

type StorageLike = Storage & {
  getDirectory?: () => Promise<FileSystemDirectoryHandle>;
  persist?: () => Promise<boolean>;
  estimate?: () => Promise<{ quota: number; usage: number }>;
};

function storageLike(): StorageLike | null {
  if (typeof navigator === "undefined") {
    return null;
  }
  const storage = navigator.storage as unknown as StorageLike | undefined;
  if (!storage) {
    return null;
  }
  return storage;
}

export async function detectPersistence(): Promise<StorageInfo> {
  const storage = storageLike();
  let persistence: PersistenceStatus = "unknown";
  let durable = false;
  let quota = 0;
  let used = 0;

  if (storage?.persist !== undefined) {
    durable = await storage.persist();
    persistence = durable ? "granted" : "denied";
  }
  if (storage?.estimate) {
    const est = await storage.estimate();
    quota = est.quota;
    used = est.usage ?? 0;
    if (quota > 0 && used / quota > 0.9) {
      persistence = "quota-pressure";
    }
  }

  return { used, quota, persistence, durable };
}

function openDb(): Promise<IDBDatabase> {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(META_STORE)) {
        db.createObjectStore(META_STORE, { keyPath: "id" });
      }
      if (!db.objectStoreNames.contains(SNAPSHOT_STORE)) {
        db.createObjectStore(SNAPSHOT_STORE, { keyPath: "id" });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
    req.onblocked = () => reject(new Error("indexedDB blocked"));
  });
}

export class MetadataStore {
  private dbPromise: Promise<IDBDatabase>;

  constructor() {
    this.dbPromise = openDb();
  }

  private async run<T>(
    store: string,
    mode: IDBTransactionMode,
    fn: (s: IDBObjectStore) => IDBRequest,
  ): Promise<T> {
    const db = await this.dbPromise;
    return new Promise<T>((resolve, reject) => {
      const tx = db.transaction(store, mode);
      const s = tx.objectStore(store);
      const req = fn(s);
      req.onsuccess = () => resolve(req.result as T);
      req.onerror = () => reject(req.error);
      tx.onabort = () => reject(tx.error);
    });
  }

  async get<T = unknown>(store: string, id: string): Promise<T | undefined> {
    return this.run(store, "readonly", (s) => s.get(id));
  }

  async put<T>(store: string, value: T & { id: string }): Promise<void> {
    await this.run(store, "readwrite", (s) => s.put(value));
  }

  async remove(store: string, id: string): Promise<void> {
    await this.run(store, "readwrite", (s) => s.delete(id));
  }

  async list<T = unknown>(store: string): Promise<T[]> {
    return this.run(store, "readonly", (s) => s.getAll());
  }
}

export const metadata = new MetadataStore();

/**
 * OPFS-backed virtual block device.
 */
export class OpfsDisk {
  private readonly path: string;
  private root: FileSystemDirectoryHandle | null = null;

  constructor(path: string) {
    this.path = path;
  }

  async ready(): Promise<boolean> {
    if (!this.root) {
      await this.ensureRoot();
    }
    return this.root !== null;
  }

  private async ensureRoot(): Promise<void> {
    const storage = storageLike();
    const getDir = storage?.getDirectory;
    if (!getDir) {
      return;
    }
    try {
      this.root = await getDir();
    } catch {
      this.root = null;
    }
  }

  private async file(create: boolean): Promise<FileSystemFileHandle | null> {
    if (!this.root) {
      return null;
    }
    try {
      return await this.root.getFileHandle(this.path, { create });
    } catch {
      return null;
    }
  }

  async size(): Promise<number> {
    const fh = await this.file(false);
    if (!fh) {
      return 0;
    }
    const file = await fh.getFile();
    return file.size;
  }

  async read(
    block: number,
    blockSize: number,
    count: number,
  ): Promise<ArrayBuffer> {
    const fh = await this.file(false);
    if (!fh) {
      throw new StorageError(-1, "disk file not found");
    }
    const file = await fh.getFile();
    const start = block * blockSize;
    const slice = file.slice(start, start + count * blockSize);
    return slice.arrayBuffer();
  }

  async write(
    block: number,
    blockSize: number,
    data: Uint8Array,
  ): Promise<void> {
    const fh = await this.file(true);
    if (!fh) {
      throw new StorageError(-1, "cannot open disk file for write");
    }
    const current = await this.size();
    const view = new Uint8Array(
      Math.max(current, block * blockSize + data.byteLength),
    );
    const existing = await this.read(0, 1, Math.max(1, Math.ceil(current)));
    view.set(new Uint8Array(existing));
    view.set(data, block * blockSize);
    const writable = await fh.createWritable();
    await writable.write(view.buffer);
    await writable.close();
  }

  async exportToArrayBuffer(): Promise<ArrayBuffer> {
    const fh = await this.file(false);
    if (!fh) {
      return new ArrayBuffer(0);
    }
    const file = await fh.getFile();
    return file.arrayBuffer();
  }

  async importFromArrayBuffer(data: ArrayBuffer): Promise<void> {
    const fh = await this.file(true);
    if (!fh) {
      throw new StorageError(-1, "cannot open disk file for import");
    }
    const writable = await fh.createWritable();
    await writable.write(data);
    await writable.close();
  }

  async delete(): Promise<void> {
    if (!this.root) {
      return;
    }
    try {
      await this.root.removeEntry(this.path);
    } catch {
      // ignore
    }
  }
}
