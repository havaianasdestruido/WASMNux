import type { Profile } from './protocol';
export const DISK_BYTES: Record<Profile, number> = { vanilla: 64 * 1024 * 1024, debian: 256 * 1024 * 1024 };
async function profileDir(profile: Profile, create = false): Promise<FileSystemDirectoryHandle> {
  const root = await navigator.storage.getDirectory();
  return root.getDirectoryHandle(`wasmnux-${profile}`, { create });
}
async function withLock<T>(profile: Profile, fn: () => Promise<T>): Promise<T> {
  if (!navigator.locks) throw new Error('Web Locks unavailable');
  const result = await navigator.locks.request(`wasmnux-${profile}`, { ifAvailable: true }, async (lock) => {
    if (!lock) throw new Error('Profile is open in another tab or worker');
    return fn();
  });
  return result;
}
/** Validates a raw ext2/3/4 image before it can replace a profile disk. */
export function validateDiskImage(profile: Profile, bytes: Uint8Array): void {
  if (bytes.length !== DISK_BYTES[profile])
    throw new Error(`Expected an exact ${DISK_BYTES[profile] / 1048576} MiB raw image`);
  if (bytes.length < 2048 || bytes[1024 + 56] !== 0x53 || bytes[1024 + 57] !== 0xef)
    throw new Error('Not a valid ext2/3/4 superblock');
}
export async function resetProfile(profile: Profile): Promise<void> {
  await withLock(profile, async () => {
    const root = await navigator.storage.getDirectory();
    await root.removeEntry(`wasmnux-${profile}`, { recursive: true }).catch((e: unknown) => {
      if (!(e instanceof DOMException && e.name === 'NotFoundError')) throw e;
    });
  });
}
export async function importProfile(profile: Profile, bytes: Uint8Array): Promise<void> {
  validateDiskImage(profile, bytes);
  await withLock(profile, async () => {
    const dir = await profileDir(profile, true);
    // Remove an old journal first: it must never replay onto a different imported disk.
    await dir.removeEntry('journal.bin').catch((e: unknown) => {
      if (!(e instanceof DOMException && e.name === 'NotFoundError')) throw e;
    });
    const fh = await dir.getFileHandle('disk.raw', { create: true });
    const w = await fh.createWritable();
    try {
      await w.write(bytes.buffer as ArrayBuffer);
      await w.close();
    } catch (e) {
      await w.abort().catch(() => undefined);
      throw e;
    }
  });
}
export async function exportProfile(profile: Profile): Promise<Uint8Array> {
  return withLock(profile, async () => {
    const dir = await profileDir(profile);
    const fh = await dir.getFileHandle('disk.raw');
    const file = await fh.getFile();
    if (file.size !== DISK_BYTES[profile]) throw new Error('Stored disk has an unexpected size');
    const journal = await dir.getFileHandle('journal.bin').catch(() => undefined);
    if (journal && (await journal.getFile()).size !== 0)
      throw new Error('Disk has an unapplied journal; boot once to recover before exporting');
    return new Uint8Array(await file.arrayBuffer());
  });
}
export async function cachedAsset(name: string, hash: string, baseUrl: URL): Promise<Uint8Array> {
  const root = await navigator.storage.getDirectory();
  const dir = await root.getDirectoryHandle('wasmnux-assets', { create: true });
  const fh = await dir.getFileHandle(name, { create: true });
  const cached = new Uint8Array(await (await fh.getFile()).arrayBuffer());
  if (cached.length && (await sha256(cached)) === hash) return cached;
  const response = await fetch(new URL(name, baseUrl));
  if (!response.ok) throw new Error(`Asset ${name}: HTTP ${response.status.toString()}`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  if ((await sha256(bytes)) !== hash) throw new Error(`Asset checksum mismatch: ${name}`);
  const w = await fh.createWritable();
  try {
    await w.write(bytes.buffer);
    await w.close();
  } catch {
    await w.abort().catch(() => undefined); /* cache is optional */
  }
  return bytes;
}
export async function sha256(bytes: Uint8Array): Promise<string> {
  return [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes.buffer as ArrayBuffer))]
    .map((x) => x.toString(16).padStart(2, '0'))
    .join('');
}
