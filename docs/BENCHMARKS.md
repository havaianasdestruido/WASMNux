# WASMNux Reproducible Performance Benchmarks

All benchmarks in this document are produced by executing the genuine `wasmnux_wasm.wasm` RV64IMAFDC + Sv39 virtual machine against upstream **Linux `7.3.1`** (`riscv64`), **BusyBox `1.36.1`**, and **Debian GNU/Linux `13.7` ("trixie") `riscv64`** — zero mocks, zero synthetic loops.

## 1. Reproducing Benchmarks Locally

```bash
npm run bench
# Equivalent direct command:
npx vitest run tests/unit/benchmarks.test.ts
```

The benchmark suite emits structured JSON (`BENCHMARK_REPORT_JSON=...`) and enforces automated regression bounds on cold boot latency, instruction throughput (MIPS), `ext4` block I/O throughput, and snapshot latency.

---

## 2. Benchmark Environment

| Property | Value |
|---|---|
| Host OS | Linux x86_64 |
| WebAssembly Engine | V8 (Node.js `v22.22.3` / Chromium `153.0.0`) |
| Rust Toolchain | `rustc 1.88.0` (`wasm32-unknown-unknown`, `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`) |
| Guest Architecture | `riscv64` (`RV64IMAFDC` + `Zicsr` + `Zifencei`, `Sv39` MMU, SBI v0.2) |
| Guest Kernel | Upstream Linux `7.3.1` (`vmlinux-7.3.1-riscv64.Image.gz`) |

---

## 3. Production Artifact Sizes

| Artifact | Path | Compressed / Bundled Size | Uncompressed Size | Notes |
|---|---|---:|---:|---|
| WASM VM Runtime | `public/wasm/wasmnux_wasm.wasm` | **304.6 KiB** (`311,929` B) | 304.6 KiB | Zero external Rust crate dependencies |
| Linux `7.3.1` Kernel | `public/images/vmlinux-7.3.1-riscv64.Image.gz` | **3.16 MiB** (`3,314,317` B) | 5.94 MiB (`6,230,832` B) | VirtIO MMIO (`blk`, `net`, `rng`), `ext4`, `SimpleFB`, `ttyS0` |
| Vanilla Initramfs | `public/images/vanilla-initramfs.cpio.gz` | **792.2 KiB** (`811,224` B) | ~1.58 MiB | Static `riscv64-linux-musl` BusyBox `1.36.1` + `wasmnux-fbtest` |
| Vanilla Base `ext4` Disk | `public/images/vanilla-disk.ext4.gz` | **69.7 KiB** (`71,424` B) | 64.0 MiB (sparse) | 256 KiB CoW chunked `ext4` rootfs |
| Debian `13.7` `ext4` Disk | `public/images/debian-rootfs.ext4.gz` | **28.21 MiB** (`29,584,790` B) | 256.0 MiB (sparse) | Official Debian `13.7` ("trixie") `riscv64` (`glibc 2.41`, `apt 3.0.3`, `dpkg 1.22.22`) |
| Browser UI Bundle | `dist/assets/index-*.js` | **115.3 KiB** (gzip) | 446.8 KiB | TypeScript orchestrator + WebGPU/Canvas2D + `@xterm/xterm` |

---

## 4. Cold Boot, Instruction Throughput, & Memory Footprint

| Profile | Guest RAM | Virtual Disk | Cold Boot to Shell Prompt | Retired RV64GC Instructions | Effective Throughput | Total WASM Linear Memory |
|---|---:|---:|---:|---:|---:|---:|
| **Vanilla Linux `7.3.1` (BusyBox `1.36.1`)** | 128 MiB | 64 MiB `ext4` | **2,036.8 ms** (~2.04 s) | `61,745,696` | **30.3 MIPS** | **144.6 MiB** |
| **Debian `13.7` ("trixie") `riscv64`** | 256 MiB | 256 MiB `ext4` | **2,901.1 ms** (~2.90 s) | `77,750,000` | **26.8 MIPS** | **344.8 MiB** |

*Why cold boot is ~2–3 seconds:* `crates/wasmnux-riscv` implements fast-path direct-mapped 4 KiB `Sv39` instruction/load/store TLBs (`ITLB`/`DTLB` with `PTE_A`/`PTE_D` permission caching) and deterministic `WFI` CLINT timer fast-forwarding so idle kernel ticks never burn host CPU cycles.

---

## 5. Interactive Terminal, Virtual Block I/O, & Live Snapshots (Vanilla Linux `7.3.1`)

| Metric | Workload | Measured Result |
|---|---|---|
| **Terminal Round-Trip Latency (Min / Median / Max)** | 5 serial `echo BENCH_RTT_N` round-trips through `NS16550A` UART + BusyBox `ash` | **3.25 ms / 14.81 ms / 16.78 ms** |
| **Guest `ext4` Sequential Write (`VirtIO-Blk`)** | `dd if=/dev/zero of=/root/bench4m.bin bs=65536 count=64 && sync` (4 MiB) | **47.2 ms** (**84.8 MiB/s**) |
| **Guest `ext4` Uncached Sequential Read (`VirtIO-Blk`)** | `echo 3 > /proc/sys/vm/drop_caches && dd if=/root/bench4m.bin of=/dev/null bs=65536` (4 MiB) | **569.9 ms** (**7.0 MiB/s**) |
| **Live Full VM Snapshot Serialization** | `WNUXFULL` (128 MiB RAM + 1 MiB VRAM + CPU/CSR/PLIC/VirtIO + CoW Disk = `132.41 MiB` raw) | **484.4 ms** |
| **Snapshot Gzip Compression** | `132.41 MiB` raw -> **`5.32 MiB`** compressed (**24.9x reduction**, safe under IndexedDB limits) | **216.1 ms** |
| **Live Full VM Snapshot Restore** | Instant resume into running Linux `7.3.1` shell | **171.1 ms** |

---

## 6. Browser E2E Suite Execution Timings (Headless Chromium `153.0.0`)

| Playwright E2E Spec | Scope | Measured Wall Time |
|---|---|---:|
| `tests/e2e/foundation.spec.ts` | WASM Worker execution + browser capability probe | **0.27 s** |
| `tests/e2e/browser-interface.spec.ts` | Full UI boot, `@xterm/xterm` typing, `Ctrl+C`, Pause/Resume, gzipped VM snapshot, corrupt import banner, reload persistence, PWA Service Worker | **6.8 s** |
| `tests/e2e/webgpu-graphics.spec.ts` | Live WebGPU WGSL compute+render `/dev/fb0` pixel readback, `device.lost` recovery, & Canvas 2D fallback | **9.8 s** |
| `tests/e2e/storage-persistence.spec.ts` | OPFS + IndexedDB chunk persistence across reload + full browser restart + `.wasmnux-disk` round-trip | **1.9 s** |
| `tests/e2e/vanilla-linux.spec.ts` | Linux `7.3.1` + 35+ BusyBox `1.36.1` applets + persistent `ext4` across cold browser restart + clean reboot/poweroff | **14.2 s** |
| `tests/e2e/debian-linux.spec.ts` | Official Debian `13.7` `riscv64` boot + real `apt-get update/install/remove` over `VirtIO-Net` + `dpkg` persistence across browser restart + offline mode | **52.2 s** |
