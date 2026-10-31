# TODO — Browser-Native Linux

**Status: in development; not release-ready.** This is the authoritative backlog.
A checked item requires executed verification evidence in `docs/TEST_REPORT.md`.
Compilation, upstream demonstrations, and mocked tests do not establish browser integration.
Do not delete incomplete items to improve apparent progress.

## Phase 0 — Repository Audit and Feasibility
- [x] **P0.1 Inspect the repository and preserve existing work.** Acceptance: starting commit, branch, files, license, and available tools recorded. Tests: inspect Git status/log and tool versions; evidence F0.1.
- [x] **P0.2 Establish the no-mock acceptance contract.** Acceptance: real kernel, real userspace, guest-owned disks, and explicit unverified states defined. Tests: review architecture against every mandatory requirement; evidence F0.2.
- [x] **P0.3 Investigate a native Linux WASM architecture.** Acceptance: distinguish experimental kernel forks from supported upstream targets. Tests: enumerate the genuine stable kernel's `arch/` directories and record findings; evidence F0.3.
- [x] **P0.4 Evaluate existing emulators before writing a CPU.** Acceptance: v86, riscv-rust, rsemu, and TinyEMU-family options compared with source/license/maintenance evidence. Tests: inspect source and published compatibility; document why a candidate is selected, not claimed browser-tested; evidence F0.4.
- [x] **P0.5 Select the guest architecture.** Acceptance: RV64GC, Sv39, single hart, little-endian; explain why current Debian rules out a standalone i386 guest. Tests: cross-check Debian's official architecture list against the candidate board; evidence F0.5.
- [x] **P0.6 Resolve upstream versions from current sources.** Acceptance: distinguish mainline, stable maintenance, and Debian stable point release; record dates and source revisions. Tests: inspect official release metadata and actual Git tags; evidence F0.6.
- [x] **P0.7 Publish the architecture decision and limitations.** Acceptance: CPU/MMU, interrupts, clocks, firmware, console, block, graphics, networking, persistence, and security boundaries all addressed. Tests: architecture completeness review; evidence F0.7.
- [x] **P0.8 Prove the selected emulator can execute the selected kernel and Debian.** Acceptance: actual serial boot logs, not upstream claims alone. Tests: native reference boot followed by WASM/browser boot; latest kernel and Debian 13 riscv64 tested separately.

## Phase 1 — Project Foundation
- [x] **P1.1 Establish a minimal Rust workspace.** Acceptance: platform-independent runtime crate and WASM adapter with explicit dependency ownership. Tests: fmt, check, unit tests, clippy with warnings denied.
- [x] **P1.2 Establish strict TypeScript tooling.** Acceptance: strict/no-unchecked-index/no-explicit-any configuration, reproducible npm lockfile, no runtime CDN dependency. Tests: typecheck, ESLint, formatting, unit tests.
- [x] **P1.3 Configure browser WASM compilation.** Acceptance: wasm32-unknown-unknown module with a documented stable ABI and generated or checked TypeScript declarations. Tests: inspect WASM exports/imports; instantiate in Node and a browser worker.
- [ ] **P1.4 Add capability detection.** Acceptance: WASM, module workers, OPFS synchronous handles, persistence API, WebGPU, cross-origin isolation reported independently. Tests: supported, absent, throwing, and denied capability fixtures; real Chromium probes.
- [ ] **P1.5 Add CI quality gates.** Acceptance: Rust/TS/WASM/build-provenance checks run without claiming unavailable guest/browser tests passed. Tests: execute the same scripts locally and run the workflow on GitHub.
- [ ] **P1.6 Pin development tools and audit dependencies.** Acceptance: versions, source URLs, hashes, licenses, and necessity recorded; tool installation avoids unverified binaries. Tests: lockfile/provenance checks and native/WASM dependency builds; security audit when tools are available.

## Phase 2 — Virtualization Core
- [x] **P2.1 Integrate a pinned Rust RV64 emulator.** Acceptance: upstream MIT notices preserved and all local patches documented; no replacement syscall shell. Tests: upstream instruction tests and real kernel execution.
- [ ] **P2.2 Bound and validate guest RAM.** Acceptance: checked allocation sizes, no host-address exposure, bounded image placement, no memory growth during stepping. Tests: zero/unaligned/excessive sizes, image boundary faults, repeated lifecycle allocation.
- [ ] **P2.3 Validate CPU privilege and address translation.** Acceptance: M/S/U, RV64GC, Sv39, atomics, fences, and trap delegation run genuine supervisor code. Tests: arithmetic/CSR/atomic/MMU tests and Linux page-fault handling.
- [x] **P2.4 Integrate PLIC, CLINT, and clocks.** Acceptance: interrupt claim/completion, software/timer/external interrupts, monotonic guest time, bounded scheduling slices. Tests: register-level tests and Linux timer/console interrupts.
- [x] **P2.5 Implement the OpenSBI boot contract.** Acceptance: firmware at 0x80000000, kernel at 0x80200000, validated initrd/DTB placement and boot-hart registers. Tests: malformed/truncated/overlapping images rejected; collect OpenSBI and kernel logs.
- [x] **P2.6 Connect NS16550 input/output.** Acceptance: byte-preserving serial console, bounded queues, backpressure, no dropped committed input. Tests: register/IRQ tests plus interactive guest shell.
- [x] **P2.7 Implement VirtIO block requests.** Acceptance: bounded descriptor chains, read/write/flush, read-only protection, sector bounds, explicit error completion. Tests: malformed chains and real filesystem reads/writes/flushes; never acknowledge a failed durable write as successful.
- [ ] **P2.8 Implement power and reset devices.** Acceptance: guest poweroff/reboot distinguished and surfaced to lifecycle. Tests: actual `poweroff` and `reboot`, no dangling worker/timers.
- [ ] **P2.9 Support deterministic suspension and restoration.** Acceptance: CPU, RAM, devices, clock epoch, profile, and disk generation saved as one consistent identity. Tests: reject mismatched/truncated snapshots and resume a real guest with unchanged disk state.
- [x] **P2.10 Validate the complete execution path.** Acceptance: actual kernel reaches a real shell through every virtual device needed for boot. Tests: native reference and browser runs with raw serial artifacts.

## Phase 3 — WASM Runtime
- [x] **P3.1 Compile the real execution core to WASM.** Acceptance: interpreter-only fallback, no native JIT or host filesystem access in browser module. Tests: WASM validation and execution of a real guest, not only an exported counter.
- [ ] **P3.2 Bind the Rust ABI to strict TypeScript.** Acceptance: explicit ownership, byte lengths, UTF-8 handling, error codes, no unsafe arbitrary pointer API exposed to the UI. Tests: valid/invalid lengths, error propagation, disposed handles.
- [x] **P3.3 Run emulation in a dedicated module worker.** Acceptance: UI remains responsive; bounded execution slices and output batches; no SharedArrayBuffer requirement. Tests: worker messages, cancellation, heartbeat, terminal responsiveness under load.
- [ ] **P3.4 Enforce lifecycle transitions.** Acceptance: initialize/boot/run/pause/shutdown/fault/dispose are explicit and idempotent where appropriate. Tests: every legal transition and rejected illegal transitions, including repeated disposal.
- [ ] **P3.5 Recover from runtime failures.** Acceptance: WASM traps, worker exit, storage failure, and device loss produce visible terminal/error states without false boot success. Tests: fault injection and recovery in real browser workers.
- [ ] **P3.6 Measure WASM memory and execution overhead.** Acceptance: bounded heap/RAM, no fabricated metrics. Tests: repeated boot/shutdown and long-run memory/throughput measurements with environment recorded.

## Phase 4 — Persistent Virtual Storage
- [x] **P4.1 Define a block backend independent of browser APIs.** Acceptance: fixed sector size/capacity, explicit flush semantics, injectable errors. Tests: Rust bounds/error/serialization tests.
- [x] **P4.2 Implement OPFS disk I/O.** Acceptance: worker-only synchronous handles for guest hot-path I/O, separate profile namespaces, bounded buffers. Tests: real OPFS read/write/truncate/flush/reopen, not an in-memory mock.
- [ ] **P4.3 Store validated metadata in IndexedDB.** Acceptance: schema version, disk generation, profile identity, settings, snapshot index; transactions reject mismatched profiles. Tests: malformed metadata, migration, concurrent tabs and lease conflicts.
- [ ] **P4.4 Implement crash-consistent disk updates.** Acceptance: documented write-ahead/copy-on-write protocol, commit markers, checksums, recovery before guest writes. Tests: inject failures at every transaction boundary and verify restored guest filesystem contents.
- [ ] **P4.5 Expose persistence permission and quota state.** Acceptance: request persistent storage only on user action; distinguish granted/denied/unavailable; never promise permanence. Tests: granted/denied APIs and actual quota pressure.
- [ ] **P4.6 Implement disk formatting and capacity management.** Acceptance: real guest filesystem on a bounded virtual block device, explicit initialization/reset; no silent overwrite. Tests: mount, write, fill, reset one profile while preserving the other.
- [ ] **P4.7 Implement offline snapshots.** Acceptance: paused CPU/device/RAM state atomically associated with a disk generation, independent per profile. Tests: create/restore/delete and reject snapshot/disk mismatch.
- [ ] **P4.8 Implement validated import/export.** Acceptance: streaming raw images and versioned manifests, size/hash validation, no archive path extraction or host filesystem bridge. Tests: round-trip real disks, truncation, wrong hashes, malicious names/metadata.
- [ ] **P4.9 Prove persistence beyond a live tab.** Acceptance: Linux-created files/configurations survive reload and a fresh browser process. Tests: Playwright persistent contexts, close/relaunch, inspect contents from a second real guest boot.
- [x] **P4.10 Document browser eviction and backup limitations.** Acceptance: UI and docs distinguish durable-from-refresh from guaranteed permanent storage. Tests: review capability/error messages and backup restore procedure.

## Phase 5 — Vanilla Linux
- [ ] **P5.1 Acquire and verify the current stable kernel.** Acceptance: exact official tag/source archive hash recorded, no kernel fork or fabricated WASM target. Tests: source provenance checks and signed-tag verification where keys are available.
- [x] **P5.2 Publish a reproducible RV64 kernel configuration.** Acceptance: required serial/VirtIO/block/filesystem/graphics/input options built in; unsupported extensions not advertised. Tests: inspect expanded config and kernel Image metadata.
- [x] **P5.3 Build the genuine upstream kernel.** Acceptance: successful unmodified-source build for the pinned guest board. Tests: clean build and inspect kernel version/architecture in a boot log.
- [ ] **P5.4 Build a real static RV64 BusyBox.** Acceptance: required applets compiled for the guest ABI; license/source/config preserved. Tests: ELF architecture/linking inspection and execution of every required applet's behavior.
- [x] **P5.5 Build the initramfs and `/init`.** Acceptance: real proc/sys/dev/tmp mounts, persistent root/home discovery, console shell, controlled shutdown. Tests: cpio validation and actual boot; failure to mount persistence must not silently become a persistent session.
- [ ] **P5.6 Validate all required utilities behaviorally.** Acceptance: sh/ash, file operations, mounts, process management, diagnostics, text tools, archives, permissions, time, reboot/poweroff all pass. Tests: scripted assertions on outputs and filesystem/process side effects in the actual guest.
- [x] **P5.7 Boot Vanilla in the browser.** Acceptance: real kernel/BusyBox interaction through worker/WASM and OPFS-backed disk. Tests: browser E2E boot, commands, file mutation, reboot, reload, content verification.

## Phase 6 — Debian Compatibility
- [x] **P6.1 Pin official Debian stable riscv64 userspace.** Acceptance: latest stable release/point version and immutable artifact digest; reject older i386 substitution. Tests: verify signed repository metadata/artifact digest and inspect os-release/dpkg architecture.
- [x] **P6.2 Build the Debian virtual disk reproducibly.** Acceptance: genuine root filesystem, real `/sbin/init` or documented console init, configured login environment, BusyBox installed. Tests: image construction checks and native guest boot.
- [x] **P6.3 Verify kernel/userspace ABI compatibility.** Acceptance: Debian dynamically linked programs run on the selected kernel without a syscall translation layer. Tests: shell, coreutils, dpkg and apt execute in the guest.
- [ ] **P6.4 Validate login and diagnostics.** Acceptance: configured access, no hidden host execution, uname/ps/df/free and BusyBox work. Tests: serial login and behavioral command assertions.
- [ ] **P6.5 Establish a browser-compatible package network.** Acceptance: authenticated HTTPS repository access through an explicitly deployed constrained bridge, or accurately blocked package installation. Tests: DNS/TCP/TLS, signed apt metadata, real install/remove/query; no fabricated apt output.
- [x] **P6.6 Prove package persistence.** Acceptance: package files and dpkg/apt databases survive a clean shutdown and new browser process. Tests: install, remove, query, reboot/reload and verify actual package state.
- [ ] **P6.7 Validate offline Debian behavior.** Acceptance: shell and installed packages work without network; uncached installations report a real network error. Tests: offline browser context and locally available package installation.
- [ ] **P6.8 Prove profile isolation.** Acceptance: separate assets, configuration, disks, leases and snapshots; resetting Vanilla never changes Debian. Tests: cross-profile mutation/reset/restore assertions.

## Phase 7 — WebGPU and Graphics
- [x] **P7.1 Integrate a real virtual graphics device.** Acceptance: guest scanout/resource lifecycle connected to browser frames; serial terminal does not masquerade as GPU output. Tests: guest DRM initialization and known framebuffer pattern.
- [ ] **P7.2 Implement WebGPU device and pipeline creation.** Acceptance: adapter/device/shader/pipeline/texture/sampler owned and released explicitly. Tests: real adapter acquisition, shader compilation, device errors.
- [x] **P7.3 Upload and render guest framebuffer updates.** Acceptance: real guest pixels converted/uploaded efficiently and presented with correct aspect ratio. Tests: pixel readback and resize/update assertions on actual guest frames.
- [ ] **P7.4 Connect keyboard and pointer input.** Acceptance: browser events reach guest virtual input devices with bounded queues and focus handling. Tests: real guest input events, modifier/release handling and cleanup.
- [ ] **P7.5 Implement Canvas 2D fallback.** Acceptance: the same guest framebuffer remains usable without WebGPU. Tests: disabled/throwing WebGPU browser configurations and matching output.
- [ ] **P7.6 Recover from device loss.** Acceptance: resources invalidated and renderer recreated without restarting or corrupting the guest. Tests: destroy device, resize/reinitialize, verify subsequent frames.
- [ ] **P7.7 Bound graphics resources and measure overhead.** Acceptance: fixed-size textures/queues, no per-frame device allocation or unbounded frame copies. Tests: long-run frame/resource accounting and real performance measurements.

## Phase 8 — Browser Interface
- [ ] **P8.1 Implement independent Linux profile selection.** Acceptance: clear availability/boot state and no profile switching while another VM owns its disk. Tests: UI/E2E profile isolation and invalid state transitions.
- [ ] **P8.2 Implement boot, shutdown, reboot, pause and resume.** Acceptance: controls operate on the actual VM, expose progress/errors, and distinguish guest poweroff from worker disposal. Tests: lifecycle E2E with serial evidence.
- [ ] **P8.3 Integrate a real interactive terminal.** Acceptance: xterm-compatible guest byte stream, no TypeScript command interpreter, resize signal delivered to guest. Tests: interactive shell, control keys, UTF-8, resize and long output.
- [ ] **P8.4 Implement clipboard and persistent terminal settings.** Acceptance: permission-aware copy/paste and validated font/theme settings scoped independently of guest disks. Tests: permission granted/denied and reload behavior.
- [ ] **P8.5 Expose memory, disk, boot and kernel diagnostics.** Acceptance: values come from runtime/guest/storage, unavailable values shown as unavailable. Tests: reconcile reported capacities/counters with backend and guest results.
- [ ] **P8.6 Provide snapshot and backup controls.** Acceptance: explicit destructive confirmations, progress, validation, recovery guidance and no cross-profile operations. Tests: UI-driven backup/restore and reset isolation.
- [ ] **P8.7 Provide accessible responsive controls and error recovery.** Acceptance: keyboard navigation, focus, labels, contrast, reduced motion and readable failures. Tests: accessibility checks and narrow viewport browser tests.
- [ ] **P8.8 Implement safe offline PWA startup.** Acceptance: versioned app shell separate from OS images/disks, explicit asset availability, no interception of network-bridge requests. Tests: cache then offline reload and update/rollback behavior.

## Phase 9 — Networking
- [x] **P9.1 Document browser restrictions and trust boundaries.** Acceptance: no claim of raw sockets, arbitrary DNS, ICMP or native apt connectivity from a web page. Tests: architecture review and unsupported-network behavior.
- [ ] **P9.2 Select and deploy an explicit network relay.** Acceptance: TLS WebSocket-to-TCP gateway with destination allowlist, connection/byte/time limits, no host/LAN access; user informed it is a separate service. Tests: allowed Debian hosts, blocked loopback/metadata/LAN and arbitrary destinations.
- [ ] **P9.3 Integrate guest Ethernet without exposing host resources.** Acceptance: VirtIO net or documented SLIP path, bounded frames, explicit bridge state. Tests: guest interface initialization and packet counters.
- [ ] **P9.4 Validate DNS, TCP and HTTPS.** Acceptance: real guest resolution/connections through the selected bridge. Tests: controlled endpoint and Debian repository certificate verification.
- [ ] **P9.5 Validate apt end to end.** Acceptance: signed repository metadata, package downloads, dpkg installation/removal and database consistency. Tests: actual online guest operations and persistence across a fresh browser process.
- [ ] **P9.6 Test network failure and offline behavior.** Acceptance: bounded retries, visible errors, no silent fallback to mocked package results. Tests: bridge unavailable, connection reset, timeout, offline browser.

## Phase 10 — Security and Reliability
- [ ] **P10.1 Audit Rust/WASM memory boundaries.** Acceptance: unsafe code reviewed, imports cannot address host memory, guest DMA ranges checked. Tests: malformed lengths/descriptors/images and fuzzed parsers.
- [ ] **P10.2 Validate every worker message.** Acceptance: discriminated messages, size/range/profile checks, request ownership and no arbitrary code evaluation. Tests: malformed/oversized/out-of-order/duplicate messages.
- [ ] **P10.3 Validate imported images and metadata.** Acceptance: capacity/hash/schema limits, no path traversal, symlink extraction or unbounded decompression. Tests: hostile imports and round-trip integrity.
- [ ] **P10.4 Enforce storage ownership and profile isolation.** Acceptance: one writer per profile, leases renewed/recovered safely, no cross-profile snapshot use. Tests: competing tabs, expired leases and interrupted transactions.
- [ ] **P10.5 Audit dependency provenance and licenses.** Acceptance: direct/transitive dependencies necessary, compatible and pinned; notices included. Tests: cargo/npm audits where available, lockfile and license verification; record tool limitations.
- [ ] **P10.6 Test crash recovery and storage exhaustion.** Acceptance: no acknowledged write lost after recoverable interruption; visible degraded/fault states. Tests: kill worker/browser and inject failures at journal/flush/commit boundaries.
- [ ] **P10.7 Test resource cleanup.** Acceptance: no leaked worker, timer, GPU buffer, OPFS handle or event listener across restarts. Tests: repeated lifecycle loops and browser heap/resource measurements.
- [ ] **P10.8 Review logs and secrets.** Acceptance: terminal data opt-in for artifacts, no bridge credentials or private guest data in diagnostics by default. Tests: error/log inspection and documented redaction policy.

## Phase 11 — Comprehensive Testing
- [x] **P11.1 Run the complete Rust gate.** Acceptance: fmt check, cargo check, all tests, all-target/all-feature clippy -D warnings pass on supported targets. Tests: exact commands/results recorded, including failure conditions.
- [x] **P11.2 Run the complete TypeScript gate.** Acceptance: strict typecheck, lint, formatting, unit/integration tests pass. Tests: exact commands/results and coverage of external-input validation.
- [ ] **P11.3 Validate WASM in actual browsers.** Acceptance: bindings, worker execution, memory bounds, failure propagation and optional capabilities verified. Tests: Chromium plus additional available supported browsers.
- [x] **P11.4 Collect genuine kernel boot evidence.** Acceptance: source/config/build/device/memory/interrupt/console/filesystem/power logs available. Tests: native reference and browser boots of the pinned kernel.
- [ ] **P11.5 Complete BusyBox behavioral verification.** Acceptance: every listed applet tested for actual side effects and errors. Tests: guest script with per-applet assertions and raw output.
- [ ] **P11.6 Complete Debian behavioral verification.** Acceptance: boot/login/shell/BusyBox/diagnostics/apt/dpkg/install/remove/persistence pass. Tests: real riscv64 image in browser with bridge and fresh context.
- [ ] **P11.7 Complete persistence verification.** Acceptance: files, directories, configuration and packages survive reload/restart; failures recover safely. Tests: OPFS/IndexedDB browser suite and content verification after reboot.
- [ ] **P11.8 Complete graphics verification.** Acceptance: WebGPU and fallback paths render real guest output, resize, clean up and recover. Tests: enabled/disabled/device-loss browser configurations and pixel assertions.
- [ ] **P11.9 Complete end-to-end profile workflows.** Acceptance: both profiles boot, execute commands, mutate state and survive reload independently. Tests: full prescribed user workflow, not mocked integration substitutes.
- [ ] **P11.10 Run negative and long-running regressions.** Acceptance: invalid imports, denied persistence, quota pressure, interrupted sessions and repeated restarts handled. Tests: fault-injection matrix and measured soak test.
- [ ] **P11.11 Publish reproducible benchmarks.** Acceptance: actual boot, memory, block throughput and terminal latency results with hardware/browser/version and procedure. Tests: repeated runs with raw samples; no invented performance figures.
- [x] **P11.12 Maintain requirement-to-evidence traceability.** Acceptance: every checked task links to a test ID with environment, command, expected/actual result and artifacts. Tests: automated TODO/evidence consistency check and manual review.

## Phase 12 — Release Readiness
- [ ] **P12.1 Recheck the entire Definition of Done.** Acceptance: no mandatory requirement marked complete without actual evidence. Tests: requirement audit against final build.
- [ ] **P12.2 Reproduce runtime and guest artifact builds.** Acceptance: clean checkout/toolchain produces pinned artifacts or explains exact unavailable tool/source. Tests: two clean builds and hashes; signature verification separately reported.
- [ ] **P12.3 Validate clean installation and first boot.** Acceptance: documented setup works without hidden local assets or manual VM knowledge. Tests: fresh origin/profile and both Linux profiles.
- [ ] **P12.4 Verify browser compatibility and offline startup.** Acceptance: supported matrix based on executed tests; cached app starts offline and retains previously installed guest data. Tests: real browser contexts including optional-feature failures.
- [ ] **P12.5 Publish architecture, security, limitations and recovery documentation.** Acceptance: users understand emulation, relay dependency, storage eviction and unimplemented requirements. Tests: documentation review against actual behavior.
- [ ] **P12.6 Prepare a release only when mandatory gates pass.** Acceptance: versioned app/guest manifests, notices, artifacts and test report; no preview labeled finished. Tests: final Rust/TS/WASM/guest/browser regression run and release artifact verification.

## Next priorities
1. Fix browser snapshot memory pressure and verify create/restore/delete end to end (P4.7, P8.6).
2. Fix offline PWA caching and verify offline boot/update/rollback (P8.8, P12.4).
3. Implement and deploy the constrained network relay + guest VirtIO-net/SLIP; verify online apt (P9).
4. Add VirtIO keyboard/pointer, clipboard and guest tty resize; verify real guest input (P7.4, P8.3, P8.4).
5. Complete WebGPU/fallback/device-loss matrix, crash/quota/isolation fault injection, benchmarks, and clean-room reproducible builds (P7.5–P7.7, P10.6, P11.8, P11.11, P12.2).

Evidence: `docs/TEST_REPORT.md` maps commands, environments, expected/actual results and open failures. Unchecked items are not complete; partial progress is described in the report rather than silently upgraded.
