import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { extractNonZeroBaseChunks } from '../../src/runtime/profiles';
import { crc32 } from '../../src/storage/crc32';
import { MemoryStorageAdapter, PersistentStorageEngine } from '../../src/storage/storage-engine';
import { DISK_CHUNK_BYTES, VmRuntimeError, WasmVmRuntime } from '../../src/wasm/bindings';
import { VmEngineHost } from '../../src/worker/vm-engine-host';
import type { WorkerMessage } from '../../src/worker/protocol';

const PUBLIC_ROOT = resolve(process.cwd(), 'public');
const WASM_BYTES = readFileSync(resolve(PUBLIC_ROOT, 'wasm/wasmnux_wasm.wasm'));
const KERNEL_IMAGE = gunzipSync(
  readFileSync(resolve(PUBLIC_ROOT, 'images/vmlinux-7.3.1-riscv64.Image.gz')),
);
const VANILLA_INITRD = gunzipSync(
  readFileSync(resolve(PUBLIC_ROOT, 'images/vanilla-initramfs.cpio.gz')),
);
const VANILLA_DISK = gunzipSync(readFileSync(resolve(PUBLIC_ROOT, 'images/vanilla-disk.ext4.gz')));
const VANILLA_CHUNKS = extractNonZeroBaseChunks(VANILLA_DISK);

function collectTsFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      out.push(...collectTsFiles(full));
    } else if (full.endsWith('.ts')) {
      out.push(full);
    }
  }
  return out;
}

function pumpVanilla(
  vm: ReturnType<WasmVmRuntime['createVm']>,
  marker: string,
  maxSlices = 700,
): { matched: boolean; output: string } {
  const dec = new TextDecoder();
  let output = '';
  for (let i = 0; i < maxSlices; i += 1) {
    const step = vm.step(250_000);
    const s = vm.drainSerialOutput();
    if (s.byteLength > 0) {
      output += dec.decode(s, { stream: true });
    }
    if (output.includes(marker)) {
      return { matched: true, output };
    }
    if (step.outcome === 'poweroff' || step.outcome === 'reboot') {
      break;
    }
  }
  return { matched: false, output };
}

describe('Security Boundaries, Fault Injection, Crash Recovery & Isolation (Phase 10)', () => {
  it('audits TypeScript source tree for zero eval(), new Function(), or @ts-ignore / any escapes', () => {
    const srcFiles = collectTsFiles(resolve(process.cwd(), 'src'));
    expect(srcFiles.length).toBeGreaterThanOrEqual(15);

    for (const file of srcFiles) {
      const code = readFileSync(file, 'utf8');
      expect(code).not.toMatch(/\beval\s*\(/);
      expect(code).not.toMatch(/\bnew\s+Function\s*\(/);
      expect(code).not.toContain('@ts-ignore');
      expect(code).not.toContain('@ts-expect-error');
    }
  });

  it('rejects out-of-bounds RAM/disk configs, corrupted kernels, and tampered snapshots at the WASM boundary', async () => {
    const rt = await WasmVmRuntime.fromBytes(WASM_BYTES);

    expect(() => rt.createVm({ profileId: 'vanilla', ramMib: 4, diskMib: 64 })).toThrow(
      VmRuntimeError,
    );
    expect(() => rt.createVm({ profileId: 'vanilla', ramMib: 8192, diskMib: 64 })).toThrow(
      VmRuntimeError,
    );
    expect(() => rt.createVm({ profileId: '../traversal', ramMib: 64, diskMib: 64 })).toThrow(
      VmRuntimeError,
    );

    const vm = rt.createVm({ profileId: 'vanilla', ramMib: 64, diskMib: 16 });
    // Booting before staging a kernel, staging an empty kernel, or out-of-bounds chunk index must fail cleanly
    expect(() => {
      vm.boot('console=ttyS0');
    }).toThrow(VmRuntimeError);
    vm.stageKernel(new Uint8Array(0));
    expect(() => {
      vm.boot('console=ttyS0');
    }).toThrow(VmRuntimeError);
    expect(() => {
      vm.loadBaseDiskChunk(999_999, new Uint8Array(DISK_CHUNK_BYTES));
    }).toThrow(VmRuntimeError);

    // Stepping an invalid RV64 opcode stream (0xffffffff) must trap cleanly inside CPU without WASM panic
    vm.stageKernel(new Uint8Array([0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]));
    vm.boot('console=ttyS0');
    expect(() => vm.step(64)).not.toThrow();

    // Tampered snapshot blob must fail cleanly with snapshot_error
    expect(() => {
      vm.restoreFullSnapshot(new TextEncoder().encode('WNUXFULL_CORRUPTED_PAYLOAD'));
    }).toThrow(VmRuntimeError);

    vm.destroy();
  });

  it('rejects malformed Worker postMessage envelopes without crashing VmEngineHost', async () => {
    const emitted: WorkerMessage[] = [];
    const host = new VmEngineHost((msg) => emitted.push(msg));

    await host.handleRawMessage(null);
    await host.handleRawMessage('not-an-object');
    await host.handleRawMessage({ type: 'unknown_cmd', reqId: 1 });
    await host.handleRawMessage({ type: 'init_vm', reqId: 'bad-id' });

    expect(emitted).toHaveLength(4);
    for (const msg of emitted) {
      expect(msg.kind).toBe('vm_error');
    }
  });

  it('rejects path-traversal, wrong-profile, and CRC-tampered disk import bundles', async () => {
    const storage = new PersistentStorageEngine(new MemoryStorageAdapter());
    const chunk = new Uint8Array(DISK_CHUNK_BYTES);
    chunk[0] = 0x42;

    await storage.commitDirtyChunks('vanilla', 64 * 1024 * 1024, [
      { chunkIndex: 0, crc32: crc32(chunk), data: chunk },
    ]);
    const validBundle = await storage.exportProfileDiskBundle('vanilla', 64 * 1024 * 1024);

    // Cross-profile import ('vanilla' bundle imported into 'debian') must be rejected
    await expect(storage.importProfileDiskBundle('debian', validBundle)).rejects.toThrow();

    // Path traversal profile ID must be rejected
    await expect(
      storage.importProfileDiskBundle('../../etc/passwd', validBundle),
    ).rejects.toThrow();

    // Bit-flipped CRC tamper in chunk data must be rejected
    const tampered = validBundle.slice();
    tampered[tampered.length - 1] = (tampered[tampered.length - 1] ?? 0) ^ 0xff;
    await expect(storage.importProfileDiskBundle('vanilla', tampered)).rejects.toThrow();
  });

  it('recovers cleanly from simulated mid-write crash + storage quota exhaustion while preserving committed ext4 state', async () => {
    const adapter = new MemoryStorageAdapter();
    const storage = new PersistentStorageEngine(adapter);
    const capBytes = 64 * 1024 * 1024;
    const rt = await WasmVmRuntime.fromBytes(WASM_BYTES);

    // Boot 1: write committed file and commit cleanly to generation 1
    const vm1 = rt.createVm({ profileId: 'vanilla', ramMib: 128, diskMib: 64 });
    for (const c of VANILLA_CHUNKS) {
      vm1.loadBaseDiskChunk(c.chunkIndex, c.data);
    }
    vm1.stageKernel(KERNEL_IMAGE);
    vm1.stageInitrd(VANILLA_INITRD);
    vm1.boot('console=ttyS0,115200 earlycon=sbi panic=-1');
    expect(pumpVanilla(vm1, 'wasmnux-vanilla:~#').matched).toBe(true);

    vm1.pushSerialInput('echo "GEN1_SURVIVED_CRASH" > /root/committed.txt && sync && poweroff\n');
    for (let i = 0; i < 400; i += 1) {
      if (vm1.step(250_000).outcome === 'poweroff') {
        break;
      }
    }
    await storage.commitDirtyChunks('vanilla', capBytes, vm1.drainDirtyDiskChunks());
    vm1.destroy();

    // Inject orphan uncommitted generation-99 chunk (simulating power cut mid-OPFS write!)
    await adapter.writeChunk('vanilla', 5, 99, new Uint8Array(DISK_CHUNK_BYTES).fill(0xaa));

    // Also simulate storage quota exhaustion on an attempted write: verify error surfaces AND gen 1 survives!
    const origWrite = adapter.writeChunk.bind(adapter);
    let failWrites = true;
    adapter.writeChunk = (pid, idx, gen, data, crc) => {
      if (failWrites) {
        return Promise.reject(new Error('Simulated QuotaExceededError: disk full'));
      }
      return origWrite(pid, idx, gen, data, crc);
    };
    await expect(
      storage.commitDirtyChunks('vanilla', capBytes, [
        { chunkIndex: 1, crc32: 0, data: new Uint8Array(DISK_CHUNK_BYTES) },
      ]),
    ).rejects.toThrow('QuotaExceededError');
    failWrites = false;

    // Recover profile -> must sweep orphan gen-99 chunk AND mount ext4 cleanly with GEN1_SURVIVED_CRASH intact!
    const recovered = await storage.openAndRecoverProfile('vanilla', capBytes);
    expect(recovered.recoveredOrphanChunks).toBe(1);

    const vm2 = rt.createVm({ profileId: 'vanilla', ramMib: 128, diskMib: 64 });
    for (const c of VANILLA_CHUNKS) {
      vm2.loadBaseDiskChunk(c.chunkIndex, c.data);
    }
    for (const c of recovered.overlayChunks) {
      vm2.hydrateOverlayDiskChunk(c.chunkIndex, c.data, c.crc32);
    }
    vm2.stageKernel(KERNEL_IMAGE);
    vm2.stageInitrd(VANILLA_INITRD);
    vm2.boot('console=ttyS0,115200 earlycon=sbi panic=-1');
    expect(pumpVanilla(vm2, 'wasmnux-vanilla:~#').matched).toBe(true);

    vm2.pushSerialInput('cat /root/committed.txt && printf "CRASH_REC_%s\\n" "OK"\n');
    const recCheck = pumpVanilla(vm2, 'CRASH_REC_OK');
    expect(recCheck.matched).toBe(true);
    expect(recCheck.output).toContain('GEN1_SURVIVED_CRASH');
    vm2.destroy();
  });

  it('maintains bounded WASM linear memory across 5 repeated boot-destroy cycles and enforces strict profile isolation', async () => {
    const storage = new PersistentStorageEngine(new MemoryStorageAdapter());
    const rt = await WasmVmRuntime.fromBytes(WASM_BYTES);
    const heapSamples: number[] = [];

    for (let cycle = 0; cycle < 5; cycle += 1) {
      const vm = rt.createVm({ profileId: 'vanilla', ramMib: 128, diskMib: 64 });
      for (const c of VANILLA_CHUNKS) {
        vm.loadBaseDiskChunk(c.chunkIndex, c.data);
      }
      vm.stageKernel(KERNEL_IMAGE);
      vm.stageInitrd(VANILLA_INITRD);
      vm.boot('console=ttyS0,115200 earlycon=sbi panic=-1');
      expect(pumpVanilla(vm, 'wasmnux-vanilla:~#').matched).toBe(true);

      if (cycle === 0) {
        vm.pushSerialInput(
          'echo "SECRET_VANILLA_ONLY_777" > /root/secret.txt && sync && poweroff\n',
        );
        for (let i = 0; i < 400; i += 1) {
          if (vm.step(250_000).outcome === 'poweroff') {
            break;
          }
        }
        await storage.commitDirtyChunks('vanilla', 64 * 1024 * 1024, vm.drainDirtyDiskChunks());
      }

      heapSamples.push(vm.getTelemetry().wasmLinearMemoryBytes);
      vm.destroy();
    }

    // WASM linear memory after 5 full Linux 7.3.1 boots must not grow unboundedly (<= 16 MiB variance)
    const firstHeap = heapSamples[0] ?? 0;
    const lastHeap = heapSamples[heapSamples.length - 1] ?? 0;
    expect(lastHeap - firstHeap).toBeLessThanOrEqual(16 * 1024 * 1024);

    // Resetting 'debian' must NEVER affect 'vanilla' storage
    await storage.commitDirtyChunks('debian', 256 * 1024 * 1024, [
      { chunkIndex: 0, crc32: 0, data: new Uint8Array(DISK_CHUNK_BYTES).fill(0x11) },
    ]);
    await storage.resetProfileDisk('debian', 256 * 1024 * 1024);

    const debAfterReset = await storage.openAndRecoverProfile('debian', 256 * 1024 * 1024);
    const vanAfterDebReset = await storage.openAndRecoverProfile('vanilla', 64 * 1024 * 1024);
    expect(debAfterReset.overlayChunks).toHaveLength(0);
    expect(vanAfterDebReset.overlayChunks.length).toBeGreaterThan(0);
  }, 90_000);
});
