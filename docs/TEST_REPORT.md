# WASMNux Verification & Test Evidence Report

This report tracks verification evidence for every completed phase and requirement in `TODO.MD`. No unexecuted test is ever marked as passing.

---

## Phase 0 — Repository Audit and Feasibility Evidence

| Test ID | Requirement Covered | Environment | Exact Command / Procedure | Expected Result | Actual Result | Status |
|---|---|---|---|---|---|---|
| `P0-01` | Initial repository audit | Debian 12 `x86_64` sandbox | `git status && git log --oneline -n 5 && ls -la` | Clean branch `arena/5741c179-wasmnux` at `c023751` with `LICENSE` (Apache-2.0) | Verified branch `arena/5741c179-wasmnux`, commit `c023751`, `LICENSE` (11,357 bytes) | PASS |
| `P0-02` | Host & toolchain provisioning check | Linux `6.1.158+` `x86_64` | Provision Rust `1.88.0`, `wasm32-unknown-unknown`, `rustfmt`, `clippy`, Clang/LLD `23.1.3`, Zig `0.17.0`; run `--version` and cross-compile RV64GC ELF + kernel object | All tools execute; `llvm-readelf -h` confirms `ELF64`, `Machine: RISC-V`, `Flags: 0x5, RVC, double-float ABI` | `rustc 1.88.0`, `cargo 1.88.0`, `rustfmt 1.8.0`, `clippy 0.1.88`, `clang 23.1.3`, `LLD 23.1.3`, `zig 0.17.0` verified; RV64GC static ELF compiled in `0.046s` | PASS |
| `P0-03` | Upstream Linux kernel provenance check | Git remote query | `git ls-remote --tags https://github.com/gregkh/linux.git` | Identify latest stable upstream Linux tag | Latest stable tag verified: `v7.3.1` (`dfc1145bbaa1c5dd626fd25d9ae8fc277864e7bd`) | PASS |
| `P0-04` | Official Debian stable provenance check | GitHub API / `debuerreotype/docker-debian-artifacts` `dist-riscv64` | Fetch `trixie/rootfs.os-release`, `trixie/rootfs.debian_version`, `trixie/slim/rootfs.manifest` | Confirm latest official Debian stable version and `riscv64` package versions | Verified `Debian GNU/Linux 13 (trixie)`, `DEBIAN_VERSION_FULL=13.7`, `apt 3.0.3`, `dpkg 1.22.22`, `libc6:riscv64 2.41-12+deb13u4`, `bash 5.2.37` | PASS |
| `P0-05` | Open-source emulator source audit | Local clone `/tmp/emuko_inspect` | Inspect `emuko`, `rvemu`, `riscv-rust`, `RV64emu-rs`, `v86`; audit `hart/memory.rs` and `dev.rs` | Identify reusable components and architectural defects prior to integration | Identified `emuko` (Apache-2.0) RV64IMAFDC foundation + documented 4 Sv39 MMU defects (`PTE_U`, `MXR`, `PTE_D` write-gate, PPN upper mask) and 32-bit WASM `usize` truncation fix in `docs/ARCHITECTURE.md` | PASS |

---

## Phase 1 — Project Foundation Evidence

| Test ID | Requirement Covered | Environment | Exact Command / Procedure | Expected Result | Actual Result | Status |
|---|---|---|---|---|---|---|
| `P1-01` | Rust workspace formatting, typecheck, Clippy, unit tests | Rust `1.88.0` `x86_64` | `cargo fmt --all -- --check && cargo check --workspace && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace` | 0 warnings, 0 errors, all 5 workspace crate unit tests pass | 5/5 Rust workspace crate tests passed (`0.04s`), 0 Clippy warnings | PASS |
| `P1-02` | WASM target compilation | `wasm32-unknown-unknown` | `./scripts/build-wasm.sh` | Produces optimized `public/wasm/wasmnux_wasm.wasm` | Built `public/wasm/wasmnux_wasm.wasm` (`428` bytes), `wasmnux_abi_version() == 1` | PASS |
| `P1-03` | TypeScript strict typecheck, ESLint, Prettier, Vitest | Node `v22.22.3` / TS `5.8.3` | `npm run format:check && npm run typecheck && npm run lint && npm test` | 0 lint/type/format errors; capability detector tests pass | Prettier clean, ESLint `strictTypeChecked` 0 errors, Vitest `3/3` passed (`5ms`) | PASS |
| `P1-04` | Real Chromium browser WASM + WebGPU + OPFS capability E2E | Playwright `1.64.1` + Chromium `153.0.0` | `npm run build && npm run test:e2e` | Loads WASM ABI v1 and detects WebAssembly, WebWorker, WebGPU (`google / swiftshader`), OPFS, IndexedDB, COOP/COEP | `1 passed (1.3s)`, `abiVersion: 1`, `webGpuAvailable: true`, `opfsAvailable: true`, `crossOriginIsolated: true`, 0 page errors | PASS |

---

## Phase 2 — Virtualization Core Evidence

| Test ID | Requirement Covered | Environment | Exact Command / Procedure | Expected Result | Actual Result | Status |
|---|---|---|---|---|---|---|
| `P2-01` | Sparse CoW virtual block disk (`wasmnux-storage`) | Rust `1.88.0` | `cargo test -p wasmnux-storage` | 64 KiB CoW overlays, CRC32 check vector, quota & read-only enforcement, snapshot/restore, `.wasmnux-disk` export/import tamper rejection | `5/5` passed (`0.01s`) | PASS |
| `P2-02` | User-mode virtual Ethernet switch (`wasmnux-net`) | Rust `1.88.0` | `cargo test -p wasmnux-net` | RFC 1071 checksum, ARP + ICMPv4 echo, DNS `10.0.3.x` virtual mapping, 40 KiB multi-segment TCPv4 HTTP bridge | `3/3` passed (`0.00s`) | PASS |
| `P2-03` | `RV64IMAFDC` + `Zicsr`/`Zifencei`/`Zba`/`Zbb` CPU, `Sv39` MMU, FDT, SBI v2.0, devices, & snapshots (`wasmnux-riscv`) | Rust `1.88.0` | `cargo test -p wasmnux-riscv` | ISA suite, Sv39 `PTE_U`/`SUM`/`SMEP`/`PTE_A`/`PTE_D` + CoW `W=0` fault, CSR privilege isolation, FDT `0xd00dfeed`, SBI SRST, SimpleFB dirty scanlines, snapshot roundtrip | `5/5` passed (`0.05s`) | PASS |
| `P2-04` | VirtIO MMIO v2 split-virtqueue Block, Net, and RNG (`wasmnux-virtio`) | Rust `1.88.0` | `cargo test -p wasmnux-virtio --lib` | `VirtioBlkMmio` sector write/read DMA + PLIC IRQ, `VirtioNetMmio` 12-byte v1 header ARP TX/RX + byte config reads, `VirtioRngMmio` entropy DMA | `2/2` passed (`0.00s`) | PASS |
| `P2-05` | End-to-end S-mode mini-OS hardware boot integration | Rust `1.88.0` | `cargo test -p wasmnux-virtio --test mini_os_boot` | Boots RV64 S-mode `Image`, activates `Sv39`, services CLINT timer IRQ via `WFI`/`SRET`, writes UART `"TOK"`, reads Goldfish RTC, paints SimpleFB row 20, DMAs sector 0 via VirtIO-Blk, powers off via Syscon | `1/1` passed (`0.02s`) | PASS |
| `P2-06` | Dual-target native + `wasm32` Clippy & formatting gate | Rust `1.88.0` (`x86_64` + `wasm32-unknown-unknown`) | `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo clippy --workspace --target wasm32-unknown-unknown -- -D warnings` | Zero warnings or formatting diffs on both 64-bit native and 32-bit WASM targets | 0 warnings, 0 errors across all 5 crates (`17/17` total workspace tests passed) | PASS |

---

## Phase 3 — WASM Runtime Evidence

| Test ID | Requirement Covered | Environment | Exact Command / Procedure | Expected Result | Actual Result | Status |
|---|---|---|---|---|---|---|
| `P3-01` | Zero-import WASM compilation (`crates/wasmnux-wasm`) | Rust `1.88.0` `wasm32-unknown-unknown` | `./scripts/build-wasm.sh && node -e '...WebAssembly.Module.imports(mod)...'` | Release LTO `.wasm` builds cleanly with `imports: []` and 43 ABI exports | Built `public/wasm/wasmnux_wasm.wasm` (`308,951` bytes), `imports: []`, `43` exports, `2/2` Rust ABI tests passed | PASS |
| `P3-02` | Strict TS WASM bindings, bounds validation, & multi-cycle stress (`src/wasm/bindings.ts`) | Vitest `3.2.7` / Node `v22.22.3` | `npm test` (`tests/unit/wasm-runtime.test.ts`) | Rejects invalid profile IDs, RAM bounds, empty kernels, corrupt snapshots; completes 5 `create -> boot -> snapshot -> restore -> destroy` cycles cleanly | `6/6` Vitest tests passed (`340ms`), 0 leaked handles | PASS |
| `P3-03` | Worker RPC schema validation & cooperative fallback (`src/worker/*`, `src/runtime/vm-client.ts`) | Vitest `3.2.7` | `npm test` (`validates Worker RPC envelopes and executes cooperative fallback end-to-end`) | Malformed RPC command emits `vm_error`; cooperative fallback boots RV64 smoke kernel to `"CLIENT-BRIDGE-OK"` | Passed (`CLIENT-BRIDGE-OK` + `powered_off` verified) | PASS |
| `P3-04` | Strict TypeScript + ESLint `strictTypeChecked` + Prettier | TS `5.8.3` / ESLint `9.39.0` | `npm run format:check && npm run typecheck && npm run lint` | 0 warnings, 0 errors, zero `any` types | 0 errors, 0 warnings across `src/` and `tests/` | PASS |
| `P3-05` | Real Chromium Web Worker WASM RV64 execution E2E | Playwright `1.64.1` + Chromium `153.0.0` | `npm run build && npm run test:e2e` | Dedicated module Worker (`vm.worker.ts`) boots RV64 guest in WASM, emits `"WORKER-RV64-OK"` + SimpleFB row 4 pixel `0x0034d000` | `1 passed (1.4s)`, `transportMode: "worker"`, `serialBanner: "WORKER-RV64-OK"`, `dirtyScanlineObserved: true` | PASS |

---

## Phase 4 — Persistent Virtual Storage Evidence

| Test ID | Requirement Covered | Environment | Exact Command / Procedure | Expected Result | Actual Result | Status |
|---|---|---|---|---|---|---|
| `P4-01` | `.wasmnux-disk` CRC32 & Rust<->TS binary wire compatibility | Vitest `3.2.7` + `wasmnux_wasm.wasm` | `npm test` (`tests/unit/storage-engine.test.ts`: `computes exact IEEE 802.3 CRC32 and interoperates bit-for-bit with Rust WASM bundles`) | TS-encoded `WASMNUXD` bundle imports into Rust `SparseBlockDisk` and re-exports identically; rejects `../traversal` & invalid IDs | Passed (`crc32("123456789") === 0xcbf43926`, bit-for-bit roundtrip verified) | PASS |
| `P4-02` | Two-phase crash-consistent generation journal, profile isolation, & quota limits | Vitest `3.2.7` | `npm test` (`isolates profiles, enforces quotas, and recovers cleanly from mid-write crashes`) | Simulated mid-commit power loss + orphan `g99` chunk rolls back cleanly to generation 1; quota cap rejects overflow without corruption; `vanilla` reset leaves `debian` intact | Passed (`recoveredOrphanChunks: 1`, `committedGeneration: 1`, `debian` preserved) | PASS |
| `P4-03` | Named snapshots/backups, restore, & tampered/cross-profile rejection | Vitest `3.2.7` | `npm test` (`supports named backups/restoration and rejects corrupted or mismatched bundles`) | Overwriting disk then restoring `snap-clean` reverts exact chunk & CRC32; 1-byte tampered bundle & cross-profile import rejected | Passed (`9/9` total Vitest unit tests passed) | PASS |
| `P4-04` | Real Browser OPFS (`WNXC` v1) + IndexedDB (`idb-only` fallback) persistence | Playwright `1.64.1` + Chromium `153.0.0` | `npm run test:e2e` (`tests/e2e/storage-persistence.spec.ts`) | `createAuto()` selects `opfs+idb`, `forceIdbOnly: true` selects `idb-only`, chunks survive full `page.reload()` | Passed (`mode: "opfs+idb"`, `idbOnlyMode: "idb-only"`, reload verified) | PASS |
| `P4-05` | Full browser process close & reopen persistence (`launchPersistentContext`) | Playwright `1.64.1` + Chromium `153.0.0` (`userDataDir`) | `npm run test:e2e` (`await ctx1.close()` -> fresh `launchPersistentContext(userDataDir)`) | `vanilla` & `debian` OPFS chunks, IDB fallback chunks, named backup `snap-v1`, and `terminal.fontSize` survive complete browser restart | `2 passed (3.2s)` (`vanillaCrc`, `debianCrc`, `snap-v1`, `fontSize: 15` intact) | PASS |
| `P4-06` | Strict TypeScript + ESLint `strictTypeChecked` + Prettier gate | TS `5.8.3` / ESLint `9.39.0` | `npm run format:check && npm run typecheck && npm run lint` | Zero errors, zero warnings, runtime-guarded preference deserialization | 0 errors, 0 warnings | PASS |

---

## Phase 5 — Vanilla Linux (`7.3.1` + BusyBox `1.36.1`) Evidence

| Test ID | Requirement Covered | Environment | Exact Command / Procedure | Expected Result | Actual Result | Status |
|---|---|---|---|---|---|---|
| `P5-01` | Upstream Linux `7.3.1` `riscv64` C compilation & driver verification | Clang/LLD `23.1.3` + `gregkh/linux` `v7.3.1` (`79a907609d53...`) | `./scripts/build-kernel.sh` | All 30 mandatory Kconfig symbols (`SERIAL_8250_CONSOLE`, `VIRTIO_{MMIO,BLK,NET}`, `HW_RANDOM_VIRTIO`, `FB_SIMPLE`, `FB_DEVICE`, `RTC_DRV_GOLDFISH`, `POWER_RESET_SYSCON`, `EXT4_FS`, `SECCOMP`, etc.) `=y`; `RSC\x05` magic verified | Built `vmlinux-7.3.1-riscv64.Image.gz` (`6,230,832` B raw / `3,314,317` B gz, magic `0x52534305`) | PASS |
| `P5-02` | Static `riscv64-linux-musl` BusyBox `1.36.1` + initramfs + `ext4` disk | Zig `0.17.0` (`-DBB_GLOBAL_CONST=`) + `mkfs.ext4` `1.47.0` | `./scripts/build-busybox.sh && ./scripts/build-vanilla-rootfs.sh` | Static `ELF64 RISC-V` BusyBox (`1,259,504` B, no `PT_INTERP`) with all 39 mandatory applet configs, `vanilla-initramfs.cpio.gz` (`793 KiB`), `vanilla-disk.ext4.gz` (`70 KiB`) | Built & verified | PASS |
| `P5-03` | Live Linux `7.3.1` WASM boot + all 31 mandatory BusyBox applets | Vitest `3.2.7` + `wasmnux_wasm.wasm` | `npm test` (`tests/unit/vanilla-linux.test.ts`) | Boots Linux `7.3.1` in ~2.3s; executes `sh`, `ash`, `ls`, `cd`, `cat`, `cp`, `mv`, `rm`, `mkdir`, `rmdir`, `mount`, `umount`, `ps`, `kill`, `dmesg`, `uname`, `free`, `df`, `du`, `grep`, `sed`, `awk`, `find`, `tar`, `gzip`, `touch`, `chmod`, `chown`, `ln`, `readlink`, `date`, `sleep`, `top`, `reboot`, `poweroff` | `ALL_APPLETS_VERIFIED`, `fbAfterTest.yMax === 400`, ICMP `0% packet loss`, bridged HTTP `wget` verified | PASS |
| `P5-04` | Guest `reboot`, `poweroff`, & `ext4` `/dev/vda` persistence | Vitest `3.2.7` | `npm test` (`tests/unit/vanilla-linux.test.ts`) | `/root/persist-proof.txt` survives in-place `reboot` (`hardReset`) AND `poweroff` -> `PersistentStorageEngine` commit -> fresh VM cold boot | `REBOOT_CHECK_OK` & `COLD_PERSIST_OK` verified (`9.26s` total) | PASS |
| `P5-05` | Real Chromium 153 Web Worker Vanilla Linux `7.3.1` boot + OPFS reload persistence | Playwright `1.64.1` + Chromium `153.0.0` | `npm run test:e2e` (`tests/e2e/vanilla-linux.spec.ts`) | Boots Linux `7.3.1` in Dedicated Web Worker, writes `/root/e2e.txt` on `ext4`, powers off, reloads tab, re-boots Linux `7.3.1`, reads `/root/e2e.txt` from OPFS+IDB | `3 passed (10.8s)` (`storageMode: "opfs+idb"`, `persistedContentFound: true`) | PASS |




