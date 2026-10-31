import { readFileSync, statSync } from 'node:fs';
import { resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
import { gunzipSync, gzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { extractNonZeroBaseChunks } from '../../src/runtime/profiles';
import { WasmVmRuntime, type WasmVmSession } from '../../src/wasm/bindings';

const ROOT = process.cwd();
const PUBLIC = resolve(ROOT, 'public');

const wasmRaw = readFileSync(resolve(PUBLIC, 'wasm/wasmnux_wasm.wasm'));
const kernelGz = readFileSync(resolve(PUBLIC, 'images/vmlinux-7.3.1-riscv64.Image.gz'));
const vanillaInitrdGz = readFileSync(resolve(PUBLIC, 'images/vanilla-initramfs.cpio.gz'));
const vanillaDiskGz = readFileSync(resolve(PUBLIC, 'images/vanilla-disk.ext4.gz'));
const debianDiskGz = readFileSync(resolve(PUBLIC, 'images/debian-rootfs.ext4.gz'));

const kernelImg = gunzipSync(kernelGz);
const vanillaInitrd = gunzipSync(vanillaInitrdGz);
const vanillaDisk = gunzipSync(vanillaDiskGz);
const debianDisk = gunzipSync(debianDiskGz);

function pumpUntil(
  vm: WasmVmSession,
  marker: string,
  budget = 250_000,
  maxSlices = 3500,
): { ok: boolean; out: string } {
  const dec = new TextDecoder();
  let out = '';
  for (let i = 0; i < maxSlices; i += 1) {
    const step = vm.step(budget);
    const s = vm.drainSerialOutput();
    if (s.byteLength > 0) {
      out += dec.decode(s, { stream: true });
      if (out.includes(marker)) {
        return { ok: true, out };
      }
    }
    if (step.outcome === 'poweroff' || step.outcome === 'reboot') {
      break;
    }
  }
  return { ok: false, out };
}

describe('Reproducible Performance Benchmarks (Vanilla Linux 7.3.1 & Debian 13.7)', () => {
  it('measures cold boot time, MIPS, terminal RTT, ext4 block I/O throughput, snapshot compression/restore, and WASM memory footprint', async () => {
    // 1. Vanilla Linux 7.3.1 Cold Boot Benchmark
    const vanRt = await WasmVmRuntime.fromBytes(wasmRaw);
    const vanillaChunks = extractNonZeroBaseChunks(vanillaDisk);
    const tVanStart = performance.now();
    const vanVm = vanRt.createVm({ profileId: 'vanilla', ramMib: 128, diskMib: 64 });
    for (const c of vanillaChunks) {
      vanVm.loadBaseDiskChunk(c.chunkIndex, c.data);
    }
    vanVm.stageKernel(kernelImg);
    vanVm.stageInitrd(vanillaInitrd);
    vanVm.boot('console=ttyS0,115200 earlycon=sbi panic=-1');
    const vanBoot = pumpUntil(vanVm, 'wasmnux-vanilla:~#');
    const vanColdBootMs = performance.now() - tVanStart;
    expect(vanBoot.ok).toBe(true);
    const vanTel = vanVm.getTelemetry();
    const vanSteps = Number(vanTel.totalSteps);
    const vanMips = vanSteps / 1e6 / (vanColdBootMs / 1000);

    // 2. Terminal Round-Trip Latency (5 iterations)
    const rttSamplesMs: number[] = [];
    for (let i = 0; i < 5; i += 1) {
      const token = `BENCH_RTT_${String(i)}_${String(Date.now())}`;
      const t0 = performance.now();
      vanVm.pushSerialInput(`echo ${token}\n`);
      const res = pumpUntil(vanVm, `${token}\r\n`, 50_000, 500);
      expect(res.ok).toBe(true);
      rttSamplesMs.push(performance.now() - t0);
    }
    rttSamplesMs.sort((a, b) => a - b);

    // 3. Guest ext4 Virtual Disk Sequential Write & Read Throughput (4 MiB)
    const writeBytes = 4 * 1024 * 1024;
    const tWrite0 = performance.now();
    vanVm.pushSerialInput(
      'dd if=/dev/zero of=/root/bench4m.bin bs=65536 count=64 && sync && echo WRITE_DONE_4M\n',
    );
    const wrRes = pumpUntil(vanVm, 'WRITE_DONE_4M');
    const writeMs = performance.now() - tWrite0;
    expect(wrRes.ok).toBe(true);
    const writeMibPerSec = writeBytes / (1024 * 1024) / (writeMs / 1000);

    const tRead0 = performance.now();
    vanVm.pushSerialInput(
      'echo 3 > /proc/sys/vm/drop_caches && dd if=/root/bench4m.bin of=/dev/null bs=65536 && echo READ_DONE_4M\n',
    );
    const rdRes = pumpUntil(vanVm, 'READ_DONE_4M');
    const readMs = performance.now() - tRead0;
    expect(rdRes.ok).toBe(true);
    const readMibPerSec = writeBytes / (1024 * 1024) / (readMs / 1000);

    // 4. Full Live VM Snapshot Save + Gzip + Restore Benchmark
    const tSnapSave0 = performance.now();
    const rawSnap = vanVm.saveFullSnapshot();
    const snapSaveMs = performance.now() - tSnapSave0;

    const tGzip0 = performance.now();
    const gzSnap = gzipSync(rawSnap, { level: 1 });
    const snapGzipMs = performance.now() - tGzip0;

    const tRestore0 = performance.now();
    vanVm.restoreFullSnapshot(rawSnap);
    const snapRestoreMs = performance.now() - tRestore0;
    vanVm.destroy();

    // 5. Debian 13.7 ("trixie") Cold Boot Benchmark
    const debRt = await WasmVmRuntime.fromBytes(wasmRaw);
    const debChunks = extractNonZeroBaseChunks(debianDisk);
    const tDebStart = performance.now();
    const debVm = debRt.createVm({ profileId: 'debian', ramMib: 256, diskMib: 256 });
    for (const c of debChunks) {
      debVm.loadBaseDiskChunk(c.chunkIndex, c.data);
    }
    debVm.stageKernel(kernelImg);
    debVm.boot(
      'console=ttyS0,115200 earlycon=sbi root=/dev/vda rw rootwait init=/usr/sbin/init panic=-1',
    );
    const debBoot = pumpUntil(debVm, 'WASMNUX_DEBIAN_BOOT_COMPLETE', 250_000, 4000);
    const debColdBootMs = performance.now() - tDebStart;
    expect(debBoot.ok).toBe(true);
    const debTel = debVm.getTelemetry();
    const debSteps = Number(debTel.totalSteps);
    const debMips = debSteps / 1e6 / (debColdBootMs / 1000);
    debVm.destroy();

    const report = {
      timestampIso: new Date().toISOString(),
      nodeVersion: process.version,
      artifacts: {
        wasmBytes: statSync(resolve(PUBLIC, 'wasm/wasmnux_wasm.wasm')).size,
        kernelGzipBytes: kernelGz.byteLength,
        kernelRawBytes: kernelImg.byteLength,
        vanillaInitrdGzipBytes: vanillaInitrdGz.byteLength,
        vanillaDiskGzipBytes: vanillaDiskGz.byteLength,
        debianDiskGzipBytes: debianDiskGz.byteLength,
      },
      vanillaLinux: {
        coldBootMs: Number(vanColdBootMs.toFixed(1)),
        bootInstructions: vanSteps,
        effectiveMips: Number(vanMips.toFixed(1)),
        wasmLinearMemoryMib: Number((vanTel.wasmLinearMemoryBytes / (1024 * 1024)).toFixed(1)),
        terminalRttMinMs: Number((rttSamplesMs[0] ?? 0).toFixed(2)),
        terminalRttMedianMs: Number((rttSamplesMs[2] ?? 0).toFixed(2)),
        terminalRttMaxMs: Number((rttSamplesMs[4] ?? 0).toFixed(2)),
        ext4SeqWrite4MibMs: Number(writeMs.toFixed(1)),
        ext4SeqWriteMibPerSec: Number(writeMibPerSec.toFixed(1)),
        ext4SeqRead4MibMs: Number(readMs.toFixed(1)),
        ext4SeqReadMibPerSec: Number(readMibPerSec.toFixed(1)),
        snapshotRawMib: Number((rawSnap.byteLength / (1024 * 1024)).toFixed(2)),
        snapshotGzipMib: Number((gzSnap.byteLength / (1024 * 1024)).toFixed(2)),
        snapshotSaveMs: Number(snapSaveMs.toFixed(1)),
        snapshotGzipMs: Number(snapGzipMs.toFixed(1)),
        snapshotRestoreMs: Number(snapRestoreMs.toFixed(1)),
      },
      debianLinux: {
        coldBootMs: Number(debColdBootMs.toFixed(1)),
        bootInstructions: debSteps,
        effectiveMips: Number(debMips.toFixed(1)),
        wasmLinearMemoryMib: Number((debTel.wasmLinearMemoryBytes / (1024 * 1024)).toFixed(1)),
      },
    };

    console.info('BENCHMARK_REPORT_JSON=' + JSON.stringify(report, null, 2));

    expect(vanColdBootMs).toBeLessThan(15_000);
    expect(debColdBootMs).toBeLessThan(45_000);
    expect(vanMips).toBeGreaterThan(20);
    expect(writeMibPerSec).toBeGreaterThan(1);
    expect(readMibPerSec).toBeGreaterThan(5);
  }, 120_000);
});
