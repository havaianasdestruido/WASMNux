import type { Profile } from './protocol';
export interface SnapshotMeta {
  id: string;
  profile: Profile;
  created: number;
  diskHash: string;
  bytes: number;
}
const DB = 'wasmnux-metadata',
  VERSION = 1;
function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains('snapshots'))
        db.createObjectStore('snapshots', { keyPath: 'id' }).createIndex('profile', 'profile');
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error('IndexedDB open failed'));
  });
}
function tx<T>(mode: IDBTransactionMode, fn: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  return open().then(
    (db) =>
      new Promise<T>((resolve, reject) => {
        const t = db.transaction('snapshots', mode);
        const req = fn(t.objectStore('snapshots'));
        t.oncomplete = () => {
          db.close();
          resolve(req.result);
        };
        t.onerror = () => {
          db.close();
          reject(t.error ?? new Error('IndexedDB transaction failed'));
        };
      }),
  );
}
export async function putSnapshot(meta: SnapshotMeta): Promise<void> {
  await tx('readwrite', (s) => s.put(meta));
}
export async function listSnapshots(profile: Profile): Promise<SnapshotMeta[]> {
  const all = await tx('readonly', (s) => s.getAll());
  return (all as SnapshotMeta[]).filter((x) => x.profile === profile).sort((a, b) => b.created - a.created);
}
export async function getSnapshot(id: string): Promise<SnapshotMeta | undefined> {
  return (await tx('readonly', (s) => s.get(id))) as SnapshotMeta | undefined;
}
export async function deleteSnapshot(id: string): Promise<void> {
  await tx('readwrite', (s) => s.delete(id));
  const storage = navigator.storage;
  if (!storage?.getDirectory) return;
  const root = await storage.getDirectory();
  const dir = await root.getDirectoryHandle('wasmnux-snapshots').catch(() => undefined);
  await dir?.removeEntry(id).catch(() => undefined);
}
