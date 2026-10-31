import { JournaledDisk, type SyncFile } from './journal';
interface AccessHandle {
  read(buffer: ArrayBufferView, options: { at: number }): number;
  write(buffer: ArrayBufferView, options: { at: number }): number;
  truncate(size: number): void;
  getSize(): number;
  flush(): void;
  close(): void;
}
type AccessDirectory = FileSystemDirectoryHandle & {
  getFileHandle(
    name: string,
    options?: { create?: boolean },
  ): Promise<FileSystemFileHandle & { createSyncAccessHandle(): Promise<AccessHandle> }>;
};
function adapter(handle: AccessHandle): SyncFile {
  return {
    read(offset, length) {
      const b = new Uint8Array(length);
      const n = handle.read(b, { at: offset });
      if (n !== length) throw new Error('Short OPFS read');
      return b;
    },
    write(offset, bytes) {
      for (let i = 0; i < bytes.length; i += 65536) {
        const chunk = bytes.subarray(i, Math.min(i + 65536, bytes.length));
        const n = handle.write(chunk, { at: offset + i });
        if (n !== chunk.length) throw new Error(`Short OPFS write at ${offset + i}: ${n}/${chunk.length}`);
      }
    },
    truncate: (n) => {
      handle.truncate(n);
    },
    size: () => handle.getSize(),
    flush: () => {
      handle.flush();
    },
  };
}
export async function openDisk(
  profile: string,
  initial: Uint8Array,
  sizeBytes: number,
): Promise<{ disk: JournaledDisk; close(): void }> {
  if (!/^(vanilla|debian)$/.test(profile)) throw new Error('Invalid profile');
  const root = (await navigator.storage.getDirectory()) as AccessDirectory;
  const dir = await root.getDirectoryHandle(`wasmnux-${profile}`, { create: true });
  const fh = await dir.getFileHandle('disk.raw', { create: true });
  const jh = await dir.getFileHandle('journal.bin', { create: true });
  const h = await fh.createSyncAccessHandle(),
    j = await jh.createSyncAccessHandle();
  try {
    if (h.getSize() === 0) {
      if (initial.length !== sizeBytes) throw new Error('Invalid initial disk size');
      for (let offset = 0; offset < initial.length; offset += 65536) {
        const chunk = initial.subarray(offset, Math.min(offset + 65536, initial.length));
        const written = h.write(chunk, { at: offset });
        if (written !== chunk.length)
          throw new Error(`OPFS short write at ${offset}: ${written}/${chunk.length}`);
      }
      h.flush();
    }
    const disk = new JournaledDisk(adapter(h), adapter(j), sizeBytes);
    return {
      disk,
      close() {
        disk.close();
        h.close();
        j.close();
      },
    };
  } catch (e) {
    h.close();
    j.close();
    throw e;
  }
}
