# WASMNux Reproducible Performance Benchmarks

All benchmarks in this document are produced by executing the genuine `wasmnux_wasm.wasm` RV64IMAFDC + Sv39 virtual machine against upstream **Linux `7.3.1`** (`riscv64`), **BusyBox `1.36.1`**, and **Debian GNU/Linux `13.7` ("trixie") `riscv64`** — zero mocks, zero synthetic loops.

---

## 1. Reproducible Benchmark Procedure

Run the dedicated multi-trial benchmark harness (with no background dev/preview server competing for CPU):

```bash
WASMNUX_BENCH_OUT=/tmp/benchmark-report.json npm run bench
# Equivalent direct command:
npx vitest run tests/unit/benchmarks.test.ts
```

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

## 2. Canonical Reference Environment (`2026-10-31T23:35:44.846Z`)

| Property | Value |
|---|---|
| Host Kernel | `Linux 6.1.158+` (`x86_64`) |
| Host CPU | `Intel(R) Xeon(R) Processor @ 2.60GHz` (`2` logical vCPUs) |
| WebAssembly Engine | Node.js `v22.22.3` / V8 `12.4.254.21-node.56` (`vitest v5.0.5`) |
| Rust Toolchain | `rustc 1.88.0` (`wasm32-unknown-unknown`, `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`) |
| Guest Architecture | `riscv64` (`RV64IMAFDC` + `Zicsr` + `Zifencei`, `Sv39` MMU, SBI v0.2) |
| Guest Kernel | Upstream Linux `7.3.1` (`vmlinux-7.3.1-riscv64.Image.gz`) |

---

## 3. Production Artifact Sizes

| Artifact | Path | Compressed / Bundled Size | Uncompressed Size | Notes |
|---|---|---:|---:|---|
| WASM VM Runtime | `public/wasm/wasmnux_wasm.wasm` | **304.6 KiB** (`311,906` B) | 304.6 KiB | Zero external Rust crate dependencies |
| Linux `7.3.1` Kernel | `public/images/vmlinux-7.3.1-riscv64.Image.gz` | **3.16 MiB** (`3,314,317` B) | 5.94 MiB (`6,230,832` B) | VirtIO MMIO (`blk`, `net`, `rng`), `ext4`, `SimpleFB`, `ttyS0` |
| Vanilla Initramfs | `public/images/vanilla-initramfs.cpio.gz` | **792.2 KiB** (`811,224` B) | ~1.58 MiB | Static `riscv64-linux-musl` BusyBox `1.36.1` + `wasmnux-fbtest` |
| Vanilla Base `ext4` Disk | `public/images/vanilla-disk.ext4.gz` | **69.7 KiB** (`71,424` B) | 64.0 MiB (sparse) | 256 KiB CoW chunked `ext4` rootfs |
| Debian `13.7` `ext4` Disk | `public/images/debian-rootfs.ext4.gz` | **28.21 MiB** (`29,584,790` B) | 256.0 MiB (sparse) | Official Debian `13.7` ("trixie") `riscv64` (`glibc 2.41`, `apt 3.0.3`, `dpkg 1.22.22`) |
| Browser UI Bundle | `dist/assets/index-*.js` | **115.3 KiB** (gzip) | 446.8 KiB | TypeScript orchestrator + WebGPU/Canvas2D + `@xterm/xterm` |

---

## 4. Multi-Trial Cold Boot, Instruction Throughput, & Memory Footprint

| Profile | Guest RAM / Disk | Cold Boot Median (`N=3`) | Raw Cold Boot Trials (ms) | Retired RV64GC Instructions | Effective MIPS Median (`N=3`) | Total WASM Linear Memory |
|---|---|---:|---|---:|---:|---:|
| **Vanilla Linux `7.3.1` (BusyBox `1.36.1`)** | 128 MiB / 64 MiB | **2,517.4 ms** (~2.52 s) | `[2662.3, 2517.4, 2507.3]` | `62,072,348` | **24.8 MIPS** (`[23.8, 25.1, 24.8]`) | **144.6 MiB** |
| **Debian `13.7` ("trixie") `riscv64`** | 256 MiB / 256 MiB | **3,249.0 ms** (~3.25 s) | `[3249.0, 3256.0, 3171.5]` | `78,000,000` | **24.0 MIPS** (`[23.9, 24.0, 24.6]`) | **344.8 MiB** |

---

## 5. Interactive Terminal, Virtual Block I/O, & Live Snapshots (Vanilla Linux `7.3.1`)

| Metric | Workload | Median / Primary | Raw Trials / Details |
|---|---|---|---|
| **Terminal Round-Trip Latency** | 5 serial `echo BENCH_RTT_N` round-trips (`NS16550A` + BusyBox `ash`) | **3.93 ms min / 12.92 ms median / 30.51 ms max** | `[12.92, 21.84, 4.05, 30.51, 3.93]` ms |
| **Guest `ext4` Non-Zero Sequential Write (`VirtIO-Blk`)** | `dd if=/dev/urandom of=/root/bench4m.bin bs=65536 count=64 conv=fsync && sync` (4 MiB) | **60.3 ms** (**66.4 MiB/s**) | Full random payload + `fsync` across 16 CoW chunks |
| **Guest `ext4` Uncached Sequential Read (`VirtIO-Blk`)** | `drop_caches` + `dd if=/root/bench4m.bin of=/dev/null bs=65536` (4 MiB) | **10.7 ms** (**375.6 MiB/s**) | Read after `sync && echo 3 > /proc/sys/vm/drop_caches` |
| **Live Full VM Snapshot Serialization** | `WNUXFULL` (`128.09 MiB` raw: 128 MiB RAM + VRAM + CPU/CSR/VirtIO + CoW overlay) | **131.0 ms** (median) | `[427.7, 129.2, 131.0]` ms (Trial 1 includes initial `memory.grow`) |
| **Snapshot Gzip Compression** | `128.09 MiB` raw -> **`5.23 MiB`** compressed (**24.5x reduction**) | **245.0 ms** (median) | `[242.8, 245.0, 245.8]` ms (`zlib` level 1) |
| **Live Full VM Snapshot Restore** | Instant resume into running Linux `7.3.1` shell | **40.6 ms** (median) | `[145.0, 39.9, 40.6]` ms (Trial 1 `145.0 ms` includes staging buffer allocation) |

---

## 6. Raw Benchmark JSON Output (`npm run bench`)

```json
{
  "timestampIso": "2026-10-31T23:35:44.846Z",
  "environment": {
    "os": "Linux 6.1.158+",
    "cpuModel": "Intel(R) Xeon(R) Processor @ 2.60GHz",
    "logicalCores": 2,
    "nodeVersion": "v22.22.3",
    "v8Version": "12.4.254.21-node.56"
  },
  "artifacts": {
    "wasmBytes": 311906,
    "kernelGzipBytes": 3314317,
    "kernelRawBytes": 6230832,
    "vanillaInitrdGzipBytes": 811224,
    "vanillaDiskGzipBytes": 71424,
    "debianDiskGzipBytes": 29584790
  },
  "vanillaLinux": {
    "coldBootTrialsMs": [2662.3, 2517.4, 2507.3],
    "coldBootMedianMs": 2517.4,
    "bootInstructions": 62072348,
    "effectiveMipsTrials": [23.8, 25.1, 24.8],
    "effectiveMipsMedian": 24.8,
    "wasmLinearMemoryMib": 144.6,
    "terminalRttTrialsMs": [12.92, 21.84, 4.05, 30.51, 3.93],
    "terminalRttMinMs": 3.93,
    "terminalRttMedianMs": 12.92,
    "terminalRttMaxMs": 30.51,
    "ext4UrandomWrite4MibMs": 60.3,
    "ext4UrandomWriteMibPerSec": 66.4,
    "ext4UncachedRead4MibMs": 10.7,
    "ext4UncachedReadMibPerSec": 375.6,
    "snapshotRawMib": 128.09,
    "snapshotGzipMib": 5.23,
    "snapshotSaveTrialsMs": [427.7, 129.2, 131],
    "snapshotSaveMedianMs": 131,
    "snapshotGzipTrialsMs": [242.8, 245, 245.8],
    "snapshotGzipMedianMs": 245,
    "snapshotRestoreTrialsMs": [145, 39.9, 40.6],
    "snapshotRestoreMedianMs": 40.6
  },
  "debianLinux": {
    "coldBootTrialsMs": [3249, 3256, 3171.5],
    "coldBootMedianMs": 3249,
    "bootInstructions": 78000000,
    "effectiveMipsTrials": [23.9, 24, 24.6],
    "effectiveMipsMedian": 24,
    "wasmLinearMemoryMib": 344.8
  }
}
```

---

## 7. Reconciliation Note on Earlier Single-Sample Runs

Earlier draft revisions showed slight differences between `docs/BENCHMARKS.md` (captured at `2026-10-31T22:32:50.701Z` via single-sample standalone `vitest run`) and the initial PR summary / `docs/TEST_REPORT.md` (captured at `2026-10-31T22:37:02.924Z` during `npm test`):
1. **Host vCPU allocation & V8 TurboFan warm-up**: Across container snapshots and concurrent Vitest worker scheduling, V8's Liftoff-to-TurboFan tier-up and host Xeon vCPU scheduling produced ~15–20% wall-clock jitter on single-trial cold boots (`1.97 s`–`2.66 s` for Vanilla; `2.60 s`–`3.31 s` for Debian) while retiring the exact same `~62M` / `~78M` guest instructions.
2. **First-touch `WebAssembly.Memory.grow` page faults on Snapshots**: As visible in the 3-trial arrays above (`save`: `[427.7, 129.2, 131.0]` ms; `restore`: `[145.0, 39.9, 40.6]` ms), the first snapshot grows the WASM I/O staging buffer by `128.09 MiB` (`143`–`171 ms` restore on Trial 1), whereas subsequent snapshots reuse the allocated pages (`~40 ms` restore).
3. **Zero-filled vs. non-zero `fsync` `dd` writes**: Replacing `dd if=/dev/zero` with `dd if=/dev/urandom ... conv=fsync` followed by `sync && echo 3 > /proc/sys/vm/drop_caches` ensures deterministic CoW chunk materialization (`66.4 MiB/s` write, `375.6 MiB/s` uncached read).

All documentation (`README.md`, `docs/BENCHMARKS.md`, `docs/TEST_REPORT.md`, and the PR description) is now synchronized to the multi-trial canonical run above (`2026-10-31T23:35:44.846Z`).

---

## 8. Browser E2E Suite Execution Timings (Headless Chromium `153.0.0`, `2026-10-31`)

| Playwright E2E Spec | Scope | Measured Wall Time |
|---|---|---:|
| `tests/e2e/foundation.spec.ts` | WASM Worker execution + browser capability probe | **0.39 s** |
| `tests/e2e/storage-persistence.spec.ts` | OPFS + IndexedDB chunk persistence across reload + full browser restart + `.wasmnux-disk` round-trip | **1.7 s** |
| `tests/e2e/browser-interface.spec.ts` | Full UI boot, `@xterm/xterm` typing, `Ctrl+C`, Pause/Resume, gzipped VM snapshot, corrupt import banner, reload persistence, PWA Service Worker | **8.2 s** |
| `tests/e2e/vanilla-linux.spec.ts` | Linux `7.3.1` + BusyBox `1.36.1` applets + persistent `ext4` across reload | **9.9 s** |
| `tests/e2e/webgpu-graphics.spec.ts` | Live WebGPU WGSL compute+render `/dev/fb0` pixel readback + `device.lost` recovery (`5.4s`) & Canvas 2D fallback (`4.9s`) | **10.3 s** |
| `tests/e2e/debian-linux.spec.ts` | Official Debian `13.7` `riscv64` boot + real `apt-get update/install/remove` over `VirtIO-Net` + `dpkg` persistence across reload | **66.0 s** |
