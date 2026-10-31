import { IndexedDbDiskStore, type Profile } from "../../src/storage/indexeddb-disk";

let disk: IndexedDbDiskStore | undefined;
const databaseName = "wasmnux-storage-test";
function current(): IndexedDbDiskStore {
  if (!disk) throw new Error("Store is not open");
  return disk;
}

async function injectRecord(store: "blocks" | "snapshotBlocks", value: unknown): Promise<void> {
  const database = await new Promise<IDBDatabase>((resolve, reject) => {
    const open = indexedDB.open(databaseName);
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error);
  });
  try {
    await new Promise<void>((resolve, reject) => {
      const transaction = database.transaction(store, "readwrite");
      transaction.objectStore(store).put(value);
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(transaction.error);
    });
  } finally {
    database.close();
  }
}

const api = {
  async open() {
    disk?.close();
    disk = await IndexedDbDiskStore.open(databaseName);
  },
  close() {
    disk?.close();
    disk = undefined;
  },
  create(profile: Profile, capacity: number) {
    return current().create(profile, capacity);
  },
  info(profile: Profile) {
    return current().info(profile);
  },
  async read(profile: Profile, offset: number, length: number) {
    return Array.from(await current().read(profile, offset, length));
  },
  write(profile: Profile, offset: number, data: number[]) {
    return current().write(profile, offset, Uint8Array.from(data));
  },
  reset(profile: Profile) {
    return current().reset(profile);
  },
  snapshot(profile: Profile, name: string) {
    return current().snapshot(profile, name);
  },
  restore(profile: Profile, id: string) {
    return current().restore(profile, id);
  },
  corruptSnapshot(profile: Profile, id: string) {
    return injectRecord("snapshotBlocks", { profile, id, index: 0, data: new ArrayBuffer(1) });
  },
  injectBlock(profile: Profile, index: number | string | readonly number[]) {
    return injectRecord("blocks", { profile, index, data: new ArrayBuffer(65536) });
  },
  injectSnapshotBlock(profile: Profile, id: string, index: number | string | readonly number[]) {
    return injectRecord("snapshotBlocks", { profile, id, index, data: new ArrayBuffer(65536) });
  },
  writeRandom(profile: Profile, offset: number, length: number) {
    const bytes = new Uint8Array(length);
    for (let position = 0; position < bytes.length; position += 65536) {
      crypto.getRandomValues(bytes.subarray(position, Math.min(position + 65536, bytes.length)));
    }
    return current().write(profile, offset, bytes);
  },
};

declare global {
  interface Window {
    diskTest: typeof api;
  }
}
window.diskTest = api;
