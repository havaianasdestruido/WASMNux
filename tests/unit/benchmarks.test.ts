import { cpus, release, type as osType } from 'node:os';
import { readFileSync, statSync, writeFileSync } from 'node:fs';
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

function medianOf(values: ReadonlyArray<number>): number {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  if (sorted.length % 2 === 1) {
    return sorted[mid] ?? 0;
  }
  return ((sorted[mid - 1] ?? 0) + (sorted[mid] ?? 0)) / 2;
}

function round(val: number, digits = 2): number {
  const f = 10 ** digits;
  return Math.round(val * f) / f;
}

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
  it('measures multi-trial cold boot time, MIPS, terminal RTT, ext4 block I/O throughput, snapshot compression/restore, and WASM memory footprint', async () => {
    const vanillaChunks = extractNonZeroBaseChunks(vanillaDisk);
    const debChunks = extractNonZeroBaseChunks(debianDisk);

    // 1. Multi-trial Vanilla Linux 7.3.1 Cold Boot Benchmark (3 independent WASM instances)
    const vanBootSamplesMs: number[] = [];
    const vanMipsSamples: number[] = [];
    let lastVanVm: WasmVmSession | null = null;
    let vanSteps = 0;
    let vanWasmMemBytes = 0;

    for (let trial = 0; trial < 3; trial += 1) {
      const vanRt = await WasmVmRuntime.fromBytes(wasmRaw);
      const t0 = performance.now();
      const vm = vanRt.createVm({ profileId: 'vanilla', ramMib: 128, diskMib: 64 });
      for (const c of vanillaChunks) {
        vm.loadBaseDiskChunk(c.chunkIndex, c.data);
      }
      vm.stageKernel(kernelImg);
      vm.stageInitrd(vanillaInitrd);
      vm.boot('console=ttyS0,115200 earlycon=sbi panic=-1');
      const boot = pumpUntil(vm, 'wasmnux-vanilla:~#');
      const elapsedMs = performance.now() - t0;
      expect(boot.ok).toBe(true);

      const tel = vm.getTelemetry();
      const steps = Number(tel.totalSteps);
      vanBootSamplesMs.push(round(elapsedMs, 1));
      vanMipsSamples.push(round(steps / 1e6 / (elapsedMs / 1000), 1));
      vanSteps = steps;
      vanWasmMemBytes = tel.wasmLinearMemoryBytes;

      if (trial < 2) {
        vm.destroy();
      } else {
        lastVanVm = vm;
      }
    }

    if (!lastVanVm) {
      throw new Error('Expected active Vanilla Linux VM session');
    }
    const vanVm = lastVanVm;

    // 2. Terminal Round-Trip Latency (5 iterations)
    const rttRawMs: number[] = [];
    for (let i = 0; i < 5; i += 1) {
      const token = `BENCH_RTT_${String(i)}_${String(Date.now())}`;
      const t0 = performance.now();
      vanVm.pushSerialInput(`echo ${token}\n`);
      const res = pumpUntil(vanVm, `${token}\r\n`, 50_000, 500);
      expect(res.ok).toBe(true);
      rttRawMs.push(round(performance.now() - t0, 2));
    }
    const rttSorted = [...rttRawMs].sort((a, b) => a - b);

    // 3. Guest ext4 Virtual Disk Sequential Write & Uncached Read Throughput (4 MiB non-zero payload + conv=fsync + drop_caches)
    const writeBytes = 4 * 1024 * 1024;
    const tWrite0 = performance.now();
    vanVm.pushSerialInput(
      'dd if=/dev/urandom of=/root/bench4m.bin bs=65536 count=64 conv=fsync && sync && echo WRITE_DONE_4M\n',
    );
    const wrRes = pumpUntil(vanVm, 'WRITE_DONE_4M');
    const writeMs = performance.now() - tWrite0;
    expect(wrRes.ok).toBe(true);
    const writeMibPerSec = writeBytes / (1024 * 1024) / (writeMs / 1000);

    // Flush and drop page cache before timing purely uncached VirtIO-Blk read
    vanVm.pushSerialInput('sync && echo 3 > /proc/sys/vm/drop_caches && echo CACHE_DROPPED\n');
    expect(pumpUntil(vanVm, 'CACHE_DROPPED').ok).toBe(true);

    const tRead0 = performance.now();
    vanVm.pushSerialInput('dd if=/root/bench4m.bin of=/dev/null bs=65536 && echo READ_DONE_4M\n');
    const rdRes = pumpUntil(vanVm, 'READ_DONE_4M');
    const readMs = performance.now() - tRead0;
    expect(rdRes.ok).toBe(true);
    const readMibPerSec = writeBytes / (1024 * 1024) / (readMs / 1000);

    // 4. Multi-trial Full Live VM Snapshot Save + Gzip + Restore Benchmark (3 iterations)
    const snapSaveSamplesMs: number[] = [];
    const snapGzipSamplesMs: number[] = [];
    const snapRestoreSamplesMs: number[] = [];
    let snapRawBytes = 0;
    let snapGzipBytes = 0;

    for (let s = 0; s < 3; s += 1) {
      const tSave0 = performance.now();
      const rawSnap = vanVm.saveFullSnapshot();
      snapSaveSamplesMs.push(round(performance.now() - tSave0, 1));
      snapRawBytes = rawSnap.byteLength;

      const tGzip0 = performance.now();
      const gzSnap = gzipSync(rawSnap, { level: 1 });
      snapGzipSamplesMs.push(round(performance.now() - tGzip0, 1));
      snapGzipBytes = gzSnap.byteLength;

      const tRestore0 = performance.now();
      vanVm.restoreFullSnapshot(rawSnap);
      snapRestoreSamplesMs.push(round(performance.now() - tRestore0, 1));
    }
    vanVm.destroy();

    // 5. Multi-trial Debian 13.7 ("trixie") Cold Boot Benchmark (3 independent WASM instances)
    const debBootSamplesMs: number[] = [];
    const debMipsSamples: number[] = [];
    let debSteps = 0;
    let debWasmMemBytes = 0;

    for (let trial = 0; trial < 3; trial += 1) {
      const debRt = await WasmVmRuntime.fromBytes(wasmRaw);
      const t0 = performance.now();
      const debVm = debRt.createVm({ profileId: 'debian', ramMib: 256, diskMib: 256 });
      for (const c of debChunks) {
        debVm.loadBaseDiskChunk(c.chunkIndex, c.data);
      }
      debVm.stageKernel(kernelImg);
      debVm.boot(
        'console=ttyS0,115200 earlycon=sbi root=/dev/vda rw rootwait init=/usr/sbin/init panic=-1',
      );
      const debBoot = pumpUntil(debVm, 'WASMNUX_DEBIAN_BOOT_COMPLETE', 250_000, 4000);
      const elapsedMs = performance.now() - t0;
      expect(debBoot.ok).toBe(true);

      const debTel = debVm.getTelemetry();
      const steps = Number(debTel.totalSteps);
      debBootSamplesMs.push(round(elapsedMs, 1));
      debMipsSamples.push(round(steps / 1e6 / (elapsedMs / 1000), 1));
      debSteps = steps;
      debWasmMemBytes = debTel.wasmLinearMemoryBytes;
      debVm.destroy();
    }

    const cpuInfo = cpus()[0];
    const report = {
      timestampIso: new Date().toISOString(),
      environment: {
        os: `${osType()} ${release()}`,
        cpuModel: cpuInfo?.model ?? 'unknown',
        logicalCores: cpus().length,
        nodeVersion: process.version,
        v8Version: process.versions.v8,
      },
      artifacts: {
        wasmBytes: statSync(resolve(PUBLIC, 'wasm/wasmnux_wasm.wasm')).size,
        kernelGzipBytes: kernelGz.byteLength,
        kernelRawBytes: kernelImg.byteLength,
        vanillaInitrdGzipBytes: vanillaInitrdGz.byteLength,
        vanillaDiskGzipBytes: vanillaDiskGz.byteLength,
        debianDiskGzipBytes: debianDiskGz.byteLength,
      },
      vanillaLinux: {
        coldBootTrialsMs: vanBootSamplesMs,
        coldBootMedianMs: round(medianOf(vanBootSamplesMs), 1),
        bootInstructions: vanSteps,
        effectiveMipsTrials: vanMipsSamples,
        effectiveMipsMedian: round(medianOf(vanMipsSamples), 1),
        wasmLinearMemoryMib: round(vanWasmMemBytes / (1024 * 1024), 1),
        terminalRttTrialsMs: rttRawMs,
        terminalRttMinMs: rttSorted[0] ?? 0,
        terminalRttMedianMs: round(medianOf(rttSorted), 2),
        terminalRttMaxMs: rttSorted[rttSorted.length - 1] ?? 0,
        ext4UrandomWrite4MibMs: round(writeMs, 1),
        ext4UrandomWriteMibPerSec: round(writeMibPerSec, 1),
        ext4UncachedRead4MibMs: round(readMs, 1),
        ext4UncachedReadMibPerSec: round(readMibPerSec, 1),
        snapshotRawMib: round(snapRawBytes / (1024 * 1024), 2),
        snapshotGzipMib: round(snapGzipBytes / (1024 * 1024), 2),
        snapshotSaveTrialsMs: snapSaveSamplesMs,
        snapshotSaveMedianMs: round(medianOf(snapSaveSamplesMs), 1),
        snapshotGzipTrialsMs: snapGzipSamplesMs,
        snapshotGzipMedianMs: round(medianOf(snapGzipSamplesMs), 1),
        snapshotRestoreTrialsMs: snapRestoreSamplesMs,
        snapshotRestoreMedianMs: round(medianOf(snapRestoreSamplesMs), 1),
      },
      debianLinux: {
        coldBootTrialsMs: debBootSamplesMs,
        coldBootMedianMs: round(medianOf(debBootSamplesMs), 1),
        bootInstructions: debSteps,
        effectiveMipsTrials: debMipsSamples,
        effectiveMipsMedian: round(medianOf(debMipsSamples), 1),
        wasmLinearMemoryMib: round(debWasmMemBytes / (1024 * 1024), 1),
      },
    };

    const jsonOutput = JSON.stringify(report, null, 2);
    console.info('BENCHMARK_REPORT_JSON=' + jsonOutput);
    const outPath = process.env['WASMNUX_BENCH_OUT'];
    if (outPath) {
      writeFileSync(outPath, jsonOutput + '\n', 'utf8');
    }

    expect(report.vanillaLinux.coldBootMedianMs).toBeLessThan(15_000);
    expect(report.debianLinux.coldBootMedianMs).toBeLessThan(45_000);
    expect(report.vanillaLinux.effectiveMipsMedian).toBeGreaterThan(20);
    expect(report.vanillaLinux.ext4UrandomWriteMibPerSec).toBeGreaterThan(1);
    expect(report.vanillaLinux.ext4UncachedReadMibPerSec).toBeGreaterThan(2);
  }, 180_000);
});
