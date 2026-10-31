/**
 * ChunkStore: the one small key/value-of-bytes abstraction the persistent
 * virtual disk is built on. Three backends:
 *  - OPFS (fast path, bytes on disk in the origin-private filesystem)
 *  - IndexedDB (portable fallback, also backs metadata)
 *  - Memory (unit tests and browsers with no durable storage; the UI must
 *    say loudly when this is in use — data will not survive a reload)
 *
 * Keys are logical slash-separated paths ("profiles/vanilla/chunks/…").
 * They come only from our own code and validated profile ids; backends
 * reject traversal-style keys defensively.
 */

export function assertSafeKey(key: string): string {
  if (key.length === 0 || key.startsWith("/") || key.includes("\\")) {
    throw new Error(`unsafe storage key: ${JSON.stringify(key)}`);
  }
  for (const part of key.split("/")) {
    if (part === "" || part === "." || part === "..") {
      throw new Error(`unsafe storage key: ${JSON.stringify(key)}`);
    }
  }
  return key;
}

export interface ChunkStore {
  readonly kind: "opfs" | "indexeddb" | "memory";
  get(key: string): Promise<Uint8Array | undefined>;
  put(key: string, data: Uint8Array): Promise<void>;
  delete(key: string): Promise<void>;
  list(prefix: string): Promise<string[]>;
}

/** In-memory store. Non-persistent: test backend / explicit last resort. */
export class MemoryChunkStore implements ChunkStore {
  readonly kind = "memory" as const;
  readonly #map = new Map<string, Uint8Array>();

  get(key: string): Promise<Uint8Array | undefined> {
    const hit = this.#map.get(assertSafeKey(key));
    return Promise.resolve(hit === undefined ? undefined : new Uint8Array(hit));
  }

  put(key: string, data: Uint8Array): Promise<void> {
    this.#map.set(assertSafeKey(key), new Uint8Array(data));
    return Promise.resolve();
  }

  delete(key: string): Promise<void> {
    this.#map.delete(assertSafeKey(key));
    return Promise.resolve();
  }

  list(prefix: string): Promise<string[]> {
    return Promise.resolve([...this.#map.keys()].filter((k) => k.startsWith(prefix)));
  }

  /** Test helper: direct view without copying (tests tamper through here). */
  rawSet(key: string, data: Uint8Array): void {
    this.#map.set(key, data);
  }

  get size(): number {
    return this.#map.size;
  }
}

/** OPFS-backed store: one OPFS file per key, mirroring the slash path. */
export class OpfsChunkStore implements ChunkStore {
  readonly kind = "opfs" as const;
  readonly #root: Promise<FileSystemDirectoryHandle>;

  constructor() {
    this.#root = navigator.storage.getDirectory();
  }

  static isSupported(): boolean {
    return (
      typeof navigator !== "undefined" &&
      typeof navigator.storage?.getDirectory === "function"
    );
  }

  async #dirFor(parts: readonly string[], create: boolean): Promise<FileSystemDirectoryHandle> {
    let dir = await this.#root;
    for (const part of parts) {
      dir = await dir.getDirectoryHandle(part, { create });
    }
    return dir;
  }

  async get(key: string): Promise<Uint8Array | undefined> {
    assertSafeKey(key);
    const parts = key.split("/");
    const leaf = parts[parts.length - 1] as string;
    try {
      const dir = await this.#dirFor(parts.slice(0, -1), false);
      const handle = await dir.getFileHandle(leaf, { create: false });
      const file = await handle.getFile();
      return new Uint8Array(await file.arrayBuffer());
    } catch (error) {
      if (error instanceof DOMException && error.name === "NotFoundError") {
        return undefined;
      }
      throw error;
    }
  }

  async put(key: string, data: Uint8Array): Promise<void> {
    assertSafeKey(key);
    const parts = key.split("/");
    const leaf = parts[parts.length - 1] as string;
    const dir = await this.#dirFor(parts.slice(0, -1), true);
    const handle = await dir.getFileHandle(leaf, { create: true });
    // createWritable swaps the file atomically on close where supported,
    // which is what the disk-manifest crash-consistency story relies on.
    const writable: FileSystemWritableFileStream = await handle.createWritable({
      keepExistingData: false,
    });
    await writable.write(data as BufferSource);
    await writable.close();
  }

  async delete(key: string): Promise<void> {
    assertSafeKey(key);
    const parts = key.split("/");
    const leaf = parts[parts.length - 1] as string;
    try {
      const dir = await this.#dirFor(parts.slice(0, -1), false);
      await dir.removeEntry(leaf);
    } catch (error) {
      if (error instanceof DOMException && error.name === "NotFoundError") {
        return;
      }
      throw error;
    }
  }

  async list(prefix: string): Promise<string[]> {
    const results: string[] = [];
    const walk = async (dir: FileSystemDirectoryHandle, path: string): Promise<void> => {
      // values() is part of the OPFS spec; this cast covers the installed
      // DOM+WebWorker lib combination, which types it incompletely.
      const entries = (
        dir as unknown as { values(): AsyncIterable<FileSystemHandle> }
      ).values();
      for await (const entry of entries) {
        const childPath = path === "" ? entry.name : `${path}/${entry.name}`;
        if (entry.kind === "file") {
          if (childPath.startsWith(prefix)) {
            results.push(childPath);
          }
        } else if (entry.kind === "directory") {
          await walk(entry as FileSystemDirectoryHandle, childPath);
        }
      }
    };
    await walk(await this.#root, "");
    return results;
  }
}

const IDB_DATABASE = "wasmnux";
const IDB_STORE = "chunks";

/** IndexedDB-backed store. One object store; values are Uint8Array blobs. */
export class IdbChunkStore implements ChunkStore {
  readonly kind = "indexeddb" as const;
  #db: Promise<IDBDatabase> | undefined;

  static isSupported(): boolean {
    return typeof indexedDB !== "undefined";
  }

  #open(): Promise<IDBDatabase> {
    this.#db ??= new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open(IDB_DATABASE, 1);
      request.onupgradeneeded = () => {
        request.result.createObjectStore(IDB_STORE);
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error ?? new Error("IndexedDB open failed"));
    });
    return this.#db;
  }

  async #tx<T>(mode: IDBTransactionMode, run: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
    const db = await this.#open();
    return new Promise<T>((resolve, reject) => {
      const tx = db.transaction(IDB_STORE, mode);
      const request = run(tx.objectStore(IDB_STORE));
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error ?? new Error("IndexedDB request failed"));
      tx.onerror = () => reject(tx.error ?? new Error("IndexedDB transaction failed"));
    });
  }

  async get(key: string): Promise<Uint8Array | undefined> {
    assertSafeKey(key);
    const value: unknown = await this.#tx("readonly", (store) => store.get(key));
    return value instanceof Uint8Array ? value : undefined;
  }

  async put(key: string, data: Uint8Array): Promise<void> {
    assertSafeKey(key);
    await this.#tx("readwrite", (store) => store.put(data, key));
  }

  async delete(key: string): Promise<void> {
    assertSafeKey(key);
    await this.#tx("readwrite", (store) => store.delete(key));
  }

  async list(prefix: string): Promise<string[]> {
    const keys = await this.#tx("readonly", (store) => store.getAllKeys());
    return keys.map((k) => String(k)).filter((k) => k.startsWith(prefix));
  }
}

/**
 * Picks the best available durable store: OPFS first, then IndexedDB.
 * Returns `undefined` when the browser offers nothing durable; callers
 * must then either refuse to create disks or use MemoryChunkStore with an
 * explicit non-persistence warning in the UI.
 */
export function openDurableStore(): ChunkStore | undefined {
  if (OpfsChunkStore.isSupported()) {
    return new OpfsChunkStore();
  }
  if (IdbChunkStore.isSupported()) {
    return new IdbChunkStore();
  }
  return undefined;
}
