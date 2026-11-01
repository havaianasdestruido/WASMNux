/**
 * Origin Private File System (`OPFS`) versioned 64 KiB chunk store with `WNXC` header + CRC32 validation.
 */

import { DISK_CHUNK_BYTES } from '../wasm/bindings';
import { crc32, StorageValidationError, validateStorageIdentifier } from './crc32';

const OPFS_ROOT_DIR = 'wasmnux-v1';
const CHUNK_FILE_MAGIC = 0x4358_4e57; // "WNXC" little-endian
const CHUNK_FILE_VERSION = 1;
const CHUNK_HEADER_BYTES = 16;

/**
 * Maximum number of bytes handed to a single OPFS write call.
 *
 * Browsers have been observed to short-write (and, with positioned writes, corrupt)
 * large buffers, so every OPFS write is split into bounded pieces and the persisted
 * size is verified afterwards. A silently truncated chunk would otherwise only be
 * noticed on the next read.
 */
const OPFS_WRITE_PIECE_BYTES = 64 * 1024;

export interface OpfsChunkRecord {
  readonly chunkIndex: number;
  readonly generation: number;
  readonly crc32: number;
  readonly data: Uint8Array;
}

function formatChunkFilename(chunkIndex: number, generation: number): string {
  return `chunk-${String(chunkIndex)}-g${String(generation)}.bin`;
}

function parseChunkFilename(name: string): { chunkIndex: number; generation: number } | null {
  const match = /^chunk-(\d+)-g(\d+)\.bin$/.exec(name);
  if (!match) {
    return null;
  }
  const cIdx = Number(match[1]);
  const gen = Number(match[2]);
  if (!Number.isSafeInteger(cIdx) || !Number.isSafeInteger(gen)) {
    return null;
  }
  return { chunkIndex: cIdx, generation: gen };
}

export class OpfsChunkBackend {
  private rootDirPromise: Promise<FileSystemDirectoryHandle> | null = null;

  public static async isSupported(): Promise<boolean> {
    if (typeof navigator === 'undefined' || typeof navigator.storage.getDirectory !== 'function') {
      return false;
    }
    try {
      const root = await navigator.storage.getDirectory();
      const probeDir = await root.getDirectoryHandle(OPFS_ROOT_DIR, { create: true });
      const probeFile = await probeDir.getFileHandle('.opfs-probe', { create: true });
      if (typeof probeFile.createWritable !== 'function') {
        return false;
      }
      const w = await probeFile.createWritable();
      await w.write(new Uint8Array([0x57, 0x4e]));
      await w.close();
      await probeDir.removeEntry('.opfs-probe');
      return true;
    } catch {
      return false;
    }
  }

  private async getProfileDir(profileId: string): Promise<FileSystemDirectoryHandle> {
    validateStorageIdentifier(profileId);
    if (this.rootDirPromise === null) {
      this.rootDirPromise = (async () => {
        const navRoot = await navigator.storage.getDirectory();
        const appRoot = await navRoot.getDirectoryHandle(OPFS_ROOT_DIR, { create: true });
        return appRoot.getDirectoryHandle('profiles', { create: true });
      })();
    }
    const profilesDir = await this.rootDirPromise;
    return profilesDir.getDirectoryHandle(profileId, { create: true });
  }

  public async writeChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    data: Uint8Array,
    precomputedCrc32?: number,
  ): Promise<number> {
    if (data.byteLength !== DISK_CHUNK_BYTES) {
      throw new StorageValidationError('corrupt_bundle', 'OPFS chunk must be 64 KiB');
    }
    const dir = await this.getProfileDir(profileId);
    const filename = formatChunkFilename(chunkIndex, generation);
    const fileHandle = await dir.getFileHandle(filename, { create: true });

    const checksum = precomputedCrc32 ?? crc32(data);
    const envelope = new Uint8Array(CHUNK_HEADER_BYTES + DISK_CHUNK_BYTES);
    const dv = new DataView(envelope.buffer);
    dv.setUint32(0, CHUNK_FILE_MAGIC, true);
    dv.setUint32(4, CHUNK_FILE_VERSION, true);
    dv.setUint32(8, chunkIndex >>> 0, true);
    dv.setUint32(12, checksum >>> 0, true);
    envelope.set(data, CHUNK_HEADER_BYTES);

    const writable = await fileHandle.createWritable();
    try {
      for (let offset = 0; offset < envelope.byteLength; offset += OPFS_WRITE_PIECE_BYTES) {
        const end = Math.min(offset + OPFS_WRITE_PIECE_BYTES, envelope.byteLength);
        await writable.write(envelope.subarray(offset, end));
      }
      await writable.close();
    } catch (error) {
      await writable.abort().catch(() => undefined);
      throw error;
    }

    const persisted = await fileHandle.getFile();
    if (persisted.size !== envelope.byteLength) {
      throw new StorageValidationError(
        'io_interrupted',
        `OPFS chunk ${filename} short-wrote: ${String(persisted.size)} of ${String(envelope.byteLength)} bytes`,
      );
    }
    return checksum;
  }

  public async readChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
    expectedCrc32?: number,
  ): Promise<OpfsChunkRecord> {
    const dir = await this.getProfileDir(profileId);
    const filename = formatChunkFilename(chunkIndex, generation);
    const fileHandle = await dir.getFileHandle(filename, { create: false });
    const file = await fileHandle.getFile();
    const raw = new Uint8Array(await file.arrayBuffer());
    if (raw.byteLength !== CHUNK_HEADER_BYTES + DISK_CHUNK_BYTES) {
      throw new StorageValidationError('corrupt_bundle', `Truncated OPFS chunk file ${filename}`);
    }
    const dv = new DataView(raw.buffer, raw.byteOffset, raw.byteLength);
    if (
      dv.getUint32(0, true) !== CHUNK_FILE_MAGIC ||
      dv.getUint32(4, true) !== CHUNK_FILE_VERSION
    ) {
      throw new StorageValidationError(
        'corrupt_bundle',
        `Invalid OPFS header magic in ${filename}`,
      );
    }
    const storedIdx = dv.getUint32(8, true);
    const storedCrc = dv.getUint32(12, true);
    const data = raw.slice(CHUNK_HEADER_BYTES);
    const actualCrc = crc32(data);
    if (
      storedIdx !== chunkIndex >>> 0 ||
      actualCrc !== storedCrc ||
      (expectedCrc32 !== undefined && actualCrc !== expectedCrc32 >>> 0)
    ) {
      throw new StorageValidationError(
        'checksum_mismatch',
        `OPFS chunk CRC32 mismatch in ${filename}`,
      );
    }
    return { chunkIndex, generation, crc32: actualCrc, data };
  }

  public async deleteChunk(
    profileId: string,
    chunkIndex: number,
    generation: number,
  ): Promise<void> {
    const dir = await this.getProfileDir(profileId);
    const filename = formatChunkFilename(chunkIndex, generation);
    try {
      await dir.removeEntry(filename);
    } catch {
      // Already absent
    }
  }

  public async listChunkFiles(
    profileId: string,
  ): Promise<ReadonlyArray<{ chunkIndex: number; generation: number }>> {
    const dir = await this.getProfileDir(profileId);
    const out: Array<{ chunkIndex: number; generation: number }> = [];
    const iterableDir = dir as unknown as AsyncIterable<[string, FileSystemHandle]>;
    for await (const [name, handle] of iterableDir) {
      if (handle.kind === 'file') {
        const parsed = parseChunkFilename(name);
        if (parsed !== null) {
          out.push(parsed);
        }
      }
    }
    return out;
  }

  public async clearProfile(profileId: string): Promise<void> {
    validateStorageIdentifier(profileId);
    const dir = await this.getProfileDir(profileId);
    const names: string[] = [];
    const iterableDir = dir as unknown as AsyncIterable<[string, FileSystemHandle]>;
    for await (const [name] of iterableDir) {
      names.push(name);
    }
    for (const name of names) {
      try {
        await dir.removeEntry(name, { recursive: true });
      } catch {
        // Ignore concurrent delete
      }
    }
  }
}
