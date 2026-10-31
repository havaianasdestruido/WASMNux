# TODO — Browser-Native Linux

This is the source of truth. A checkbox is checked only after its acceptance criterion and named evidence pass. `docs/TEST_REPORT.md` records the current evidence. The diagnostic guest is **not** either deliverable profile.

## Phase 0 — Repository Audit and Feasibility
- [x] Inspect the existing repository; accept: tracked files and clean baseline recorded; test: AUDIT-01.
- [x] Identify existing infrastructure and reusable components; accept: existing code and licenses listed; test: AUDIT-01.
- [x] Investigate Linux kernel execution inside WASM; accept: distinguish emulated guest from an upstream kernel WASM target; test: ARCH-01.
- [x] Evaluate existing Rust emulators and virtualization engines; accept: candidates, licenses, maintenance, and limitations documented; test: ARCH-02.
- [x] Select a provisional guest CPU architecture; accept: rationale covers upstream Linux and Debian; test: ARCH-03.
- [x] Verify upstream Linux and Debian architecture availability, without asserting a boot; accept: primary sources recorded; test: ARCH-03.
- [x] Document architectural decisions and technical limitations; accept: `docs/ARCHITECTURE.md` covers devices, storage, security, rendering and networking; test: DOC-01.
- [x] Define acceptance criteria; accept: all phases specify testable outcomes and refer to `docs/TEST_REPORT.md`; test: DOC-01.
- [ ] Verify the selected engine boots current Linux and current Debian; accept: real browser boot logs and interactive commands; tests: KERNEL-BOOT and DEBIAN-BOOT.

## Phase 1 — Project Foundation
- [x] Establish a Rust workspace and reproducible stable toolchain; accept: clean build, tests, fmt and clippy; tests: RUST-ALL.
- [x] Establish a strict TypeScript application; accept: build and strict type check pass; tests: TS-ALL.
- [x] Configure `wasm32-unknown-unknown` compilation and checked bindings; accept: a browser instantiates the built module; tests: WASM-BROWSER.
- [x] Establish linting, formatting, unit and integration testing; accept: independent reproducible commands; tests: TS-ALL and RUST-ALL.
- [ ] Establish CI; accept: automated clean-checkout builds and all available tests; test: CI-01.
- [x] Implement browser capability and persistence detection; accept: optional features and denial displayed truthfully; tests: CAP-01 and CAP-BROWSER.
- [ ] Review and pin dependencies, transitive versions and licenses; accept: inventory and security review; test: DEP-AUDIT.

## Phase 2 — Virtualization Core
- [x] Boot the pinned historical diagnostic only; accept: real browser reaches Linux 5.4 userspace and executes a serial command; test: DIAG-BOOT.
- [x] Surface diagnostic emulator traps even if WASM cleanup throws; accept: real browser reports the trap and permits a fresh start; test: DIAG-TRAP.
- [x] Rebuild and repair the historical diagnostic conversion instructions; accept: tested rounding/flags and the unchanged real guest utility script passes; tests: DIAG-ISA and DIAG-UTILITIES.
- [ ] Integrate the selected VM engine with pinned provenance; accept: Rust/WASM executes actual RISC-V instructions; tests: VM-INST and VM-BROWSER.
- [ ] Validate guest CPU, memory, privilege/MMU, interrupts, clocks and timers; accept: real kernel reaches userspace; tests: KERNEL-BOOT and ISA.
- [ ] Validate firmware/direct boot and kernel boot protocol; accept: documented command line and inspected logs; test: KERNEL-BOOT.
- [ ] Connect a bounded persistent virtual block device; accept: guest writes reach durable backend; tests: DISK-GUEST and STORAGE-RESTART.
- [ ] Implement a serial console; accept: bidirectional bytes reach actual guest; test: CONSOLE-REAL.
- [ ] Implement lifecycle management and cleanup; accept: start, shutdown and restart leave no live worker or event handlers; test: VM-LIFECYCLE.
- [ ] Validate the complete execution path; accept: actual guest commands execute, no mocked shell; test: VM-BROWSER.

## Phase 3 — WASM Runtime
- [x] Add a checked, bounded diagnostic-only Wasm ABI; accept: invalid arguments fail and a browser runs the locally built module; tests: WASM-BOUNDARY and WASM-BROWSER.
- [ ] Compile the own runtime to WASM with stable ABI; accept: repeatable build with pinned dependencies; tests: RUST-ALL and WASM-BUILD.
- [ ] Implement checked TypeScript bindings and explicit error propagation; accept: invalid inputs and traps produce errors; test: WASM-BOUNDARY.
- [ ] Implement initialization, shutdown, suspension and restoration; accept: state transitions and resources verified; test: VM-LIFECYCLE.
- [ ] Run expensive VM work in a worker; accept: UI remains responsive and messages are validated; tests: WORKER-01 and E2E-RESPONSIVE.
- [ ] Bound guest RAM, WASM memory and I/O queues; accept: measured limits and no unbounded growth; test: VM-MEMORY.
- [ ] Validate browser execution and no-SharedArrayBuffer fallback; accept: real browser runs guest in both configurations; test: WASM-BROWSER.

## Phase 4 — Persistent Virtual Storage
- [x] Implement a sector-aligned storage abstraction (standalone prototype, not connected to a guest); accept: bounds, partial-block updates and failures verified; test: STORAGE-UNIT.
- [ ] Implement OPFS integration where supported; accept: worker-safe durable reads/writes and failures; test: STORAGE-OPFS.
- [x] Implement IndexedDB metadata and disk management (standalone prototype, not connected to a guest); accept: isolated profile disks survive reload; test: STORAGE-IDB.
- [ ] Implement crash-consistent block updates and flush ordering; accept: completed writes recover and aborted updates remain atomic; test: STORAGE-CRASH.
- [ ] Expose quota and persistence/eviction status; accept: denial and quota exhaustion actionable; test: STORAGE-QUOTA.
- [ ] Implement independent snapshots and restoration; accept: exact guest contents restored atomically; test: STORAGE-SNAPSHOT.
- [ ] Implement validated disk backup, import/export and reset; accept: corruption rejected and profiles remain isolated; test: STORAGE-IMPORT.
- [ ] Test data across full browser restart; accept: guest file contents survive actual process restart; test: STORAGE-RESTART.

## Phase 5 — Vanilla Linux
- [ ] Identify and verify current stable kernel using upstream metadata and signatures; accept: source version, signature and checksums recorded; test: KERNEL-SOURCE.
- [ ] Build kernel reproducibly for the selected architecture and required devices; accept: configuration and artifact provenance recorded; tests: KERNEL-BUILD and DEVICE-INIT.
- [ ] Build a static BusyBox and initramfs with real `/init`; accept: reproducible source and target verification; test: BUSYBOX-BUILD.
- [ ] Provide root directories, mounts, diagnostics and persistent user filesystem; accept: guest can mount and modify files; tests: BUSYBOX-FS and STORAGE-RESTART.
- [ ] Boot the actual latest stable kernel in the browser; accept: inspected serial logs and shell prompt; test: KERNEL-BOOT.
- [ ] Execute every requested BusyBox applet's behavior, including process and power controls; accept: real guest assertions; test: BUSYBOX-ALL.

## Phase 6 — Debian Compatibility
- [x] Identify current official stable Debian; accept: official release and architecture source recorded; test: ARCH-03.
- [ ] Retrieve and verify an official riscv64 Debian root filesystem and packages; accept: signed archive provenance; test: DEBIAN-SOURCE.
- [ ] Configure kernel, init, login and shell; accept: real Debian boots interactively; test: DEBIAN-BOOT.
- [ ] Verify BusyBox, utilities, `dpkg` and `apt`; accept: actual guest commands return valid results; test: DEBIAN-UTILS.
- [ ] Install, query and remove real packages through a restricted bridge; accept: real package database changes; test: DEBIAN-APT.
- [ ] Verify installed packages and offline files survive browser restart; accept: contents and package database checked; test: DEBIAN-PERSIST.
- [ ] Enforce independent configurations, disks and resets; accept: resetting one does not affect the other; test: PROFILE-ISOLATION.

## Phase 7 — WebGPU and Graphics
- [x] Implement a standalone synthetic RGBA presenter; accept: real browser pixels, resizing, fallback and device-loss recovery pass without claiming guest graphics; test: FB-PROTOTYPE.
- [ ] Select an emulated graphics device with real framebuffer output; accept: kernel driver and device tested; test: GFX-DEVICE.
- [ ] Implement WebGPU adapter/device, shader, texture updates and presentation; accept: observed guest pixels rendered; test: WEBGPU-REAL.
- [ ] Handle resize, cleanup and device loss; accept: resources reclaimed and rendering recovers; test: WEBGPU-LOSS.
- [ ] Implement Canvas 2D fallback; accept: same guest pixels visible without WebGPU; test: GFX-FALLBACK.
- [ ] Test WebGPU-enabled and disabled real browser configurations; accept: screenshots and device logs; test: GFX-BROWSERS.

## Phase 8 — Browser Interface
- [ ] Implement independent Linux profile selection and VM controls; accept: both actual guests boot, stop and reboot; test: E2E-PROFILES.
- [ ] Implement interactive guest terminal with resize, keyboard and clipboard; accept: actual console output and permission failures handled; tests: CONSOLE-REAL and CONSOLE-CLIPBOARD.
- [ ] Persist terminal settings; accept: settings survive restart; test: SETTINGS-RESTART.
- [ ] Display RAM, disk, capabilities, boot diagnostics and kernel logs; accept: values derived from runtime, not invented; test: UI-DIAGNOSTICS.
- [ ] Implement snapshot, disk backup/restore and profile reset controls; accept: guest data independently restored; test: E2E-STORAGE.
- [ ] Implement clear error and interrupted-session recovery; accept: no silent failed write; test: E2E-FAILURES.
- [ ] Add PWA installation and cached offline startup separately from guest storage; accept: offline reload after asset caching; test: PWA-OFFLINE.

## Phase 9 — Networking
- [ ] Evaluate browser networking restrictions; accept: CORS, raw socket and DNS limitations documented; test: NET-DESIGN.
- [ ] Implement a restricted, configurable bridge and guest interface; accept: explicit deployment and destination policy; test: NET-SECURITY.
- [ ] Validate DNS/connectivity and signed Debian repository access; accept: real `apt` operations; test: DEBIAN-APT.
- [ ] Verify offline Debian behavior; accept: existing packages and local files remain usable; test: DEBIAN-OFFLINE.

## Phase 10 — Security and Reliability
- [ ] Audit WASM and worker boundaries, input lengths and lifecycle; accept: malicious messages rejected; tests: SECURITY-WORKER and WASM-BOUNDARY.
- [ ] Audit disk imports and filesystem operations; accept: no path traversal or arbitrary host access; test: SECURITY-IMPORT.
- [ ] Audit direct and transitive external dependencies; accept: licenses and vulnerabilities recorded with mitigation; test: DEP-AUDIT.
- [ ] Test interrupted sessions, quota exhaustion and storage failures; accept: recoverable states and no falsely reported flush; tests: STORAGE-CRASH and STORAGE-QUOTA.
- [ ] Test resource cleanup and profile isolation; accept: no stale listeners or cross-profile data; tests: VM-LIFECYCLE and PROFILE-ISOLATION.
- [ ] Measure boot, memory, disk throughput and terminal latency; accept: reproducible procedures and actual samples; test: BENCHMARKS.

## Phase 11 — Comprehensive Testing
- [ ] Complete Rust, TypeScript and WASM test layers; accept: all required commands and browser tests pass; tests: RUST-ALL, TS-ALL, WASM-BROWSER.
- [ ] Complete actual kernel and all BusyBox behavior tests; accept: logs and applet outputs recorded; tests: KERNEL-BOOT and BUSYBOX-ALL.
- [ ] Complete Debian boot, apt install/remove and restart tests; accept: real guest evidence; tests: DEBIAN-BOOT and DEBIAN-PERSIST.
- [ ] Complete storage, WebGPU and full end-to-end browser matrix; accept: real browser outcomes recorded; tests: STORAGE-RESTART, GFX-BROWSERS, E2E-FULL.
- [ ] Run regression tests and produce final evidence report; accept: commands, environment, expected/actual and status for every requirement; test: REPORT-REVIEW.

## Phase 12 — Release Readiness
- [ ] Verify every mandatory definition-of-done item; accept: no unsupported feature checked; test: RELEASE-GATE.
- [ ] Verify reproducible builds and clean installation/first boot; accept: two clean environments agree; test: REPRO-BUILD.
- [ ] Verify persistence, both profiles and browser compatibility; accept: full real-browser matrix; test: E2E-FULL.
- [ ] Verify offline startup, known limitations and recovery guidance; accept: documented and tested; test: PWA-OFFLINE.
- [ ] Complete architecture, security and testing documentation; accept: source URLs, versions and evidence current; test: REPORT-REVIEW.
- [ ] Prepare release only after all blockers are resolved; accept: release gate passes; test: RELEASE-GATE.
