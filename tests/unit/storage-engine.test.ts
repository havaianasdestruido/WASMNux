import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  crc32,
  decodeAndValidateDiskOverlayBundle,
  encodeDiskOverlayBundle,
  StorageValidationError,
  validateStorageIdentifier,
} from '../../src/storage/crc32';
import { MemoryStorageAdapter, PersistentStorageEngine } from '../../src/storage/storage-engine';
import { DISK_CHUNK_BYTES, WasmVmRuntime } from '../../src/wasm/bindings';

const WASM_BYTES = readFileSync(resolve(process.cwd(), 'public/wasm/wasmnux_wasm.wasm'));

function makeChunk(
  index: number,
  fillByte: number,
): {
  chunkIndex: number;
  crc32: number;
  data: Uint8Array;
} {
  const data = new Uint8Array(DISK_CHUNK_BYTES);
  data.fill(fillByte);
  data[0] = index & 0xff;
  return { chunkIndex: index, crc32: crc32(data), data };
}

describe('Persistent Virtual Storage Engine (Phase 4)', () => {
  it('computes exact IEEE 802.3 CRC32 and interoperates bit-for-bit with Rust WASM bundles', async () => {
    const checkBytes = new TextEncoder().encode('123456789');
    expect(crc32(checkBytes)).toBe(0xcbf4_3926);

    expect(() => {
      validateStorageIdentifier('../traversal');
    }).toThrow(StorageValidationError);
    expect(() => {
      validateStorageIdentifier('UPPERCASE');
    }).toThrow(StorageValidationError);

    // Encode a bundle in TypeScript and import it directly into Rust WASM SparseBlockDisk!
    const c2 = makeChunk(2, 0x7a);
    const tsBundle = encodeDiskOverlayBundle('vanilla', 4 * 1024 * 1024, [c2]);

    const rt = await WasmVmRuntime.fromBytes(WASM_BYTES);
    const vm = rt.createVm({ profileId: 'vanilla', ramMib: 16, diskMib: 4 });
    vm.importDiskBundle(tsBundle);

    // Re-export from Rust WASM and decode in TypeScript -> must match byte-for-byte!
    const rustExported = vm.exportDiskBundle();
    const decoded = decodeAndValidateDiskOverlayBundle(rustExported, 'vanilla');
    expect(decoded.profileId).toBe('vanilla');
    expect(decoded.capacityBytes).toBe(4 * 1024 * 1024);
    expect(decoded.chunks).toHaveLength(1);
    expect(decoded.chunks[0]?.chunkIndex).toBe(2);
    expect(decoded.chunks[0]?.crc32).toBe(c2.crc32);
    vm.destroy();
  });

  it('isolates profiles, enforces quotas, and recovers cleanly from mid-write crashes', async () => {
    const adapter = new MemoryStorageAdapter();
    const engine = new PersistentStorageEngine(adapter);
    const cap = 8 * 1024 * 1024;

    const vanillaC0 = makeChunk(0, 0x11);
    const vanillaC1 = makeChunk(1, 0x22);
    const debianC0 = makeChunk(0, 0xdd);

    await engine.commitDirtyChunks('vanilla', cap, [vanillaC0, vanillaC1]);
    await engine.commitDirtyChunks('debian', cap, [debianC0]);

    // Simulate interrupted mid-batch crash during generation 2 write on 'vanilla'
    const badBatchC0 = makeChunk(0, 0x99);
    const badBatchC5 = makeChunk(5, 0x88);
    let writesInAttempt = 0;
    adapter.onBeforeWriteChunk = (pid) => {
      if (pid === 'vanilla') {
        writesInAttempt += 1;
        if (writesInAttempt === 2) {
          throw new StorageValidationError('io_interrupted', 'Simulated power loss mid-commit');
        }
      }
    };

    await expect(
      engine.commitDirtyChunks('vanilla', cap, [badBatchC0, badBatchC5]),
    ).rejects.toThrow(StorageValidationError);
    adapter.onBeforeWriteChunk = undefined;

    // Also plant an orphan uncommitted g99 chunk simulating abrupt tab kill before cleanup
    await adapter.writeChunk('vanilla', 7, 99, makeChunk(7, 0xfe).data);

    // Recovery MUST discard g99 orphan and preserve generation 1 state untouched!
    const recovered = await engine.openAndRecoverProfile('vanilla', cap);
    expect(recovered.manifest.committedGeneration).toBe(1);
    expect(recovered.recoveredOrphanChunks).toBe(1);
    expect(recovered.overlayChunks).toHaveLength(2);
    expect(recovered.overlayChunks[0]?.crc32).toBe(vanillaC0.crc32);
    expect(recovered.overlayChunks[1]?.crc32).toBe(vanillaC1.crc32);

    // Quota limit check: cap at 2 overlay chunks -> writing chunk 9 must reject without corrupting!
    engine.setMaxOverlayChunksPerProfile(2);
    await expect(engine.commitDirtyChunks('vanilla', cap, [makeChunk(9, 0x44)])).rejects.toThrow(
      StorageValidationError,
    );
    engine.setMaxOverlayChunksPerProfile(null);

    // Resetting 'vanilla' leaves 'debian' 100% intact
    await engine.resetProfileDisk('vanilla', cap);
    const vanillaAfterReset = await engine.openAndRecoverProfile('vanilla', cap);
    expect(vanillaAfterReset.overlayChunks).toHaveLength(0);

    const debianCheck = await engine.openAndRecoverProfile('debian', cap);
    expect(debianCheck.overlayChunks).toHaveLength(1);
    expect(debianCheck.overlayChunks[0]?.crc32).toBe(debianC0.crc32);
  });

  it('supports named backups/restoration and rejects corrupted or mismatched bundles', async () => {
    const engine = new PersistentStorageEngine(new MemoryStorageAdapter());
    const cap = 4 * 1024 * 1024;
    const orig = makeChunk(3, 0x33);

    await engine.commitDirtyChunks('vanilla', cap, [orig]);
    await engine.createNamedBackup('vanilla', 'snap-clean', 'Clean baseline', cap);

    // Overwrite chunk 3 + add chunk 4
    await engine.commitDirtyChunks('vanilla', cap, [makeChunk(3, 0xee), makeChunk(4, 0xff)]);
    expect((await engine.openAndRecoverProfile('vanilla', cap)).overlayChunks).toHaveLength(2);

    // Restore 'snap-clean' -> reverts deterministically to single chunk 3 with original CRC32!
    await engine.restoreDiskBackup('vanilla', 'snap-clean');
    const restored = await engine.openAndRecoverProfile('vanilla', cap);
    expect(restored.overlayChunks).toHaveLength(1);
    expect(restored.overlayChunks[0]?.chunkIndex).toBe(3);
    expect(restored.overlayChunks[0]?.crc32).toBe(orig.crc32);

    // Export bundle, tamper 1 byte, and verify import rejects with checksum_mismatch
    const validBundle = await engine.exportProfileDiskBundle('vanilla', cap);
    const tampered = validBundle.slice();
    tampered[40] = (tampered[40] ?? 0) ^ 0xff;
    await expect(engine.importProfileDiskBundle('vanilla', tampered)).rejects.toThrow(
      StorageValidationError,
    );

    // Wrong target profile ('debian') rejects 'vanilla' bundle
    await expect(engine.importProfileDiskBundle('debian', validBundle)).rejects.toThrow(
      StorageValidationError,
    );
  });
});
