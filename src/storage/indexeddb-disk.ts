/**
 * Transactional sparse block storage. This is NOT yet attached to a VM.
 * IndexedDB transaction completion cannot flush a guest's filesystem cache.
 */
export const SECTOR_SIZE = 512;
export const BLOCK_SIZE = 64 * 1024;
export const MAX_IO_SIZE = 8 * 1024 * 1024;
export const MAX_CAPACITY = 4 * 1024 * 1024 * 1024;
export type Profile = "vanilla" | "debian";

export interface DiskInfo {
  profile: Profile;
  capacity: number;
  revision: number;
}

export interface SnapshotInfo {
  profile: Profile;
  id: string;
  name: string;
  capacity: number;
  revision: number;
  createdAt: number;
}

interface BlockRecord {
  profile: Profile;
  index: number;
  data: ArrayBuffer;
}

interface SnapshotBlock extends BlockRecord {
  id: string;
}

function checkProfile(value: unknown): asserts value is Profile {
  if (value !== "vanilla" && value !== "debian") throw new Error("Invalid disk profile");
}

function checkCapacity(capacity: number): void {
  if (
    !Number.isSafeInteger(capacity) ||
    capacity <= 0 ||
    capacity > MAX_CAPACITY ||
    capacity % SECTOR_SIZE !== 0
  ) {
    throw new Error("Invalid disk capacity");
  }
}

function checkRange(offset: number, length: number, capacity: number): void {
  if (
    !Number.isSafeInteger(offset) ||
    !Number.isSafeInteger(length) ||
    offset < 0 ||
    length <= 0 ||
    length > MAX_IO_SIZE ||
    offset % SECTOR_SIZE !== 0 ||
    length % SECTOR_SIZE !== 0 ||
    offset + length > capacity
  ) {
    throw new Error("Invalid disk I/O range");
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object";
}

function decodeDisk(value: unknown, profile: Profile): DiskInfo {
  if (
    !isRecord(value) ||
    value.profile !== profile ||
    typeof value.capacity !== "number" ||
    typeof value.revision !== "number" ||
    !Number.isSafeInteger(value.revision) ||
    value.revision < 0
  ) {
    throw new Error("Disk metadata is missing or corrupt");
  }
  checkCapacity(value.capacity);
  return { profile, capacity: value.capacity, revision: value.revision };
}

function decodeBlock(value: unknown, profile: Profile, index: number): BlockRecord {
  if (
    !isRecord(value) ||
    value.profile !== profile ||
    value.index !== index ||
    !(value.data instanceof ArrayBuffer) ||
    value.data.byteLength !== BLOCK_SIZE
  ) {
    throw new Error("Disk block is corrupt");
  }
  return { profile, index, data: value.data };
}

function decodeSnapshot(value: unknown, profile: Profile, id: string): SnapshotInfo {
  if (
    !isRecord(value) ||
    value.profile !== profile ||
    value.id !== id ||
    typeof value.name !== "string" ||
    value.name.length === 0 ||
    value.name.length > 80 ||
    typeof value.capacity !== "number" ||
    typeof value.revision !== "number" ||
    !Number.isSafeInteger(value.revision) ||
    value.revision < 0 ||
    typeof value.createdAt !== "number" ||
    !Number.isSafeInteger(value.createdAt) ||
    value.createdAt < 0
  ) {
    throw new Error("Snapshot metadata is missing or corrupt");
  }
  checkCapacity(value.capacity);
  return {
    profile,
    id,
    name: value.name,
    capacity: value.capacity,
    revision: value.revision,
    createdAt: value.createdAt,
  };
}

function nextRevision(revision: number): number {
  if (revision >= Number.MAX_SAFE_INTEGER) throw new Error("Disk revision exhausted");
  return revision + 1;
}

function request<T>(operation: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    operation.onsuccess = () => resolve(operation.result);
    operation.onerror = () => reject(operation.error ?? new Error("IndexedDB request failed"));
  });
}

function completion(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.addEventListener("complete", () => resolve(), { once: true });
    transaction.addEventListener(
      "abort",
      () => reject(transaction.error ?? new Error("IndexedDB transaction aborted")),
      {
        once: true,
      },
    );
  });
}

function walk(
  cursorRequest: IDBRequest<IDBCursorWithValue | null>,
  visit: (cursor: IDBCursorWithValue) => void,
): Promise<void> {
  return new Promise((resolve, reject) => {
    cursorRequest.onerror = () =>
      reject(cursorRequest.error ?? new Error("IndexedDB cursor failed"));
    cursorRequest.onsuccess = () => {
      const cursor = cursorRequest.result;
      if (!cursor) {
        resolve();
        return;
      }
      try {
        visit(cursor);
        cursor.continue();
      } catch (error) {
        reject(error);
      }
    };
  });
}

function checkBlockIndex(index: unknown, capacity: number): asserts index is number {
  if (
    typeof index !== "number" ||
    !Number.isSafeInteger(index) ||
    index < 0 ||
    index >= Math.ceil(capacity / BLOCK_SIZE)
  ) {
    throw new Error("Disk block index is corrupt");
  }
}

function snapshotId(id: string): void {
  if (
    typeof id !== "string" ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(id)
  ) {
    throw new Error("Invalid snapshot id");
  }
}

export class IndexedDbDiskStore {
  private constructor(private readonly db: IDBDatabase) {}

  static async open(databaseName = "wasmnux-disks-v1"): Promise<IndexedDbDiskStore> {
    if (!/^[a-zA-Z0-9._-]{1,64}$/.test(databaseName)) throw new Error("Invalid database name");
    if (typeof indexedDB === "undefined") throw new Error("IndexedDB is unavailable");
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const opening = indexedDB.open(databaseName, 2);
      let settled = false;
      opening.onupgradeneeded = () => {
        const database = opening.result;
        const upgrade = opening.transaction;
        if (!upgrade) throw new Error("Missing IndexedDB upgrade transaction");
        if (!database.objectStoreNames.contains("disks"))
          database.createObjectStore("disks", { keyPath: "profile" });
        const blocks = database.objectStoreNames.contains("blocks")
          ? upgrade.objectStore("blocks")
          : database.createObjectStore("blocks", { keyPath: ["profile", "index"] });
        if (!blocks.indexNames.contains("by-profile")) blocks.createIndex("by-profile", "profile");
        if (!database.objectStoreNames.contains("snapshots"))
          database.createObjectStore("snapshots", { keyPath: ["profile", "id"] });
        const snapshotBlocks = database.objectStoreNames.contains("snapshotBlocks")
          ? upgrade.objectStore("snapshotBlocks")
          : database.createObjectStore("snapshotBlocks", { keyPath: ["profile", "id", "index"] });
        if (!snapshotBlocks.indexNames.contains("by-snapshot"))
          snapshotBlocks.createIndex("by-snapshot", ["profile", "id"]);
      };
      opening.onerror = () => {
        if (!settled) reject(opening.error ?? new Error("Could not open disk database"));
        settled = true;
      };
      opening.onblocked = () => {
        if (!settled) reject(new Error("Disk database upgrade blocked by another tab"));
        settled = true;
      };
      opening.onsuccess = () => {
        const opened = opening.result;
        if (settled) opened.close();
        else {
          settled = true;
          opened.onversionchange = () => opened.close();
          resolve(opened);
        }
      };
    });
    return new IndexedDbDiskStore(db);
  }

  close(): void {
    this.db.close();
  }

  private async transaction<T>(
    stores: string[],
    mode: IDBTransactionMode,
    operation: (transaction: IDBTransaction) => Promise<T>,
  ): Promise<T> {
    const transaction = this.db.transaction(
      stores,
      mode,
      mode === "readwrite" ? { durability: "strict" } : undefined,
    );
    const finished = completion(transaction);
    // Handle an early abort immediately; the same rejection is still awaited below.
    void finished.catch(() => undefined);
    try {
      const result = await operation(transaction);
      await finished;
      return result;
    } catch (error) {
      try {
        transaction.abort();
      } catch (abortError) {
        // An already completed/aborted transaction cannot be aborted again.
        if (!(abortError instanceof DOMException) || abortError.name !== "InvalidStateError")
          throw abortError;
      }
      await finished.catch(() => undefined);
      throw error;
    }
  }

  async create(profile: Profile, capacity: number): Promise<DiskInfo> {
    checkProfile(profile);
    checkCapacity(capacity);
    return this.transaction(["disks"], "readwrite", async (tx) => {
      const store = tx.objectStore("disks");
      const existing: unknown = await request(store.get(profile));
      if (existing !== undefined) throw new Error("Disk already exists");
      const info: DiskInfo = { profile, capacity, revision: 0 };
      await request(store.add(info));
      return info;
    });
  }

  async info(profile: Profile): Promise<DiskInfo | null> {
    checkProfile(profile);
    return this.transaction(["disks"], "readonly", async (tx) => {
      const value: unknown = await request(tx.objectStore("disks").get(profile));
      return value === undefined ? null : decodeDisk(value, profile);
    });
  }

  async read(profile: Profile, offset: number, length: number): Promise<Uint8Array> {
    checkProfile(profile);
    return this.transaction(["disks", "blocks"], "readonly", async (tx) => {
      const info = decodeDisk(await request(tx.objectStore("disks").get(profile)), profile);
      checkRange(offset, length, info.capacity);
      const result = new Uint8Array(length);
      const blocks = tx.objectStore("blocks");
      let position = 0;
      while (position < length) {
        const absolute = offset + position;
        const index = Math.floor(absolute / BLOCK_SIZE);
        const within = absolute % BLOCK_SIZE;
        const count = Math.min(length - position, BLOCK_SIZE - within);
        const value: unknown = await request(blocks.get([profile, index]));
        if (value !== undefined) {
          const data = new Uint8Array(decodeBlock(value, profile, index).data);
          result.set(data.subarray(within, within + count), position);
        }
        position += count;
      }
      return result;
    });
  }

  async write(profile: Profile, offset: number, bytes: Uint8Array): Promise<DiskInfo> {
    checkProfile(profile);
    if (!(bytes instanceof Uint8Array) || bytes.byteLength > MAX_IO_SIZE)
      throw new Error("Invalid disk write buffer");
    const input = bytes.slice();
    return this.transaction(["disks", "blocks"], "readwrite", async (tx) => {
      const disks = tx.objectStore("disks");
      const info = decodeDisk(await request(disks.get(profile)), profile);
      checkRange(offset, input.byteLength, info.capacity);
      const blocks = tx.objectStore("blocks");
      let position = 0;
      while (position < input.byteLength) {
        const absolute = offset + position;
        const index = Math.floor(absolute / BLOCK_SIZE);
        const within = absolute % BLOCK_SIZE;
        const count = Math.min(input.byteLength - position, BLOCK_SIZE - within);
        const existing: unknown = await request(blocks.get([profile, index]));
        const data =
          existing === undefined
            ? new Uint8Array(BLOCK_SIZE)
            : new Uint8Array(decodeBlock(existing, profile, index).data);
        data.set(input.subarray(position, position + count), within);
        await request(blocks.put({ profile, index, data: data.buffer } satisfies BlockRecord));
        position += count;
      }
      const updated = { ...info, revision: nextRevision(info.revision) };
      await request(disks.put(updated));
      return updated;
    });
  }

  /** Clear just this disk; its existing snapshots are retained. */
  async reset(profile: Profile): Promise<DiskInfo> {
    checkProfile(profile);
    return this.transaction(["disks", "blocks"], "readwrite", async (tx) => {
      const disks = tx.objectStore("disks");
      const info = decodeDisk(await request(disks.get(profile)), profile);
      await walk(
        tx.objectStore("blocks").index("by-profile").openCursor(IDBKeyRange.only(profile)),
        (cursor) => cursor.delete(),
      );
      const updated = { ...info, revision: nextRevision(info.revision) };
      await request(disks.put(updated));
      return updated;
    });
  }

  /** An atomic copy of currently committed blocks, not a live VM/RAM snapshot. */
  async snapshot(profile: Profile, name: string): Promise<SnapshotInfo> {
    checkProfile(profile);
    if (typeof name !== "string" || name.trim().length === 0 || name.length > 80)
      throw new Error("Invalid snapshot name");
    const id = crypto.randomUUID();
    return this.transaction(
      ["disks", "blocks", "snapshots", "snapshotBlocks"],
      "readwrite",
      async (tx) => {
        const info = decodeDisk(await request(tx.objectStore("disks").get(profile)), profile);
        const snapshot: SnapshotInfo = { ...info, id, name: name.trim(), createdAt: Date.now() };
        await request(tx.objectStore("snapshots").add(snapshot));
        const target = tx.objectStore("snapshotBlocks");
        await walk(
          tx.objectStore("blocks").index("by-profile").openCursor(IDBKeyRange.only(profile)),
          (cursor) => {
            const key: unknown = cursor.primaryKey;
            if (!Array.isArray(key) || key.length !== 2 || key[0] !== profile)
              throw new Error("Disk block key is corrupt");
            checkBlockIndex(key[1], info.capacity);
            const block = decodeBlock(cursor.value, profile, key[1]);
            target.add({ ...block, id } satisfies SnapshotBlock);
          },
        );
        return snapshot;
      },
    );
  }

  /** Restore is atomic with respect to all disk writes and snapshots. */
  async restore(profile: Profile, id: string): Promise<DiskInfo> {
    checkProfile(profile);
    snapshotId(id);
    return this.transaction(
      ["disks", "blocks", "snapshots", "snapshotBlocks"],
      "readwrite",
      async (tx) => {
        const disks = tx.objectStore("disks");
        const info = decodeDisk(await request(disks.get(profile)), profile);
        const saved = decodeSnapshot(
          await request(tx.objectStore("snapshots").get([profile, id])),
          profile,
          id,
        );
        if (saved.capacity !== info.capacity)
          throw new Error("Snapshot capacity differs from disk");
        const blocks = tx.objectStore("blocks");
        await walk(blocks.index("by-profile").openCursor(IDBKeyRange.only(profile)), (cursor) =>
          cursor.delete(),
        );
        await walk(
          tx
            .objectStore("snapshotBlocks")
            .index("by-snapshot")
            .openCursor(IDBKeyRange.only([profile, id])),
          (cursor) => {
            const key: unknown = cursor.primaryKey;
            if (!Array.isArray(key) || key.length !== 3 || key[0] !== profile || key[1] !== id)
              throw new Error("Snapshot block key is corrupt");
            checkBlockIndex(key[2], info.capacity);
            const block = decodeBlock(cursor.value, profile, key[2]);
            blocks.add(block);
          },
        );
        const updated = { ...info, revision: nextRevision(info.revision) };
        await request(disks.put(updated));
        return updated;
      },
    );
  }
}
