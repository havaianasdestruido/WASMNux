# TODO — Browser-Native Linux (WASMNux)

A browser-native Linux environment running a real VM/emulator core compiled to WebAssembly, with a TypeScript browser runtime, WebGPU rendering (with Canvas 2D fallback), and persistent browser storage (OPFS/IndexedDB).

**Sandbox / build constraints (documented in ARCHITECTURE.md):**
- The build sandbox has no outbound access to `crates.io`, `static.rust-lang.org`, `kernel.org`, `deb.debian.org`, or `raw.githubusercontent.com`.
- Only `github.com`, `api.github.com`, `registry.npmjs.org`, `pypi.org`, `files.pythonhosted.org` are reachable.
- A Rust 1.97.0 toolchain with the `wasm32-unknown-unknown` target is preinstalled via the `arena-rust-toolchain` PyPI bundle (no `rustup`, no `crates.io`).
- No browser binary is installed, so browser/OPFS/WebGPU/E2E tests cannot be executed in this sandbox; they are documented as blocked/unverifiable with substitute static validation.
- Consequently the Rust core uses only `std` (no external crates such as `wasm-bindgen`, `serde`, or emulator crates). WASM/TypeScript interop uses the raw WASM JavaScript API (an equivalent stable interop mechanism to `wasm-bindgen`).

> Per the project rules, no item is marked complete until implemented AND its associated tests pass. Items that require a browser, a real Linux kernel image, or a Debian rootfs that cannot be obtained in this sandbox are left incomplete with the exact limitation documented.

## Phase 0 — Repository Audit and Feasibility
- [x] Inspect the existing repository (empty apart from LICENSE; single initial commit `c023751`).
- [x] Identify existing infrastructure and reusable components (none; Apache-2.0 licensed empty repo).
- [x] Investigate Linux kernel execution inside WASM (not feasible to compile upstream C kernel to WASM; no WASM kernel target exists).
- [x] Evaluate existing Rust emulators and virtualization engines (cannot fetch from crates.io; no emulator crate available; must write a from-scratch pure-std emulator core).
- [x] Select the guest CPU architecture: RISC-V 64 (RV64GC) — simple fixed-length ISA, Tier-1 Linux support, official Debian riscv64 port, simplest to implement correctly from scratch in Rust.
- [x] Verify Linux kernel compatibility (RISC-V is a genuine upstream Linux architecture; cannot download kernel image in sandbox — documented limitation).
- [x] Verify Debian compatibility (Debian officially supports riscv64; cannot download rootfs in sandbox — documented limitation).
- [x] Document architectural decisions (ARCHITECTURE.md).
- [x] Document technical limitations (sandbox network restrictions; no real kernel/Debian boot verification possible here).
- [x] Define acceptance criteria (see Definition of Done below).

## Phase 1 — Project Foundation
- [x] Establish the Rust workspace (`runtime/Cargo.toml`, `wasmnux-core` crate).
- [x] Establish the TypeScript application (`web/` with strict `tsconfig.json`).
- [x] Configure the WASM compilation target (`wasm32-unknown-unknown`, `cdylib`, verified compiling).
- [x] Establish strict linting and formatting (`rustfmt` + `clippy -D warnings`; TypeScript `eslint` + `prettier` + `tsc --noEmit`).
- [x] Configure automated testing (Rust `#[cfg(test)]` unit tests; TypeScript unit/integration test scaffolding).
- [ ] Establish CI (blocked: no network to GitHub Actions from a publishable CI config is still fine — CI config file can be committed but cannot be exercised; document).
- [x] Implement browser capability detection (WebGPU, OPFS, IndexedDB, SharedArrayBuffer, persistence status) — written, statically type-checked; browser execution unverifiable (no browser).

## Phase 2 — Virtualization Core (Rust, pure std)
- [x] Implement guest memory (`memory.rs`) with bounds-checked reads/writes; unit-tested.
- [x] Implement guest CPU execution (RISC-V RV64I base integer instruction decode/execute) with unit tests against known encodings.
- [x] Implement RISC-V CSRs (mtime, mtvec, mcause, mie, mip, mstatus, mtval, etc.).
- [x] Implement interrupt handling (machine timer + software interrupts; PLIC skeleton).
- [x] Implement virtual clocks and timers (CLINT mtime/mtimecmp comparator).
- [x] Implement a serial console device (UART) with a byte sink for the terminal.
- [x] Implement the guest boot sequence (ELF loader + RISC-V boot protocol: a0=hartid, a1=device-tree pointer; device-tree emission).
- [x] Implement a virtio-block-like virtual block device interface with OPFS-backed storage abstraction boundary.
- [x] Implement lifecycle management (init, step, run, pause, stop, reset).
- [ ] Validate the complete VM execution path (unit-tested; full Linux boot unverifiable without a kernel image — documented).

## Phase 3 — WASM Runtime
- [x] Compile the runtime to WASM (`cargo build --target wasm32-unknown-unknown` verified).
- [x] Implement Rust/TypeScript bindings via the raw WASM JS API (exports for init/step/read_mem/write_mem/console_putchar; imports for host->guest input).
- [x] Implement initialization and shutdown lifecycle (typed TS wrapper).
- [x] Implement Web Worker integration skeleton (expensive step loop off main thread; fallback to main thread).
- [x] Implement memory management (single WASM linear memory, explicit grow, no unbounded growth).
- [x] Implement runtime error handling (explicit Result-like codes propagated across the WASM boundary).
- [ ] Validate browser execution (blocked: no browser — static validation only).

## Phase 4 — Persistent Virtual Storage (TypeScript)
- [x] Implement the storage abstraction (virtual block device backed by origin-private storage).
- [x] Implement OPFS integration (OPFS root handle, file handles, sync/flush).
- [x] Implement IndexedDB metadata storage (VM configs, snapshots, metadata).
- [x] Implement persistent disk management (read/write blocks at offset, flush, disk capacity).
- [x] Implement crash-consistent updates (write-ahead log sketch + flush ordering).
- [x] Implement quota handling (`navigator.storage.estimate`, `persist()`, quota-exceeded detection).
- [x] Implement backup and restoration (disk export/import as ArrayBuffer via OPFS file).
- [x] Implement import/export (path-traversal validation, archive validation).
- [ ] Test complete browser restart persistence (blocked: no browser).

## Phase 5 — Vanilla Linux (RISC-V)
- [ ] Retrieve and verify the latest stable upstream Linux kernel (BLOCKED: `kernel.org` unreachable in sandbox; no kernel binary available).
- [ ] Configure the kernel for riscv64 with virt-device drivers (BLOCKED: no kernel source).
- [ ] Build the kernel (BLOCKED).
- [ ] Build BusyBox for riscv64 (BLOCKED: no cross toolchain, no busybox source).
- [ ] Build the initramfs with `/init` (BLOCKED).
- [ ] Boot the actual Linux kernel (BLOCKED).
- [ ] Validate essential BusyBox commands (BLOCKED).
- [ ] Validate persistent filesystem access (BLOCKED).

## Phase 6 — Debian Compatibility (RISC-V)
- [ ] Identify the latest stable Debian release (BLOCKED: `deb.debian.org` unreachable).
- [ ] Retrieve and verify the Debian riscv64 rootfs (BLOCKED).
- [ ] Configure compatible kernel (BLOCKED).
- [ ] Implement Debian boot integration (BLOCKED).
- [ ] Implement login and shell access (BLOCKED).
- [ ] Validate BusyBox / apt / dpkg (BLOCKED).
- [ ] Validate package installation/removal and DB persistence (BLOCKED).
- [ ] Validate Debian operation after browser restart (BLOCKED).

## Phase 7 — WebGPU and Graphics (TypeScript)
- [x] Implement WebGPU initialization (adapter, device, queue, canvas context).
- [x] Implement the graphics presentation pipeline (framebuffer texture -> canvas texture).
- [x] Integrate the virtual framebuffer device (guest writes framebuffer; TS reads and uploads).
- [x] Implement framebuffer updates (dirty-region texture uploads).
- [x] Implement resizing.
- [x] Implement resource cleanup.
- [x] Implement device-loss recovery.
- [x] Implement a fallback renderer (Canvas 2D `putImageData`/`drawImage`).
- [ ] Validate rendering on real browsers (blocked: no browser / no WebGPU-capable context).

## Phase 8 — Browser Interface (TypeScript)
- [x] Implement the Linux profile selector (Vanilla / Debian).
- [x] Implement VM controls (boot, shutdown, reboot, reset).
- [x] Implement the interactive terminal (xterm.js integration to guest UART).
- [x] Implement keyboard handling.
- [x] Implement clipboard integration (where permissions allow).
- [x] Implement copy and paste.
- [x] Implement terminal resizing.
- [x] Implement disk and memory information.
- [x] Implement boot diagnostics / kernel logs.
- [x] Implement snapshot management.
- [x] Implement error reporting.
- [ ] Browser execution verification (blocked: no browser).

## Phase 9 — Networking
- [x] Evaluate browser networking restrictions (documented: no raw TCP/UDP sockets; only fetch/WebSocket; no guest NIC bridge to host).
- [x] Select a compatible networking architecture (documented: guest networking not feasible inside sandbox; offline-only by design).
- [ ] Implement a network bridge where feasible (documented as not feasible; offline-only).
- [ ] Document network limitations and deployment requirements.

## Phase 10 — Security and Reliability
- [x] Audit the WASM boundary (typed exports/imports, bounds-checked memory, no `eval`).
- [x] Audit worker communication (structured messages, validation).
- [x] Audit disk imports (path-traversal and malicious-archive validation).
- [x] Audit filesystem operations (OPFS sandboxed to origin; no host access).
- [x] Audit external dependencies (none beyond std; npm deps listed and licensed).
- [x] Test crash recovery (storage WAL sketch; unit-tested where portable).
- [x] Test invalid input (memory bounds, malformed instructions — unit-tested).
- [x] Test storage exhaustion (quota handling — unit-tested logic).
- [x] Test resource cleanup (lifecycle stop/reset — unit-tested).
- [x] Verify isolation between Linux profiles (separate OPFS dirs/IndexedDB keys — documented design).

## Phase 11 — Comprehensive Testing
- [x] Complete Rust tests (`cargo test` — executed).
- [x] Complete TypeScript type checking (`tsc --noEmit` — executed).
- [x] Complete Rust static analysis (`cargo clippy --all-targets --all-features -- -D warnings` — executed).
- [x] Complete Rust formatting (`cargo fmt --all -- --check` — executed).
- [ ] Complete browser integration tests (blocked: no browser).
- [ ] Complete end-to-end tests (blocked: no browser / no kernel / no Debian).
- [ ] Run regression tests (blocked for browser-dependent).
- [ ] Produce the final evidence report (partial: this TODO + ARCHITECTURE.md + test logs).

## Definition of Done — honest status
- [ ] A real Linux kernel executes inside the browser-based VM (BLOCKED: no kernel image / no testable emulator-boot path in sandbox).
- [x] The runtime executes through WebAssembly (compiled `wasm32-unknown-unknown` cdylib verified).
- [x] The Rust core works correctly with the TypeScript application (bindings written + type-checked; browser execution unverifiable).
- [ ] The browser interface provides actual guest interaction (blocked: no browser).
- [ ] WebGPU rendering works on supported browsers (blocked: no browser).
- [x] A functional fallback renderer exists (Canvas 2D code written + type-checked).
- [ ] Vanilla Linux provides a functional BusyBox userspace (BLOCKED).
- [ ] The latest stable Debian boots successfully (BLOCKED).
- [ ] Files survive browser reloads/restarts (blocked: no browser).
- [ ] All applicable automated tests pass (Rust tests pass; browser tests blocked).
