# Architecture decision: emulate a real Linux guest

**Status:** feasibility accepted; implementation and integration verification in progress.
**Decision date:** 2026-10-31. **This document does not assert that the project is finished.**

## Decision

Run an unmodified, native **RV64GC Linux guest** on a Rust full-system emulator compiled to `wasm32-unknown-unknown`. TypeScript owns the worker, browser devices, storage and presentation. The guest kernel is not compiled to WASM. OpenSBI supplies the supervisor boot environment. The initial board is single-hart, little-endian, Sv39, with a QEMU-virt-like address map.

The planned execution path is:

```
Browser UI (strict TypeScript)
  <-> validated, bounded worker messages
  <-> Rust runtime in a dedicated Web Worker / WebAssembly
        RV64GC CPU + Sv39 + PLIC + CLINT + UART + VirtIO
        |                  |                    |
   genuine Linux      block requests       framebuffer/input
        |                  |                    |
   BusyBox/Debian     OPFS disk + journal   WebGPU / Canvas 2D
                           |
                    IndexedDB metadata
```

Neither the browser nor TypeScript interprets Linux commands. A terminal is connected to the guest's emulated serial port. Guest RAM is isolated inside the runtime's WASM memory. A disk is a guest-owned block image, not access to a host directory.

## Why not compile upstream Linux to browser WASM?

The fetched genuine Linux **7.3.1** source contains no `arch/wasm`. Its architecture directories are alpha, arc, arm, arm64, csky, hexagon, loongarch, m68k, microblaze, mips, nios2, openrisc, parisc, powerpc, riscv, s390, sh, sparc, um, x86 and xtensa. `um` is User Mode Linux on a host Linux kernel, not a browser WASM target. Experimental Linux-WASM forks are not the requested unmodified upstream kernel and are not evidence of Debian compatibility. WebAssembly executing an emulator is a different claim from Linux supporting WASM as a kernel architecture.

## Why RV64 rather than the usual browser x86 route?

Debian's official stable release information identifies **Debian 13 (trixie)**, updated to **13.7**, and lists **riscv64** as a release architecture. Standalone i386 is only a co-architecture on amd64. Therefore an emulator limited to 32-bit x86 cannot satisfy this Debian requirement with an old Debian image substituted silently. Official information: https://www.debian.org/releases/stable/ .

RV64 also has a genuine upstream Linux port, OpenSBI boot support and mature VirtIO bindings. The compromise is emulation speed and incomplete ISA/device coverage, not replacing the guest OS. Vector instructions must not be advertised unless actually tested; the initial guest uses the scalar RV64GC ABI. Debian riscv64 compatibility still requires an actual boot/package test.

## Alternatives inspected

| Candidate | Source inspected | Useful properties | Reason not selected as the complete solution |
|---|---|---|---|
| v86 | `copy/v86`, commit `315b2b0b758703ef0e6cc7b4088d6068f83f3c20`; BSD-2-Clause | Mature browser x86 PC emulator, Rust/C/JIT components, real Linux boots, graphics and networking | No 64-bit guest; current Debian cannot be a standalone i386 installation. Mixing an old Debian image would violate the request. |
| riscv-rust | `takahirox/riscv-rust`, commit `8ee69d7a5dc7ef6d8b2bda96bf86d2923f2cf176`; MIT | Small Rust RV32/64 CPU, existing browser WASM example, OpenSBI, VirtIO block | Last source update inspected was 2021; incomplete ISA, no modern GPU/network path or current Debian evidence. Useful comparison, not proof of modern compatibility. |
| rsemu / riscv-emu | `evanphx/riscv-emu`, commit `0a320bc4d8009eec2438b7544747c22aa121781f`; MIT | Rust RV64GC, Sv39, PLIC/CLINT, VirtIO block/net/GPU/input, native snapshot code, upstream author reports Debian trixie execution | Native mmap, host files, threads and clocks require a browser port. Recent project, not a mature security-audited hypervisor. Selected as the auditable CPU/device base, **not yet accepted as browser-compatible**. |
| TinyEMU / riscbox | TinyEMU-family browser RISC-V implementations, MIT-family licensing | Established Linux boot path and framebuffer | Core is C/Emscripten rather than the requested Rust core. A useful reference, but not the primary runtime. |
| Write a new CPU from scratch | Specifications and existing test suites | Full design control | Unjustified correctness/security risk when an auditable Rust implementation can be integrated. |

Upstream reports are research evidence, not this project's test results. The selected core must pass native and browser execution tests before the corresponding TODO items are checked.

## Version resolution

- Kernel.org's cached public release table and newer maintainer tags disagreed during the audit. The actual `gregkh/linux` annotated tag `v7.3.1` resolves to commit `79a907609d53d5cd645b42c11550d295d43abf99`, dated **2026-10-30**, with the message “This is the 7.3.1 stable release.” This is the current stable maintenance target selected for the build, newer than the cached 7.2.13 table. Linux 7.3 is the mainline series, not an experimental WASM fork.
- Official Debian stable: **13.7 trixie**, released as a point update **2026-09-12**. riscv64 Debuerreotype artifact metadata at immutable commit `64a870d7670389268c0d44ec0e0a4392d36d579c` reports 13.7. The rootfs digest must be verified before packaging; metadata alone is not a boot test.
- BusyBox **1.37.0** is the selected release candidate for a reproducible build. Upstream labels 1.38.0 unstable; “latest development release” is not automatically a stable dependency. Source mirror revision `be7d1b7b1701d225379bc1665487ed0871b592a5` must be checked against the official release archive hash before a release.
- OpenSBI **1.6**, commit `bd613dd92113f683052acfb23d9dc8ba60029e0a`, is the initial firmware pin. It is rebuilt for the actual board rather than assuming an unrelated QEMU binary is compatible.

The release discovery/build script must repeat discovery, pin immutable revisions and record hashes. No runtime “latest” URL is silently substituted for a tested image.

## Hardware and boot contract

- CPU: one RV64GC hart, M/S/U privilege, compressed instructions, software IEEE floating point, atomics and Zicsr/Zifencei; Sv39 page tables. No SMP, Sv48, hypervisor extension or native CPU passthrough claim.
- RAM: fixed allocation at physical `0x80000000`; capacity validated before allocation. Initial target 256 MiB, subject to actual boot measurements. No unbounded WebAssembly memory growth during CPU execution.
- Firmware: raw OpenSBI `fw_jump` at `0x80000000`; Linux Image at `0x80200000`; DTB at `0x82200000`; optional initramfs at `0x88000000`. All image ranges must fit and not overlap. OpenSBI receives hart ID in `a0` and DTB address in `a1`.
- Interrupts: PLIC at `0x0c000000`; CLINT at `0x02000000`; NS16550A at `0x10000000`, IRQ 10; VirtIO MMIO from `0x10008000`. Power/reset controller at `0x10000100`. The device tree and actual bus must agree.
- Time: a monotonic host clock drives guest time; a paused guest must account for elapsed wall time explicitly. Execution is split into bounded instruction slices so worker commands and storage requests can make progress. WFI must not block a browser worker indefinitely.
- Storage: VirtIO block, 512-byte logical sectors, bounded descriptor chains and explicit flush. Browser OPFS synchronous access handles run only in a worker. A journal/commit protocol must be verified before claiming crash consistency. Filesystem correctness remains the guest filesystem's responsibility; the block layer must preserve acknowledged flush ordering.
- Console: NS16550A bytes connect to xterm.js. Resizing uses the guest tty's window-size mechanism, not merely changing the HTML canvas. Shell output is never synthesized.
- Graphics: the selected engine's VirtIO GPU 2D scanout is the candidate. Guest DRM resources/transfers produce framebuffer pixels; WebGPU uploads and renders them, with Canvas 2D fallback. This accelerates presentation, **not arbitrary guest instructions**. The integration is incomplete until real guest pixels are tested.
- Input: serial keyboard initially; VirtIO keyboard/pointer for the graphics path. Browser event listeners belong to an abortable UI lifecycle. No guest access to host input outside the focused application.
- Network: browsers cannot expose arbitrary raw TCP/UDP/ICMP sockets to a guest. An optional, separately deployed TLS WebSocket relay is required for normal apt HTTPS. It must allow only Debian repository destinations and deny loopback, private networks and cloud metadata. No relay is silently assumed. Offline shells and already installed packages must still work.

## Storage design and remaining proof obligations

Profile names are an enum (`vanilla`, `debian`), never import-supplied paths. Each has independent OPFS disk/journal/snapshot namespaces and IndexedDB metadata. A single-writer lease prevents two tabs from modifying the same disk. Guest writes go to the block backend; metadata is not a substitute for flushing guest data.

The intended crash-recovery design uses bounded copy-on-write/write-ahead transactions with generation IDs, checksums and commit records. Recovery selects the last complete generation before allowing guest execution. Import/export accepts only bounded raw images and validated versioned metadata, not arbitrary archives. Snapshot RAM/CPU/device state must be associated with the exact disk generation; restoring CPU state over a changed disk is rejected.

OPFS and IndexedDB are separate browser storage facilities, not a single distributed transaction. Their ordering protocol and failure tests are mandatory. `navigator.storage.persist()` reduces eviction risk but is not a guarantee. Quota, private browsing, browser clearing, device failure and OS disk failure can still destroy data. Export backups remain necessary. A browser reload test and a full process restart test are separate evidence items.

## Runtime, dependencies and browser constraints

The Rust adapter exposes a narrow, versioned C ABI (an allowed stable interoperability alternative to wasm-bindgen), with explicit allocation ownership and checked lengths. TypeScript validates worker messages and owns all browser APIs. The first runtime is single-threaded inside one module worker and does not require SharedArrayBuffer, cross-origin isolation, native JIT or host threads. Threaded optimizations require separate capability and correctness tests.

Only vendor the CPU/device portions actually used, preserve upstream MIT notices and publish every local patch. Native host-filesystem, mmap, signal, terminal and JIT code must not become browser imports. Browser clocks, entropy, storage and graphics use explicit capability adapters. External dependency additions require a recorded purpose, source, license and WASM/native build check.

A dedicated worker limits main-thread blocking but cannot protect against all emulator bugs or memory exhaustion. WASM is a sandbox, not a security guarantee against denial of service. Guest processes are untrusted; guest root does not grant host filesystem access. Guest kernel vulnerabilities, emulator memory errors and browser vulnerabilities remain risks. Logs may contain secrets typed into the terminal; diagnostic export must be opt-in.

## Explicit non-completions

At this decision point, no project kernel boot, Debian package operation, OPFS persistence, WebGPU guest rendering, network relay, snapshot correctness or browser-restart test is claimed. The backlog deliberately keeps these open. The project is not a completed browser-native Linux product merely because the architecture is feasible or a Rust module compiles.

See `TODO.MD` for acceptance criteria and `docs/TEST_REPORT.md` for executed evidence and environmental restrictions.
