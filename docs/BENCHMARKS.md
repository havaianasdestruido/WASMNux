# WASMNux Reproducible Performance Benchmarks

All benchmarks in this document are produced by executing the genuine `wasmnux_wasm.wasm` RV64IMAFDC + Sv39 virtual machine against upstream **Linux `7.3.1`** (`riscv64`), **BusyBox `1.37.0`**, and **Debian GNU/Linux `13.7` ("trixie") `riscv64`** — zero mocks, zero synthetic loops.

---

## 1. Reproducible Benchmark Procedure

Run the dedicated multi-trial benchmark harness (with no background dev/preview server competing for CPU):

```bash
WASMNUX_BENCH_OUT=/tmp/benchmark-report.json npm run bench
# Equivalent direct command:
npx vitest run tests/unit/benchmarks.test.ts
```

Run it alone. Every number below is a wall-clock measurement, so anything else competing for the
host CPU moves it: the same unmodified build measured **25.0 MIPS** with a free core and **18.5
MIPS** while sibling Vitest workers were booting the Debian guest, which is enough to cross the
20 MIPS floor on its own. For that reason `benchmarks.test.ts` is excluded from `npm test`
(which asserts correctness) and is executed as its own CI step, with the results uploaded as an
artifact. `npm run test:all` still runs everything together for local convenience.

What `tests/unit/benchmarks.test.ts` executes in a single deterministic run:

1. **3 independent cold boots** of **Vanilla Linux `7.3.1`** (`128 MiB` RAM, `64 MiB` `ext4`), instantiating a fresh `WasmVmRuntime` per trial and recording all 3 wall-clock times, retired RV64GC instructions, and effective MIPS.
2. **5 consecutive interactive UART round-trips** (`echo BENCH_RTT_<i>_<ts>` -> `NS16550A` -> BusyBox `ash` -> `drainSerialOutput`), recording all 5 raw latencies plus min, median, and max.
3. **4 MiB non-zero `ext4` block write & uncached read**:
   - Write: `dd if=/dev/urandom of=/root/bench4m.bin bs=65536 count=64 conv=fsync && sync` (forces real non-zero allocation across 16 x 256 KiB CoW chunks — avoids zero-page elision).
   - Cache flush: `sync && echo 3 > /proc/sys/vm/drop_caches`.
   - Uncached read: `dd if=/root/bench4m.bin of=/dev/null bs=65536`.
4. **3 consecutive full live VM snapshot cycles** (`saveFullSnapshot()` -> `gzipSync(level=1)` -> `restoreFullSnapshot()`), capturing both Trial 1 (first-touch `WebAssembly.Memory.grow` + host OS page faults) and steady-state Trials 2–3.
5. **3 independent cold boots** of **Debian `13.7` ("trixie") `riscv64`** (`256 MiB` RAM, `256 MiB` `ext4`) to `WASMNUX_DEBIAN_BOOT_COMPLETE`.

---

## 2. Canonical Reference Environment (`2026-11-20T16:57:15.666Z`)

| Property           | Value                                                                                                            |
| ------------------ | ---------------------------------------------------------------------------------------------------------------- |
| Host Kernel        | `Linux 6.1.158+` (`x86_64`)                                                                                      |
| Host CPU           | `Intel(R) Xeon(R) Processor @ 2.60GHz` (`2` logical vCPUs)                                                       |
| WebAssembly Engine | Node.js `v22.22.3` / V8 `12.4.254.21-node.56` (`vitest v5.0.5`)                                                  |
| Rust Toolchain     | `rustc 1.88.0` (`wasm32-unknown-unknown`, `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`)                   |
| Guest Architecture | `riscv64` (`RV64IMAFDC` + `Zicsr` + `Zifencei`, `Sv39` MMU, SBI v0.2)                                            |
| Guest Kernel       | Upstream Linux `7.3.1` built from source in-repo (`scripts/build-kernel.sh`) -> `vmlinux-7.3.1-riscv64.Image.gz` |

---

## 3. Production Artifact Sizes

| Artifact                  | Path                                           |      Compressed / Bundled Size |        Uncompressed Size | Notes                                                                                                                               |
| ------------------------- | ---------------------------------------------- | -----------------------------: | -----------------------: | ----------------------------------------------------------------------------------------------------------------------------------- |
| WASM VM Runtime           | `public/wasm/wasmnux_wasm.wasm`                |    **316.9 KiB** (`324,487` B) |                316.9 KiB | Zero external Rust crate dependencies; 44 ABI exports                                                                               |
| Linux `7.3.1` Kernel      | `public/images/vmlinux-7.3.1-riscv64.Image.gz` |   **3.33 MiB** (`3,495,318` B) | 6.24 MiB (`6,547,296` B) | Built from source in-repo; `KERNEL_UNCOMPRESSED` (direct `j` entry), VirtIO MMIO (`blk`, `net`, `rng`), `ext4`, `SimpleFB`, `ttyS0` |
| Vanilla Initramfs         | `public/images/vanilla-initramfs.cpio.gz`      |    **795.2 KiB** (`814,278` B) |                ~1.59 MiB | Static `riscv64-linux-musl` BusyBox `1.37.0` + `wasmnux-fbtest`                                                                     |
| Vanilla Base `ext4` Disk  | `public/images/vanilla-disk.ext4.gz`           |      **69.7 KiB** (`71,402` B) |        64.0 MiB (sparse) | 256 KiB CoW chunked `ext4` rootfs                                                                                                   |
| Debian `13.7` `ext4` Disk | `public/images/debian-rootfs.ext4.gz`          | **28.21 MiB** (`29,584,790` B) |       256.0 MiB (sparse) | Official Debian `13.7` ("trixie") `riscv64` (`glibc 2.41`, `apt 3.0.3`, `dpkg 1.22.22`)                                             |
| Browser UI Bundle         | `dist/assets/index-*.js`                       |           **115.3 KiB** (gzip) |                446.8 KiB | TypeScript orchestrator + WebGPU/Canvas2D + `@xterm/xterm`                                                                          |

---

## 4. Multi-Trial Cold Boot, Instruction Throughput, & Memory Footprint

| Profile                                      | Guest RAM / Disk  | Cold Boot Median (`N=3`) | Raw Cold Boot Trials (ms)  | Retired RV64GC Instructions |        Effective MIPS Median (`N=3`) | Total WASM Linear Memory |
| -------------------------------------------- | ----------------- | -----------------------: | -------------------------- | --------------------------: | -----------------------------------: | -----------------------: |
| **Vanilla Linux `7.3.1` (BusyBox `1.37.0`)** | 128 MiB / 64 MiB  | **3,842.2 ms** (~3.84 s) | `[3996.7, 3842.2, 3712.1]` |                `94,703,759` | **24.6 MIPS** (`[23.7, 24.6, 25.5]`) |            **145.1 MiB** |
| **Debian `13.7` ("trixie") `riscv64`**       | 256 MiB / 256 MiB | **4,424.0 ms** (~4.42 s) | `[4424.0, 4274.0, 4434.8]` |               `104,750,000` | **23.7 MIPS** (`[23.7, 24.5, 23.6]`) |            **344.7 MiB** |

---

## 5. Interactive Terminal, Virtual Block I/O, & Live Snapshots (Vanilla Linux `7.3.1`)

| Metric                                                    | Workload                                                                               | Median / Primary                                | Raw Trials / Details                                                 |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------- | ----------------------------------------------- | -------------------------------------------------------------------- |
| **Terminal Round-Trip Latency**                           | 5 serial `echo BENCH_RTT_N` round-trips (`NS16550A` + BusyBox `ash`)                   | **1.96 ms min / 4.20 ms median / 20.78 ms max** | `[20.78, 20.74, 2.15, 1.96, 4.20]` ms                                |
| **Guest `ext4` Non-Zero Sequential Write (`VirtIO-Blk`)** | `dd if=/dev/urandom of=/root/bench4m.bin bs=65536 count=64 conv=fsync && sync` (4 MiB) | **9.5 ms** (**421.1 MiB/s**)                    | Full random payload + `fsync` across 16 CoW chunks                   |
| **Guest `ext4` Uncached Sequential Read (`VirtIO-Blk`)**  | `drop_caches` + `dd if=/root/bench4m.bin of=/dev/null bs=65536` (4 MiB)                | **213.4 ms** (**18.7 MiB/s**)                   | Read after `sync && echo 3 > /proc/sys/vm/drop_caches`               |
| **Live Full VM Snapshot Serialization**                   | `WNUXFULL` (`132.41 MiB` raw: 128 MiB RAM + VRAM + CPU/CSR/VirtIO + CoW overlay)       | **172.7 ms** (median)                           | `[479.9, 172.7, 150.3]` ms (Trial 1 includes initial `memory.grow`)  |
| **Snapshot Gzip Compression**                             | `132.41 MiB` raw -> **`14.24 MiB`** compressed (**9.3x reduction**)                    | **475.1 ms** (median)                           | `[524.3, 475.1, 457.9]` ms (`zlib` level 1)                          |
| **Live Full VM Snapshot Restore**                         | Instant resume into running Linux `7.3.1` shell                                        | **66.7 ms** (median)                            | `[89.7, 66.4, 66.7]` ms (Trial 1 includes staging buffer allocation) |

---

## 6. Raw Benchmark JSON Output (`npm run bench`)

```json
{
  "timestampIso": "2026-11-20T16:57:15.666Z",
  "environment": {
    "os": "Linux 6.1.158+",
    "cpuModel": "Intel(R) Xeon(R) Processor @ 2.60GHz",
    "logicalCores": 2,
    "nodeVersion": "v22.22.3",
    "v8Version": "12.4.254.21-node.56"
  },
  "artifacts": {
    "wasmBytes": 324487,
    "kernelGzipBytes": 3495318,
    "kernelRawBytes": 6547296,
    "vanillaInitrdGzipBytes": 814278,
    "vanillaDiskGzipBytes": 71402,
    "debianDiskGzipBytes": 29584790
  },
  "vanillaLinux": {
    "coldBootTrialsMs": [3996.7, 3842.2, 3712.1],
    "coldBootMedianMs": 3842.2,
    "bootInstructions": 94703759,
    "effectiveMipsTrials": [23.7, 24.6, 25.5],
    "effectiveMipsMedian": 24.6,
    "wasmLinearMemoryMib": 145.1,
    "terminalRttTrialsMs": [20.78, 20.74, 2.15, 1.96, 4.2],
    "terminalRttMinMs": 1.96,
    "terminalRttMedianMs": 4.2,
    "terminalRttMaxMs": 20.78,
    "ext4UrandomWrite4MibMs": 9.5,
    "ext4UrandomWriteMibPerSec": 421.1,
    "ext4UncachedRead4MibMs": 213.4,
    "ext4UncachedReadMibPerSec": 18.7,
    "snapshotRawMib": 132.41,
    "snapshotGzipMib": 14.24,
    "snapshotSaveTrialsMs": [479.9, 172.7, 150.3],
    "snapshotSaveMedianMs": 172.7,
    "snapshotGzipTrialsMs": [524.3, 475.1, 457.9],
    "snapshotGzipMedianMs": 475.1,
    "snapshotRestoreTrialsMs": [89.7, 66.4, 66.7],
    "snapshotRestoreMedianMs": 66.7
  },
  "debianLinux": {
    "coldBootTrialsMs": [4424, 4274, 4434.8],
    "coldBootMedianMs": 4424,
    "bootInstructions": 104750000,
    "effectiveMipsTrials": [23.7, 24.5, 23.6],
    "effectiveMipsMedian": 23.7,
    "wasmLinearMemoryMib": 344.7
  }
}
```

---

## 7. Reconciliation Notes

The kernel asset is no longer a prebuilt download: Linux `7.3.1` is now compiled in-repo from `scripts/build-kernel.sh` against `configs/linux-7.3.1-riscv64.config`. Four consequences are visible in the numbers above.

1. **Retired instruction count grew (`62.07M` -> `94.70M` to the shell prompt), and this is real, not noise.** The shipping kernel is compiled in-repo from `configs/linux-7.3.1-riscv64.config`, which enables more subsystems than the prebuilt image it replaced (notably `MEMCG`, `SWAP`, `COMPACTION`/`MIGRATION`, and `DEBUG_KERNEL`-selected options pulled in by `CONFIG_EXPERT`). The extra work lands almost entirely in early boot (before the console banner). This is tracked as an accepted, open optimization item in `TODO.MD`; it does not threaten any gate, because `tests/unit/benchmarks.test.ts` gates on wall-clock (`coldBootMedianMs < 15,000`) and throughput (`effectiveMipsMedian > 20`), and both pass with margin (`3,842.2 ms`, `24.6 MIPS`). Instruction count is a diagnostic here, never an assertion target.

2. **Host vCPU allocation and V8 TurboFan warm-up dominate the residual jitter.** Across container snapshots and concurrent Vitest worker scheduling, Liftoff-to-TurboFan tier-up and 2-vCPU scheduling produce `15-20%` wall-clock spread on cold boots (`3,712`-`3,997 ms` Vanilla; `4,274`-`4,435 ms` Debian) while retiring essentially the same guest instruction count.

3. **First-touch `WebAssembly.Memory.grow` page faults on snapshots.** As visible in the 3-trial arrays above (`save`: `[479.9, 172.7, 150.3]` ms; `restore`: `[89.7, 66.4, 66.7]` ms), the first snapshot grows the WASM I/O staging buffer by `132.41 MiB`, whereas subsequent snapshots reuse the allocated pages.

4. **Zero-filled vs. non-zero `fsync` `dd` writes.** `dd if=/dev/urandom ... conv=fsync` followed by `sync && echo 3 > /proc/sys/vm/drop_caches` ensures deterministic CoW chunk materialization (`421.1 MiB/s` write, `18.7 MiB/s` uncached read).

All documentation (`README.md`, `docs/BENCHMARKS.md`, `docs/TEST_REPORT.md`, `docs/ASSETS.md`, and the PR description) is synchronized to the multi-trial run above (`2026-11-20T16:57:15.666Z`).

---

## 8. Browser E2E Suite Execution Timings (Headless Chromium `153.0.0`, `2026-11-20`)

| Playwright E2E Spec                     | Scope                                                                                                                                           | Measured Wall Time |
| --------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | -----------------: |
| `tests/e2e/foundation.spec.ts`          | WASM Worker execution + browser capability probe                                                                                                |         **0.29 s** |
| `tests/e2e/storage-persistence.spec.ts` | OPFS + IndexedDB chunk persistence across reload + full browser restart + `.wasmnux-disk` round-trip                                            |          **1.6 s** |
| `tests/e2e/browser-interface.spec.ts`   | Full UI boot, `@xterm/xterm` typing, `Ctrl+C`, Pause/Resume, gzipped VM snapshot, corrupt import banner, reload persistence, PWA Service Worker |         **10.3 s** |
| `tests/e2e/webgpu-graphics.spec.ts`     | Live WebGPU WGSL compute+render `/dev/fb0` pixel readback + `device.lost` recovery (`7.3 s`) & Canvas 2D fallback (`6.9 s`)                     |          **7.3 s** |
| `tests/e2e/vanilla-linux.spec.ts`       | Linux `7.3.1` + BusyBox `1.37.0` applets + persistent `ext4` across reload                                                                      |         **13.6 s** |
| `tests/e2e/quota-and-locks.spec.ts`     | Real origin quota exhaustion (`quota_exceeded` typing + recovery) & two-tab `navigator.locks` serialization (`3.2 s`)                           |         **45.7 s** |
| `tests/e2e/debian-linux.spec.ts`        | Official Debian `13.7` `riscv64` boot + real `apt-get update/install/remove` over `VirtIO-Net` + `dpkg` persistence across reload               |          **1.1 m** |

Full suite: `9 passed (2.6m)` with `workers: 1` on the 2-vCPU reference host.
