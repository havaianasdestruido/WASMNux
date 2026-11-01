/**
 * OPFS write-integrity regression tests.
 *
 * Browsers have been observed to short-write large OPFS buffers, which would silently
 * truncate a 64 KiB disk chunk. These tests drive `OpfsChunkBackend` against an
 * in-memory File System Access implementation to prove that
 *   1. every write call is bounded to at most 64 KiB, and
 *   2. a short write is reported instead of being accepted.
 */
import { afterEach, describe, expect, it } from 'vitest';

import { StorageValidationError } from '../../src/storage/crc32';
import { OpfsChunkBackend } from '../../src/storage/opfs-backend';
import { DISK_CHUNK_BYTES } from '../../src/wasm/bindings';

/** Records the size of every `write()` call so bounding can be asserted. */
class FakeWritable {
  public readonly writeSizes: number[] = [];
  private readonly parts: Uint8Array[] = [];
  public aborted = false;

  public constructor(
    private readonly file: FakeFile,
    private readonly truncateToBytes?: number,
  ) {}

  public write(data: Uint8Array): Promise<void> {
    this.writeSizes.push(data.byteLength);
    this.parts.push(Uint8Array.from(data));
    return Promise.resolve();
  }

  public close(): Promise<void> {
    const total = this.parts.reduce((sum, part) => sum + part.byteLength, 0);
    const merged = new Uint8Array(total);
    let offset = 0;
    for (const part of this.parts) {
      merged.set(part, offset);
      offset += part.byteLength;
    }
    this.file.bytes =
      this.truncateToBytes === undefined ? merged : merged.slice(0, this.truncateToBytes);
    return Promise.resolve();
  }

  public abort(): Promise<void> {
    this.aborted = true;
    return Promise.resolve();
  }
}

class FakeFile {
  public bytes = new Uint8Array(0);
  public lastWritable: FakeWritable | null = null;

  public constructor(private readonly truncateToBytes?: number) {}

  public createWritable(): Promise<FakeWritable> {
    this.lastWritable = new FakeWritable(this, this.truncateToBytes);
    return Promise.resolve(this.lastWritable);
  }

  public getFile(): Promise<{ size: number; arrayBuffer: () => Promise<ArrayBuffer> }> {
    return Promise.resolve({
      size: this.bytes.byteLength,
      arrayBuffer: () => Promise.resolve(this.bytes.slice().buffer),
    });
  }
}

class FakeDir {
  public readonly files = new Map<string, FakeFile>();

  public constructor(private readonly truncateToBytes?: number) {}

  // Always returns this directory: the backend only ever walks down to the profile dir.
  public getDirectoryHandle(): Promise<FakeDir> {
    return Promise.resolve(this);
  }

  public getFileHandle(name: string): Promise<FakeFile> {
    const existing = this.files.get(name);
    if (existing !== undefined) {
      return Promise.resolve(existing);
    }
    const created = new FakeFile(this.truncateToBytes);
    this.files.set(name, created);
    return Promise.resolve(created);
  }

  public removeEntry(): Promise<void> {
    return Promise.resolve();
  }
}

function installFakeOpfs(truncateToBytes?: number): { root: FakeDir; restore: () => void } {
  const root = new FakeDir(truncateToBytes);
  const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  Object.defineProperty(globalThis, 'navigator', {
    configurable: true,
    value: { storage: { getDirectory: () => Promise.resolve(root) } },
  });
  return {
    root,
    restore: () => {
      if (original === undefined) {
        Reflect.deleteProperty(globalThis, 'navigator');
      } else {
        Object.defineProperty(globalThis, 'navigator', original);
      }
    },
  };
}

const chunkOf = (seed: number): Uint8Array => {
  const data = new Uint8Array(DISK_CHUNK_BYTES);
  for (let i = 0; i < data.byteLength; i += 1) {
    data[i] = (i + seed) & 0xff;
  }
  return data;
};

let cleanup: (() => void) | null = null;
afterEach(() => {
  cleanup?.();
  cleanup = null;
});

describe('OPFS chunk write integrity', () => {
  it('bounds every OPFS write call to 64 KiB and persists the full envelope', async () => {
    const fake = installFakeOpfs();
    cleanup = fake.restore;

    const backend = new OpfsChunkBackend();
    const checksum = await backend.writeChunk('vanilla', 7, 1, chunkOf(3));

    const dir = await (
      await (await fake.root.getDirectoryHandle()).getDirectoryHandle()
    ).getDirectoryHandle();
    const file = dir.files.get('chunk-7-g1.bin');
    expect(file).toBeDefined();
    const writable = file?.lastWritable;
    expect(writable).not.toBeNull();
    const sizes = writable?.writeSizes ?? [];
    // 64 KiB payload + 16-byte header => two bounded pieces, never one oversized write.
    expect(sizes.length).toBe(2);
    for (const size of sizes) {
      expect(size).toBeLessThanOrEqual(64 * 1024);
    }
    expect(sizes.reduce((sum, size) => sum + size, 0)).toBe(DISK_CHUNK_BYTES + 16);
    // The header plus payload really landed, so the CRC is recoverable on read.
    expect(file?.bytes.byteLength).toBe(DISK_CHUNK_BYTES + 16);
    expect(checksum).toBeGreaterThan(0);

    const roundTripped = await backend.readChunk('vanilla', 7, 1);
    expect(roundTripped.data).toEqual(chunkOf(3));
    expect(roundTripped.crc32).toBe(checksum);
  });

  it('reports a short OPFS write instead of silently accepting a truncated chunk', async () => {
    const fake = installFakeOpfs(1024); // pretend only 1 KiB reaches storage
    cleanup = fake.restore;

    const backend = new OpfsChunkBackend();
    await expect(backend.writeChunk('debian', 1, 1, chunkOf(9))).rejects.toBeInstanceOf(
      StorageValidationError,
    );
    await expect(backend.writeChunk('debian', 1, 1, chunkOf(9))).rejects.toMatchObject({
      code: 'io_interrupted',
    });
  });
});
