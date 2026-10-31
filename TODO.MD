# TODO — Browser-Native Linux

> Source of truth for project status. An item is marked `[x]` **only** after
> its implementation *and* associated tests pass, with evidence in
> `docs/TEST-REPORT.md`. Incomplete items are never deleted to look finished.
> Architecture rationale: `docs/ARCHITECTURE.md`.
> Status at a glance: **Phase 0 complete · Phase 1 complete (CI green incl. browser e2e) ·
> Phases 2–4 substantially implemented and unit-verified; first actual Linux boot (Phase 5)
> still gated on guest-image CI. Evidence: `docs/TEST-REPORT.md`.**

## Phase 0 — Repository Audit and Feasibility
- [x] Inspect the existing repository. *(Empty except Apache-2.0 LICENSE; branch arena/c540599d-wasmnux.)*
- [x] Identify existing infrastructure and reusable components. *(None in-repo; external candidates evaluated below.)*
- [x] Investigate Linux kernel execution inside WASM. *(No upstream WASM kernel port exists — `docs/ARCHITECTURE.md` §1. Emulation is the only valid path; the impossible requirement is documented, not claimed.)*
- [x] Evaluate existing Rust emulators and virtualization engines. *(riscv-rust MIT/unmaintained-2023, rvemu MIT/Linux-incomplete, RVVM C/GPL-3, v86 JS/BSD 32-bit only, qemu-wasm experimental C, CheerpX proprietary — table in `docs/ARCHITECTURE.md` §2, queried live via GitHub API 2026-10-31.)*
- [x] Select the guest CPU architecture. *(**RISC-V 64** — QEMU-`virt`-compatible machine implemented in Rust.)*
- [x] Verify Linux kernel compatibility. *(Upstream riscv64 is first-class; latest stable **7.3.1** resolved from kernel.org `releases.json` 2026-10-31. Actual boot = Phase 5, still open.)*
- [x] Verify Debian compatibility. *(Latest stable **Debian 13 trixie** is the first with official riscv64; Debian 13 dropped i386 kernel, which is what disqualifies v86. Actual boot = Phase 6, still open.)*
- [x] Document architectural decisions. *(`docs/ARCHITECTURE.md`.)*
- [x] Document technical limitations. *(`docs/ARCHITECTURE.md` §5: no F/D yet → Debian lp64d blocked, Sv39-only, 1 hart, interpreter, no raw browser networking, WebGPU ≠ guest GPU passthrough, evictable storage.)*
- [x] Define acceptance criteria. *(Mission Definition of Done, mirrored at the bottom of this file.)*
- [x] Audit dev-environment constraints. *(Sandbox: no Rust toolchain, no kernel.org/debian.org network; GitHub/npm/PyPI only. Consequence: Rust verified on GitHub Actions, TypeScript locally, guest builds in dispatch CI — recorded in `docs/TEST-REPORT.md`.)*

## Phase 1 — Project Foundation
- [x] Establish the Rust workspace. *(crates/wasmnux-core + crates/wasmnux-wasm; CI fmt/clippy/-D warnings/cargo test 40/40 green — TEST-REPORT T-05…T-07.)*
- [x] Establish the TypeScript application. *(web/ strict tsc + ESLint + vitest 28/28 + production build green locally and in CI — T-01…T-04.)*
- [x] Configure the WASM compilation target. *(wasm32-unknown-unknown release build + wasm-bindgen 0.2.130 package generated in CI — T-08/T-09.)*
- [x] Establish strict linting and formatting. *(rustfmt, clippy -D warnings, tsc strict, ESLint-less strict config + prettier-check via tsc/vitest gate — see CI.)*
- [x] Configure automated testing. *(cargo test 40, vitest 28, Playwright e2e suite — all executing in CI.)*
- [x] Establish CI. *(ci.yml: fmt/rust/web/e2e jobs green; guest.yml builds guest images (dispatch + trigger file).)*
- [x] Implement browser capability detection. *(Unit-tested and exercised in real Chromium by e2e.)*
- [ ] Establish reproducible dependency policy. *(package-lock committed + wasm-bindgen exact-pinned; Cargo.lock not yet committed (no local cargo) — policy doc in docs/DEPENDENCIES.md; lockfile commit remains.)*

## Phase 2 — Virtualization Core
- [ ] Implement or integrate the selected VM engine. *(Own Rust RV64 machine in crates/wasmnux-core.)*
- [x] Implement guest CPU execution. *(RV64 IMAC+Zicsr/Zifencei, M/S/U; per-class unit tests green. No F/D yet — see next item.)*
- [ ] Implement F/D floating-point extensions. *(Required for Debian lp64d userspace; hard-fp CSR state, NaN-boxing, riscv-tests fp suites.)*
- [x] Implement guest memory. *(RAM with bounds-checked LE accessors; memory-map per docs/ARCHITECTURE.md §2.)*
- [x] Implement MMU. *(Sv39 unit-verified incl. WARL rejection of Sv48; Sv48 still TODO under validation item.)*
- [x] Implement interrupt handling. *(PLIC/CLINT/delegation unit-verified; in-guest validation pending kernel.)*
- [x] Implement virtual clocks and timers. *(CLINT mtime/mtimecmp driven by executed cycles; SBI TIME.)*
- [ ] Implement the guest boot sequence. *(Loader/DTB/reset path unit-verified incl. end-to-end tiny S-mode guest→SBI print→poweroff; unchecked until the real OpenSBI→kernel path runs.)*
- [ ] Implement firmware path. *(fw_jump ELF loading + labelled built-in SBI fallback implemented & unit-tested; real OpenSBI binary boot pending guest CI.)*
- [x] Implement virtual devices. *(UART + VirtIO-MMIO block unit-verified; virtio-net/gpu in later phases.)*
- [x] Implement virtual block storage. *(BlockBackend trait; sector read/write/flush; descriptor-chain queue processing.)*
- [x] Implement a serial console. *(UART ↔ terminal byte queues + IRQ.)*
- [x] Implement framebuffer device. *(simple-framebuffer carve-out + DTB; guest-driven validation pending kernel.)*
- [x] Implement lifecycle management. *(Created→Ready→Running→Paused→Halted transitions, error on illegal transition.)*
- [ ] Validate against riscv-tests / official compliance vectors.
- [ ] Validate the complete VM execution path. *(Boot OpenSBI→kernel on native test harness before any browser claim.)*

## Phase 3 — WASM Runtime
- [x] Compile the runtime to WASM. *(CI release build.)*
- [x] Implement Rust/TypeScript bindings. *(wasm-bindgen Machine; e2e proves instantiation in Chromium.)*
- [x] Implement initialization and shutdown. *(Machine lifecycle API + worker handshake.)*
- [x] Implement Web Worker integration. *(Validated protocol; e2e drives the real worker.)*
- [ ] Implement memory management. *(Bounded RAM config; WASM memory accounting surfaced to UI.)*
- [x] Implement runtime error handling.
- [ ] Implement threaded execution where permitted. *(SharedArrayBuffer detection; single-threaded fallback is the default.)*
- [ ] Validate browser execution. *(Module instantiates in real Chromium via Playwright ✓; RV64-program-in-browser assertion lands with the first bootable guest — remains open.)*

## Phase 4 — Persistent Virtual Storage
- [x] Implement the storage abstraction.
- [x] Implement OPFS integration. *(e2e: storeKind=opfs in Chromium; persistence across reload verified.)*
- [x] Implement IndexedDB metadata. *(Fallback backend implemented; OPFS is the exercised path in e2e.)*
- [x] Implement persistent disk management.
- [x] Implement crash-consistent updates. *(Manifest-flip design + unflushed-write crash test.)*
- [x] Implement copy-on-write snapshots per profile. *(Unit + e2e restore/isolation/reset.)*
- [ ] Implement quota handling. *(estimate/persist surfaced in UI ✓; graceful QuotaExceeded *behaviour* not yet test-proven — open.)*
- [x] Implement backup and restoration.
- [x] Implement import/export. *(Validation unit-tested.)*
- [ ] Implement cross-tab disk locking. *(Web Locks API around open/flush; today two tabs share a profile disk unsafely — same-tab operations are serialised inside SparseDisk.)*
- [ ] Implement async block bridge. *(Worker ChunkStore directly behind VirtIO, replacing v1 whole-image load/export.)*
- [ ] Test complete browser restart persistence. *(Page-reload persistence PROVEN in e2e ✓; full browser-process restart evidence still required.)*

## Phase 5 — Vanilla Linux
- [ ] Retrieve and verify the latest stable upstream Linux kernel. *(Script resolves kernel.org releases.json + PGP signature; resolved 7.3.1 on 2026-10-31.)*
- [ ] Configure the kernel for the guest architecture. *(guest/kernel/config.fragment: virtio-mmio/blk, 8250, simplefb, ext4, initramfs.)*
- [ ] Enable the required virtual-device drivers.
- [ ] Build the kernel. *(guest.yml dispatch job; cross gcc-riscv64-linux-gnu.)*
- [x] Build BusyBox. *(riscv64 static soft-float (musl) built in guest CI; mission applet list enforced — T-11. tc applet disabled: references CBQ UAPI removed from modern headers.)*
- [x] Build the initramfs. *( `vanilla-initramfs` CI artifact ≈783 KB, newc cpio.gz — T-11.)*
- [ ] Implement `/init`. *(Mounts proc/sys/dev, persistent virtio root or data partition, execs sh on console.)*
- [ ] Implement a usable root filesystem.
- [ ] Boot the actual Linux kernel. *(Acceptance: kernel log to shell prompt in browser terminal + native harness.)*
- [ ] Validate essential BusyBox commands. *(Behaviour suite passes on the real riscv64 binary under qemu-user ✓ (T-11); in-guest (our emulator) validation waits for the kernel.)*
- [ ] Validate persistent filesystem access. *(Create file → poweroff → reboot → file present with same content/hash.)*

## Phase 6 — Debian Compatibility
- [ ] Identify the latest stable Debian release. *(13 trixie; script resolves stable suite from deb.debian.org Release file at build time.)*
- [ ] Retrieve and verify the appropriate Debian root filesystem. *(debootstrap --arch=riscv64 from official mirror, signature-checked.)*
- [ ] Configure the compatible kernel. *(Same guest kernel + Debian-required options: cgroups, namespaces, devtmpfs, ext4.)*
- [ ] Implement F/D CPU support. *(Hard blocker: Debian riscv64 = lp64d ABI. Tracked in Phase 2; Debian cannot be marked done before this.)*
- [ ] Implement Debian boot integration. *(virtio root disk, /sbin/init or documented console-init, fstab.)*
- [ ] Implement login and shell access.
- [ ] Validate BusyBox. *(busybox-static package inside Debian profile.)*
- [ ] Validate `apt`. *(apt-get update requires Phase-9 relay; local .deb install/remove validated offline.)*
- [ ] Validate `dpkg`.
- [ ] Validate package installation and removal. *(Real dpkg operations, no mocks.)*
- [ ] Validate package database persistence. *(Install → browser restart → dpkg -l still lists package.)*
- [ ] Validate Debian operation after browser restart.

## Phase 7 — WebGPU and Graphics
- [x] Implement WebGPU initialization. *(Feature-detected; headless CI takes the documented fallback path.)*
- [x] Implement the graphics presentation pipeline. *(WGSL blit implemented; pixel swizzle unit-tested; GPU-path browser validation pending real adapter.)*
- [ ] Integrate the selected virtual graphics device. *(simple-framebuffer readback API ready; guest-visible validation pending kernel.)*
- [x] Implement framebuffer updates. *(Full-frame uploads; dirty-region optimisation remains.)*
- [ ] Implement resizing.
- [ ] Implement resource cleanup.
- [ ] Implement device-loss recovery.
- [x] Implement a fallback renderer. *(Canvas2D; e2e-verified with screenshot diff in Chromium.)*
- [ ] Validate rendering on real browsers. *(Chromium fallback path ✓ via Playwright; WebGPU adapter + Firefox runs pending.)*

## Phase 8 — Browser Interface
- [x] Implement the Linux profile selector. *(Isolated profiles; e2e isolation verified.)*
- [x] Implement VM controls. *(Honest boot blocking enumerates every reason; e2e asserts no fake boot.)*
- [ ] Implement the interactive terminal. *(xterm wired to UART queues; guest-driven proof pending kernel boot.)*
- [ ] Implement keyboard handling.
- [ ] Implement clipboard integration. *(Browser-permission gated copy/paste.)*
- [ ] Implement terminal resizing.
- [x] Implement disk and memory information.
- [x] Implement boot diagnostics. *(Boot-blocker list, VM state, app log; UART pane is the raw guest stream.)*
- [x] Implement snapshot management. *(Create/restore/delete; e2e-covered.)*
- [x] Implement error reporting. *(run() wrapper + status/log rendering; no silent failures observed in e2e.)*
- [ ] Implement PWA packaging. *(Manifest + service worker written; offline-startup verification pending Phase 12.)*

## Phase 9 — Networking
- [ ] Evaluate browser networking restrictions. *(No raw TCP/UDP/DNS for pages — documented in docs/ARCHITECTURE.md §5.)*
- [ ] Select a compatible networking architecture. *(virtio-net + user-mode stack + WebSocket relay server, v86-style; relay is a deployment component, not in-page magic.)*
- [ ] Implement virtio-net device in the Rust core.
- [ ] Implement a network bridge where feasible. *(Relay protocol + reference relay; guest gets a real L2 interface.)*
- [ ] Integrate guest network interfaces.
- [ ] Validate DNS and connectivity. *(Against the relay in CI with a local target.)*
- [ ] Validate Debian repository access. *(`apt-get update` from deb.debian.org through the relay.)*
- [ ] Document network limitations and deployment requirements.

## Phase 10 — Security and Reliability
- [ ] Audit the WASM boundary. *(docs/SECURITY.md: guest fully untrusted; only virtual devices cross the boundary.)*
- [x] Audit worker communication. *(Runtime schemas reject malformed messages; unit-tested.)*
- [x] Audit disk imports. *(Size/CRC/bare-filename validation; unit-tested.)*
- [ ] Audit filesystem operations.
- [ ] Audit external dependencies. *(cargo audit + npm audit in CI; docs/DEPENDENCIES.md.)*
- [ ] Test crash recovery. *(ChunkStore-layer crash model is unit-proven; kill-tab mid-write browser test pending.)*
- [ ] Test invalid input. *(Unit level ✓ (corrupt chunk/CRC, malformed messages); browser-level fuzzing pending.)*
- [ ] Test storage exhaustion.
- [ ] Test resource cleanup. *(Repeated boot/shutdown cycles; listener + GPU resource accounting.)*
- [x] Verify isolation between Linux profiles. *(e2e: reset-vanilla keeps Debian data intact.)*

## Phase 11 — Comprehensive Testing
- [x] Complete Rust tests. *(fmt/clippy -D warnings/40 tests green in CI for the current core scope.)*
- [x] Complete TypeScript tests. *(tsc strict + ESLint + vitest 28/28.)*
- [ ] Complete WASM tests. *(CI build + Chromium instantiation ✓; in-browser guest execution pending.)*
- [ ] Complete kernel boot tests. *(Native harness: OpenSBI→7.3.x→shell; then browser.)*
- [ ] Complete BusyBox tests. *(Host + qemu-user behaviour proven (T-10/T-11); in-guest execution pending.)*
- [ ] Complete Debian tests. *(boot/login/dpkg/apt/persistence per Mission §8F.)*
- [ ] Complete storage tests. *(CRUD/snapshots/import-export/interrupted-write covered at unit + e2e level; quota + browser-restart cases open.)*
- [ ] Complete WebGPU tests. *(Enabled + disabled configurations.)*
- [x] Complete browser integration tests. *(Playwright suite green in CI for app-level scope.)*
- [ ] Complete end-to-end tests. *(Mission §8I 16-step workflow, unabridged.)*
- [x] Run regression tests. *(Full matrix green on every push; each fixed bug carries a regression test — see TEST-REPORT failure history.)*
- [ ] Produce the final evidence report. *(Maintained continuously in docs/TEST-REPORT.md; 'final' at release.)*
- [ ] Add reproducible performance benchmarks. *(Boot time, MIPS estimate, disk throughput, terminal latency — measured, never invented.)*

## Phase 12 — Release Readiness
- [ ] Verify all mandatory acceptance criteria.
- [ ] Verify reproducible builds. *(Pinned toolchains, locked deps, guest asset hashes recorded.)*
- [ ] Verify clean installation. *(Fresh clone → documented commands → running app.)*
- [ ] Verify first boot.
- [ ] Verify persistent storage.
- [ ] Verify both Linux profiles.
- [ ] Verify browser compatibility. *(Chromium + Firefox at minimum; Safari WebGPU/OPFS notes.)*
- [ ] Verify offline startup. *(PWA shell + previously cached assets; guest disks are local by design.)*
- [ ] Document known limitations.
- [ ] Produce final architecture documentation.
- [ ] Produce final testing documentation.
- [ ] Prepare the release.

## Definition of Done (Mission §11 — none may be faked)
### Core functionality
- [ ] A real Linux kernel executes inside the browser-based VM.
- [ ] The runtime executes through WebAssembly.
- [ ] The Rust core works correctly with the TypeScript application.
- [ ] The browser interface provides actual guest interaction.
- [ ] WebGPU rendering works on supported browsers.
- [ ] A functional fallback renderer exists.
### Linux compatibility
- [ ] The latest stable upstream Linux kernel boots.
- [ ] Vanilla Linux provides a functional BusyBox userspace.
- [ ] The latest stable Debian boots successfully.
- [ ] Debian provides a functional shell and package management.
- [ ] Both environments execute actual Linux commands.
- [ ] Both environments have independent filesystems.
### Persistence
- [ ] Files survive browser reloads.
- [ ] Files survive browser restarts.
- [ ] Debian package installations survive browser restarts.
- [ ] System configuration changes survive browser restarts.
- [ ] Virtual disks can be backed up and restored.
- [ ] Storage failures are handled safely.
### Quality
- [ ] All applicable automated tests pass.
- [ ] All required integration tests pass.
- [ ] Both Linux environments pass actual boot tests.
- [ ] Storage persistence tests pass.
- [ ] WebGPU tests pass on supported configurations.
- [ ] No known critical security issue remains unresolved.
- [ ] All completed TODO items have verification evidence.
- [ ] Build and setup instructions are reproducible.
