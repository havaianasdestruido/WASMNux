# TODO — Browser-Native Linux

> Source of truth for project status. An item is marked `[x]` **only** after
> its implementation *and* associated tests pass, with evidence in
> `docs/TEST-REPORT.md`. Incomplete items are never deleted to look finished.
> Architecture rationale: `docs/ARCHITECTURE.md`.
> Status at a glance: **Phase 0 complete · Phase 1 in progress · Phases 2–12 open.**

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
- [ ] Establish the Rust workspace. *(crates/wasmnux-core + crates/wasmnux-wasm — code written; checked when CI cargo fmt/clippy/test passes.)*
- [ ] Establish the TypeScript application. *(web/ with strict tsconfig — checked when tsc + vitest + build pass.)*
- [ ] Configure the WASM compilation target. *(wasm32-unknown-unknown + wasm-bindgen — checked when CI wasm build passes.)*
- [ ] Establish strict linting and formatting. *(rustfmt, clippy -D warnings, tsc strict, ESLint-less strict config + prettier-check via tsc/vitest gate — see CI.)*
- [ ] Configure automated testing. *(cargo test, vitest, Playwright config.)*
- [ ] Establish CI. *(.github/workflows/ci.yml on push; guest.yml manual dispatch.)*
- [ ] Implement browser capability detection. *(WASM, WebGPU, OPFS, IndexedDB, SharedArrayBuffer, storage persistence — web/src/capabilities.ts.)*
- [ ] Establish reproducible dependency policy. *(Cargo.lock + package-lock committed; every dependency justified in docs/DEPENDENCIES.md.)*

## Phase 2 — Virtualization Core
- [ ] Implement or integrate the selected VM engine. *(Own Rust RV64 machine in crates/wasmnux-core.)*
- [ ] Implement guest CPU execution. *(RV64I + M + A + C-subset + Zicsr/Zifencei; hand-assembled unit tests per instruction class.)*
- [ ] Implement F/D floating-point extensions. *(Required for Debian lp64d userspace; hard-fp CSR state, NaN-boxing, riscv-tests fp suites.)*
- [ ] Implement guest memory. *(RAM with bounds-checked LE accessors; memory-map per docs/ARCHITECTURE.md §2.)*
- [ ] Implement MMU. *(Sv39 walk, MXR/SUM, page-fault causes 12/13/15; Sv48 later.)*
- [ ] Implement interrupt handling. *(PLIC claim/complete, CLINT, M/S delegation via mideleg/medeleg.)*
- [ ] Implement virtual clocks and timers. *(CLINT mtime/mtimecmp driven by executed cycles; SBI TIME.)*
- [ ] Implement the guest boot sequence. *(Reset vector, a0/a1 convention, kernel Image + ELF loaders, initrd, generated DTB.)*
- [ ] Implement firmware path. *(OpenSBI fw_jump production path + built-in minimal SBI shim for development.)*
- [ ] Implement virtual devices. *(NS16550A UART, VirtIO-MMIO block; virtio-net + virtio-gpu later phases.)*
- [ ] Implement virtual block storage. *(BlockBackend trait; sector read/write/flush; descriptor-chain queue processing.)*
- [ ] Implement a serial console. *(UART ↔ terminal byte queues + IRQ.)*
- [ ] Implement framebuffer device. *(simple-framebuffer RAM carve-out + DTB node; host-side readback API.)*
- [ ] Implement lifecycle management. *(Created→Ready→Running→Paused→Halted transitions, error on illegal transition.)*
- [ ] Validate against riscv-tests / official compliance vectors.
- [ ] Validate the complete VM execution path. *(Boot OpenSBI→kernel on native test harness before any browser claim.)*

## Phase 3 — WASM Runtime
- [ ] Compile the runtime to WASM. *(wasm32-unknown-unknown in CI.)*
- [ ] Implement Rust/TypeScript bindings. *(crates/wasmnux-wasm via wasm-bindgen.)*
- [ ] Implement initialization and shutdown.
- [ ] Implement Web Worker integration. *(web/src/worker.ts + typed message protocol with runtime validation.)*
- [ ] Implement memory management. *(Bounded RAM config; WASM memory accounting surfaced to UI.)*
- [ ] Implement runtime error handling. *(VmError → typed JS errors; no silent traps.)*
- [ ] Implement threaded execution where permitted. *(SharedArrayBuffer detection; single-threaded fallback is the default.)*
- [ ] Validate browser execution. *(Playwright in CI: instantiate module, run RV64 test program, assert UART output.)*

## Phase 4 — Persistent Virtual Storage
- [ ] Implement the storage abstraction. *(ChunkStore interface: OPFS, IndexedDB, in-memory backends.)*
- [ ] Implement OPFS integration.
- [ ] Implement IndexedDB metadata.
- [ ] Implement persistent disk management. *(SparseDisk: 64 KiB chunks, CRC-32, dirty tracking, configurable capacity.)*
- [ ] Implement crash-consistent updates. *(Chunk write + manifest generation flip; CRC verification on load.)*
- [ ] Implement copy-on-write snapshots per profile.
- [ ] Implement quota handling. *(navigator.storage.estimate/persist surfaced in UI; graceful QuotaExceeded path.)*
- [ ] Implement backup and restoration. *(Whole-disk export/import with header + per-chunk CRC validation.)*
- [ ] Implement import/export. *(Binary format validated against path traversal / oversize / corrupt input.)*
- [ ] Implement async block bridge. *(Worker ChunkStore directly behind VirtIO, replacing v1 whole-image load/export.)*
- [ ] Test complete browser restart persistence. *(Playwright: write via guest or disk API → reload → context restart → verify bytes.)*

## Phase 5 — Vanilla Linux
- [ ] Retrieve and verify the latest stable upstream Linux kernel. *(Script resolves kernel.org releases.json + PGP signature; resolved 7.3.1 on 2026-10-31.)*
- [ ] Configure the kernel for the guest architecture. *(guest/kernel/config.fragment: virtio-mmio/blk, 8250, simplefb, ext4, initramfs.)*
- [ ] Enable the required virtual-device drivers.
- [ ] Build the kernel. *(guest.yml dispatch job; cross gcc-riscv64-linux-gnu.)*
- [ ] Build BusyBox. *(riscv64, static, soft-float until Phase-2 F/D lands; applet list enforced by script.)*
- [ ] Build the initramfs. *(guest/scripts/build-initramfs.sh, newc cpio + gzip.)*
- [ ] Implement `/init`. *(Mounts proc/sys/dev, persistent virtio root or data partition, execs sh on console.)*
- [ ] Implement a usable root filesystem.
- [ ] Boot the actual Linux kernel. *(Acceptance: kernel log to shell prompt in browser terminal + native harness.)*
- [ ] Validate essential BusyBox commands. *(guest/tests/busybox-applets.sh executes behaviour tests for every Mission-§3 applet inside the guest.)*
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
- [ ] Implement WebGPU initialization. *(Adapter/device request with feature detection and error surface.)*
- [ ] Implement the graphics presentation pipeline. *(WGSL blit: BGRA/RGB565 conversion + scaling, storage-texture upload.)*
- [ ] Integrate the selected virtual graphics device. *(simple-framebuffer readback; virtio-gpu stretch.)*
- [ ] Implement framebuffer updates. *(Dirty detection; partial texture uploads.)*
- [ ] Implement resizing.
- [ ] Implement resource cleanup.
- [ ] Implement device-loss recovery.
- [ ] Implement a fallback renderer. *(Canvas 2D ImageData path, Linux fully usable without WebGPU.)*
- [ ] Validate rendering on real browsers. *(Playwright Chromium: renderer pattern + guest framebuffer; WebGPU-disabled run.)*

## Phase 8 — Browser Interface
- [ ] Implement the Linux profile selector. *(Vanilla / Debian, isolated state.)*
- [ ] Implement VM controls. *(Boot/pause/reboot/poweroff/reset-profile; no fake boot when guest assets are absent — the UI must say what is missing and how to build/import it.)*
- [ ] Implement the interactive terminal. *(xterm.js wired to guest UART byte stream only.)*
- [ ] Implement keyboard handling.
- [ ] Implement clipboard integration. *(Browser-permission gated copy/paste.)*
- [ ] Implement terminal resizing.
- [ ] Implement disk and memory information. *(storage.estimate + profile disk usage + RAM config.)*
- [ ] Implement boot diagnostics. *(Kernel log pane = raw UART stream; VM state + last error.)*
- [ ] Implement snapshot management.
- [ ] Implement error reporting. *(No unhandled rejections; errors rendered with remediation.)*
- [ ] Implement PWA packaging. *(Manifest + service worker caching the app shell only, never guest disks.)*

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
- [ ] Audit worker communication. *(Runtime-validated message schemas.)*
- [ ] Audit disk imports. *(Size caps, CRC, no filesystem path use of guest-controlled strings.)*
- [ ] Audit filesystem operations.
- [ ] Audit external dependencies. *(cargo audit + npm audit in CI; docs/DEPENDENCIES.md.)*
- [ ] Test crash recovery. *(Kill tab mid-write → CRC-checked manifest recovers last consistent generation.)*
- [ ] Test invalid input. *(Corrupt images, oversize imports, malformed worker messages.)*
- [ ] Test storage exhaustion.
- [ ] Test resource cleanup. *(Repeated boot/shutdown cycles; listener + GPU resource accounting.)*
- [ ] Verify isolation between Linux profiles. *(Separate OPFS paths/IDB keys; reset-one-keeps-other test.)*

## Phase 11 — Comprehensive Testing
- [ ] Complete Rust tests. *(cargo fmt --check, clippy -D warnings, cargo test — CI.)*
- [ ] Complete TypeScript tests. *(tsc strict, vitest — local + CI.)*
- [ ] Complete WASM tests. *(CI build + Playwright instantiation test.)*
- [ ] Complete kernel boot tests. *(Native harness: OpenSBI→7.3.x→shell; then browser.)*
- [ ] Complete BusyBox tests. *(Every Mission-§3 applet behaviour-tested in-guest.)*
- [ ] Complete Debian tests. *(boot/login/dpkg/apt/persistence per Mission §8F.)*
- [ ] Complete storage tests. *(CRUD, snapshots, import/export, quota, interrupted-write recovery.)*
- [ ] Complete WebGPU tests. *(Enabled + disabled configurations.)*
- [ ] Complete browser integration tests. *(Playwright.)*
- [ ] Complete end-to-end tests. *(Mission §8I 16-step workflow, unabridged.)*
- [ ] Run regression tests.
- [ ] Produce the final evidence report. *(docs/TEST-REPORT.md, maintained continuously.)*
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
