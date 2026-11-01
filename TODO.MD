# TODO — Browser-Native Linux (WASMNux)

Central implementation plan, acceptance criteria and verification tracker for **WASMNux**
(browser-native RISC-V Linux on Rust + WebAssembly + WebGPU + persistent browser storage).

> **Status policy**
> * `[x]` — implementation **and** its required automated test have executed and passed.
>   The evidence ID (e.g. `E-BOOT-01`) refers to `docs/TEST_REPORT.md`.
> * `[ ]` — not implemented, not verified, or verified only in an environment that is not
>   the target environment. The trailing note says what is missing and what would close it.
>
> Nothing in this file may be marked `[x]` on the strength of a PR description, a green
> badge, or a benchmark claim. Every `[x]` below is backed either by a named CI gate in
> `.github/workflows/ci.yml` (which runs `cargo fmt`, `cargo check --all-targets`,
> `cargo clippy` for host **and** `wasm32`, `cargo test --workspace`, `build-wasm.sh`,
> TypeScript format/lint/typecheck, Vitest, production build and Playwright E2E), or by an
> entry in `docs/TEST_REPORT.md`.

---

## Supporting evidence

| ID | Evidence |
| --- | --- |
| `E-CI` | CI run `41259403266` on the consolidation branch: all 13 gates green |
| `E-CI-2` | CI run `41259753779` on `main` @ `f3eba73`: all 13 gates green (post-merge) |
| `E-BOOT-01` | Vanilla Linux reaches `wasmnux-vanilla:~#` in ~2 s / 63.3 M instructions |
| `E-BOOT-02` | Debian 13.7 reaches `root@wasmnux-debian:/#` in ~3 s / 77.8 M instructions |
| `E-BB` | In-guest BusyBox: `uname`, `ps`, `free`, `df`, `cat`, `mkdir`, `echo`, `sync`, `poweroff` |
| `E-DEB-01` | Debian `dpkg 1.22.22`, `apt 3.0.3`, 82 packages, `eth0` `10.0.2.15/24` |
| `E-DEB-02` | `apt-get update` + `apt-get install -y wasmnux-hello` over VirtIO-Net, then native ELF runs |
| `E-DEB-03` | Package DB + files survive poweroff → reboot; `apt-get remove` cleans up |
| `E-PERS-01` | ext4 file content identical after poweroff → reboot with restored overlay |
| `E-ASSET` | `public/assets-manifest.json` verified by `tests/unit/assets-manifest.test.ts` |

---

## Phase 0 — Repository audit and feasibility

- [x] Inventory the starting repository (`LICENSE`, single `c023751` commit) and record it. `E-CI`
- [x] Identify host toolchains and registries reachable from the build environment.
- [x] Justify hardware ISA emulation in WASM instead of compiling Linux to `wasm32` (`docs/ARCHITECTURE.md`).
- [x] Comparative audit of `emuko`, `riscv-rust`, `rvemu`, `RV64emu-rs`, `v86` (`docs/ARCHITECTURE.md`).
- [x] Select RV64GC `lp64d` + `Sv39` as the guest architecture.
- [x] Verify kernel provenance: `gregkh/linux` tag `v7.3.1` = `dfc1145bbaa1c5dd626fd25d9ae8fc277864e7bd`. `docs/ASSETS.md`
- [x] Verify Debian provenance: `trixie` `rootfs.debian_version` = `13.7` from official `docker-debian-artifacts`. `docs/ASSETS.md`
- [x] Document dependency and licence inventory (`THIRD_PARTY_NOTICES.md`, `docs/SECURITY.md`).
- [x] Document limitations and security boundaries (`docs/ARCHITECTURE.md`, `docs/SECURITY.md`).

## Phase 1 — Project foundation

- [x] Rust workspace: `wasmnux-riscv`, `wasmnux-storage`, `wasmnux-virtio`, `wasmnux-net`, `wasmnux-wasm`. `E-CI`
- [x] Strict TypeScript app (`strict`, `noUncheckedIndexedAccess`) building with Vite. `E-CI`
- [x] `wasm32-unknown-unknown` release build via `scripts/build-wasm.sh`. `E-CI`
- [x] rustfmt + Clippy `-D warnings` (host and `wasm32`) + ESLint + Prettier. `E-CI`
- [x] CI covering Rust, TypeScript, Vitest and Playwright. `E-CI`
- [x] CI failure diagnostics: every gate's output is teed and the decisive excerpts are
  re-published as annotations and to a `ci-diagnostics` branch, because run logs live in
  object storage that restricted networks cannot reach.
- [x] Browser capability detection with graceful degradation (`tests/unit/capabilities.test.ts`).

## Phase 2 — Virtualization core

- [x] Single-hart RV64IMAFDC + Zicsr/Zifencei/Zba/Zbb interpreter, M/S/U modes.
- [x] RV64M divide/remainder edge semantics (div by zero, `MIN / -1`) — `rv64m_div_rem_zero_and_overflow_semantics_match_riscv_spec`.
- [x] `FCVT.L.D` / `FCVT.LU.D` / `FCVT.D.LU` saturation semantics — `fcvt_l_d_and_fcvt_d_lu_saturate`.
- [x] RVC (compressed) expansion, including `C.ADDI`/`C.SUB` decimal encodings.
- [x] `Sv39` page walker with `PTE_U`/`SUM`/`SMEP`/`PTE_A`/`PTE_D` enforcement — `sv39_enforces_pte_u_sum_smep_and_cow_dirty_bit`.
- [x] Byte-accurate unaligned 4/8-byte guest accesses, with faults when they leave RAM — `unaligned_guest_access_is_byte_accurate_and_bounds_checked`.
- [x] Malformed kernel `Image` rejection (short/bad magic) and oversized/empty payload rejection — `malformed_kernel_images_are_rejected`.
- [x] PLIC interrupt delivery, CLINT timer, `WFI`/`SRET` returns.
- [x] Dynamic FDT generation (`0xd00dfeed`) and RISC-V `Image` load protocol.
- [x] SBI v1/v2 `BASE`/`TIME`/`IPI`/`RFENCE`/`HSM`/`SRST`/`DBCN`.
- [x] VirtIO-MMIO block, net and RNG devices with interrupt-driven virtqueues.
- [x] Goldfish RTC and syscon poweroff/reboot.
- [x] Full VM snapshot save/restore and deterministic replay (`tests/unit/wasm-runtime.test.ts`).
- [ ] Multi-hart (SMP) execution. *(Single hart only; the FDT advertises one hart.)*

## Phase 3 — WASM runtime and ABI

- [x] Zero-import WASM module with a versioned C ABI (43 exports, `imports: []`). `E-CI`
- [x] Strict TypeScript bindings with bounds validation and typed error kinds (`tests/unit/wasm-runtime.test.ts`).
- [x] Dedicated module `Worker` transport with RPC schema validation.
- [x] Multi-cycle create → boot → snapshot → restore → destroy without handle leaks.
- [x] Reject corrupt snapshots and corrupt disk bundles with typed errors.
- [x] Explicit lifecycle states (`created`/`running`/`paused`/`powered_off`/`reboot`).
- [ ] WASM memory growth is bounded by configuration, but a long-running soak test
  (hours, many reboots) has not been executed.

## Phase 4 — Persistent virtual storage

- [x] Sparse 64 KiB copy-on-write overlay over an immutable base image.
- [x] CRC32-bearing chunk envelope with versioned magic (`WXNC`), profile isolation.
- [x] Atomic quota enforcement and read-only mode (Rust `wasmnux-storage` tests).
- [x] Crash-consistent generation journal; orphaned chunks roll back on recovery.
- [x] Disk bundle export/import with SHA-256 + CRC verification and traversal rejection.
- [x] OPFS writes are bounded to 64 KiB pieces and the persisted size is verified, so a
  short write is reported as `io_interrupted` instead of silently truncating a chunk —
  `tests/unit/opfs-write-integrity.test.ts`.
- [x] IndexedDB fallback backend with the same envelope format.
- [x] Guest ext4 contents survive poweroff → reboot exactly. `E-PERS-01`
- [x] Disk state survives a full browser-process restart (`launchPersistentContext`). `E-CI`
- [ ] Real browser quota exhaustion (navigator quota) is exercised only through simulated
  quota in unit tests, not by filling a real origin until the browser refuses writes.
- [ ] Cross-tab concurrency (two tabs on one profile) is not locked with the Web Locks API.

## Phase 5 — Vanilla Linux

- [x] Kernel `Image` boots: VFS, ext4, VirtIO-Blk, PLIC, RTC, `simplefb` all initialise. `E-BOOT-01`
- [x] BusyBox shell is interactive; `uname`, `ps`, `free`, `df`, `cat`, `mkdir`, `echo`, `sync` behave. `E-BB`
- [x] Persistent ext4 root mounted from `/dev/vda`; files survive a reboot. `E-PERS-01`
- [x] Clean `poweroff` through syscon; clean `reboot` through SBI S-RST.
- [x] Boot and persistence replay in Playwright inside real Chromium. `E-CI`
- [ ] The shipped kernel/initramfs/disk were built outside this environment. The build
  scripts (`scripts/build-kernel.sh`, `scripts/build-busybox.sh`,
  `scripts/build-vanilla-rootfs.sh`) exist and are documented, but a from-source rebuild
  has not been re-executed and byte-compared here.
- [ ] BusyBox 1.37.0. Assets currently ship 1.36.1; bumping requires rebuilding the
  initramfs and ext4 image and re-running the applet suite (see `docs/ASSETS.md`).

## Phase 6 — Debian compatibility

- [x] Official Debian 13 (`trixie`) `riscv64` rootfs boots to a root shell. `E-DEB-01` `E-BOOT-02`
- [x] `uname`, `cat /etc/debian_version` (`13.7`), `dpkg --version` (`1.22.22`), `apt-get --version` (`3.0.3`). `E-DEB-01`
- [x] GNU/busybox userspace utilities and a working `dpkg -l` (82 packages). `E-DEB-01`
- [x] `apt-get update` against the configured repository over `eth0` (`10.0.2.15/24`). `E-DEB-02`
- [x] `apt-get install -y` of a real `riscv64` `.deb`; the installed native ELF executes. `E-DEB-02`
- [x] `apt-get remove` removes the package and its files; `dpkg-query` confirms absence. `E-DEB-03`
- [x] Package database and files persist across poweroff → reboot. `E-DEB-03`
- [x] Package state persists across a browser reload. `E-CI`
- [ ] **Official Debian archive access.** The guest's `debian.sources` is deliberately
  disabled and apt points at the bundled four-package repository
  (`http://apt.wasmnux.local/debian-repo`). This proves apt/dpkg work over a real HTTP
  index on the emulated NIC; it is **not** access to `deb.debian.org`. Closing this needs
  a documented relay/egress design (see Phase 9).
- [ ] Installing a large real-world package tree (e.g. `build-essential`) from an official
  mirror. Blocked by the item above.
- [ ] `systemd`-based boot. The guest uses a minimal `rcS` console init, not `systemd`.

## Phase 7 — Graphics

- [x] Guest `simple-framebuffer` at `0x5000_0000`, dirty-scanline tracking, XRGB8888.
- [x] WebGPU compute+render pipeline presents real guest `/dev/fb0` frames. `E-CI`
- [x] Canvas 2D fallback renders identical pixels when WebGPU is unavailable. `E-CI`
- [x] Resize handling and GPU device-loss recovery without restarting the guest. `E-CI`
- [ ] Hardware (non-SwiftShader) WebGPU validation. CI Chromium exposes a software
  adapter; a run on real GPU hardware has not been recorded.
- [ ] Input devices (keyboard/pointer into guest `/dev/input`) are not implemented; the
  console is driven through the serial terminal.

## Phase 8 — Browser interface

- [x] xterm.js terminal wired to the guest UART; typing reaches the guest. `E-CI`
- [x] Pause/resume, profile switching, persistent UI settings. `E-CI`
- [x] Snapshot **creation** with a named entry in the UI. `E-CI`
- [x] Snapshot save/restore at the WASM ABI level with deterministic replay.
- [x] Invalid disk import surfaces a dismissible error banner; no unhandled rejections. `E-CI`
- [x] Structured diagnostics, capability report and milestone list. `E-CI`
- [x] Service worker registers and caches the app shell. `E-CI`
- [ ] Snapshot **restore** and **delete** through the UI. Only creation is E2E-tested;
  restore/delete are covered at the ABI level but not driven through the browser.
- [ ] Offline cold start from the service worker cache with the network disabled. The
  worker registers, but a `context.setOffline(true)` boot has not been run.
- [ ] Firefox and WebKit. Only Chromium is executed.

## Phase 9 — Networking

- [x] Documented browser networking constraints (no raw sockets/ICMP/ARP; fetch bridge).
- [x] Rust user-mode Ethernet/ARP/ICMP/DNS/DHCP/TCP stack behind VirtIO-Net.
- [x] `eth0` up with `10.0.2.15/24`, gateway `10.0.2.2`, DNS `10.0.2.3`. `E-DEB-01`
- [x] HTTP bridge: guest TCP → `wasmnux_net_pop_http_request` → host fetch → guest socket. `E-DEB-02`
- [x] SSRF filtering of private/loopback destinations (`tests/unit/network-bridge.test.ts`).
- [ ] Arbitrary internet HTTPS/TLS. There is no TLS implementation and no documented
  relay protocol, so guests cannot reach the public web (including official Debian mirrors).
- [ ] Long-lived connections / WebSocket relay. Not implemented.

## Phase 10 — Security and reliability

- [x] WASM boundary validates every pointer/length pair; typed errors instead of traps
  escaping to JS.
- [x] Worker RPC payloads are schema-validated; malformed envelopes are rejected.
- [x] Disk import rejects oversize, truncated, wrong-profile and traversal payloads.
- [x] Storage identifiers are allowlisted (`^[a-z0-9_-]+$`); no host filesystem access.
- [x] Corrupted snapshots and corrupted chunk envelopes are rejected, not applied.
- [x] Interrupted OPFS writes are detected instead of silently truncating chunks.
- [x] Crash recovery after simulated power loss mid-write (`tests/unit/storage-engine.test.ts`).
- [x] Profile isolation: writes in one profile never appear in the other. `E-CI`
- [x] `npm audit --audit-level=low` is part of CI. `E-CI`
- [ ] No dependency or boundary fuzzing (no `cargo-fuzz`/`libFuzzer` target).
- [ ] No third-party security review of `crates/wasmnux-riscv`.

## Phase 11 — Comprehensive testing

- [x] `cargo fmt --all -- --check`. `E-CI`
- [x] `cargo check --workspace --all-targets`. `E-CI`
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings` (host). `E-CI`
- [x] `cargo clippy --workspace --target wasm32-unknown-unknown --all-features -- -D warnings`. `E-CI`
- [x] `cargo test --workspace` (19 Rust tests across 5 crates + mini-OS integration). `E-CI`
- [x] `./scripts/build-wasm.sh`. `E-CI`
- [x] Prettier, ESLint, `tsc --noEmit`. `E-CI`
- [x] Vitest (12 files / 32 tests including asset manifest and OPFS integrity). `E-CI`
- [x] Production Vite build. `E-CI`
- [x] Playwright E2E (6 specs: foundation, vanilla, Debian, storage restart, WebGPU/Canvas2D,
  browser interface). `E-CI`
- [x] Kernel boot, BusyBox behaviour, Debian apt/dpkg, ext4 persistence reproduce outside
  the browser via a headless harness driving the same `.wasm`. `E-BOOT-01` `E-BOOT-02` `E-BB` `E-DEB-02` `E-DEB-03` `E-PERS-01`
- [x] Benchmarks are multi-trial medians rather than single samples (`tests/unit/benchmarks.test.ts`).
- [ ] The benchmark suite is timing-sensitive; one unrelated CI run showed a transient
  Vitest failure that did not reproduce. The thresholds have not been de-flaked formally.

## Phase 12 — Release readiness

- [x] Both profiles boot from a cold profile with no prior storage. `E-CI`
- [x] Guest file state and Debian package state survive reload and browser restart. `E-CI` `E-DEB-03`
- [x] Clean `git status`; assets and provenance documented. `E-ASSET`
- [x] `TODO.MD` reconciled with real results; limitations recorded.
- [ ] Snapshot restore/delete E2E, offline cold start, second browser engine, hardware
  WebGPU, official Debian mirror access, and a from-source guest rebuild all remain open.
  See the Phase 8, 6, 7 and 5 items above.

---

## Known limitations (summary)

1. **No official Debian archive access.** apt is served by a bundled four-package
   repository over the in-browser bridge; `deb.debian.org` is unreachable because there is
   no TLS stack and no egress relay. This is the largest honest gap versus "Debian package
   management".
2. **No TLS anywhere in the guest network path.**
3. **Single hart**, interpreter-only; ~30 MIPS, so the guest is usable but not fast.
4. **Chromium only**; Firefox/Safari unverified.
5. **Software WebGPU adapter** in CI; hardware adapters unverified.
6. **No snapshot restore/delete through the UI, and no offline cold start.**
7. **Guest assets are prebuilt**; the from-source pipeline is documented but not re-run.

---

## Consolidation outcome — five candidate PRs

Exactly **one** of the five candidate PRs was merged: **#4**, as `main` `438b831`. The other
four were reviewed, mined for anything that could be independently verified, and closed as
superseded. No second branch history was merged and nothing was cherry-picked wholesale.

| PR | Outcome | Preserved in `main` | Excluded |
| --- | --- | --- | --- |
| #1 | closed, superseded | CI-diagnostics publishing; asset provenance (`docs/ASSETS.md`, `public/assets-manifest.json`, `tools/hash-assets.mjs`, `tests/unit/assets-manifest.test.ts`) | second emulator (`crates/wasmnux-core`), unexecutable from-source guest pipeline, non-reproducible `versions.sh` "latest tag" resolution |
| #2 | closed, superseded | `fcvt_l_d_and_fcvt_d_lu_saturate`, `unaligned_guest_access_is_byte_accurate_and_bounds_checked`, `malformed_kernel_images_are_rejected` | vendored emulator, prototype UI, unconditional tracing in `cpu.rs` |
| #3 | closed, superseded | nothing executable | second emulator with no MMU, prototype `web/`; its `ARCHITECTURE.md` §7 (guest networking "not feasible") is contradicted by `E-DEB-02` and was deliberately not carried over |
| #5 | closed, superseded | OPFS ≤64 KiB bounded writes + `io_interrupted` short-write reporting (`src/storage/opfs-backend.ts`, `tests/unit/opfs-write-integrity.test.ts`); asset manifest idea | BusyBox 1.37.0 config (needs a non-reproducible initramfs/ext4 rebuild), vendored emulator, unverified snapshot/offline paths |

Each closure comment on GitHub lists the same preserved-versus-excluded split with exact
paths/lines, and claims preservation only where the artefact is verifiably present in `main`.

### Still open after consolidation

- The four PRs above stay **closed, not merged**; their head branches are untouched history.
- GitHub Pages deployment (workflow from #6/#7) fails at `configure-pages` because Pages is
  not enabled on the repository — a repository-settings action only an admin can take. This
  does not affect the `CI` workflow, which is green on `main`.
- Everything listed under "Known limitations (summary)" above remains open; none of it is
  marked `[x]`.
