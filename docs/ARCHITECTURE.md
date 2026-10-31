# Architecture decision record: genuine Linux in the browser

Status: **research / implementation in progress**, not a completed VM. Date: 2026-10-31.

## Decision and scope

Run an unmodified, upstream-compatible RISC-V Linux kernel as a **guest** of a full-system emulator compiled from Rust to WebAssembly. WebAssembly is the host instruction set, not an upstream Linux CPU architecture. WASI is not a substitute for a guest kernel. `riscv64` is the provisional guest architecture because Linux supports it and [Debian 13 officially supports it](https://www.debian.org/releases/trixie/release-notes/whats-new.en.html). Compatibility of a particular engine with the latest kernel and Debian is not established until actual boots are observed.

As of 2026-10-31 the [official kernel release API](https://www.kernel.org/releases.json) reports stable **7.3.1** (2026-10-30); the [official Debian releases page](https://www.debian.org/releases/) reports **Debian 13.7 Trixie**, stable (2026-09-12). These are dated observations, not permanent build pins. A release pipeline must fetch and authenticate current official metadata, pin source and verify signatures/hashes before building. Neither image has been built or booted here.

## Engines considered

| Candidate | License / activity observed | Assessment |
| --- | --- | --- |
| [justwasm/rv64.js](https://github.com/justwasm/rv64.js), `f9b55b32788a2649bfe769f3f2813d86fceed501` | MIT; pushed 2026-09-26; pre-release, small project, no independent security audit found | Provisional production candidate: Rust RV64 core, interpreter + Wasm JIT, virtio block, console and network, Linux direct SBI boot and worker mode. Upstream says Linux/Alpine boots; our Debian, current kernel, disk persistence and graphics tests are outstanding. Public block API accepts an image but does not expose a durable backing interface; an audited integration/upstream contribution is required. |
| [takahirox/riscv-rust](https://github.com/takahirox/riscv-rust), `8ee69d7a5dc7ef6d8b2bda96bf86d2923f2cf176` | MIT; last push 2023-04; prebuilt historical guest assets | Useful as an isolated **diagnostic**: existing Rust/Wasm RISC-V emulator plus upstream project's Linux 5.4/OpenSBI/BusyBox image. Lacks a supported disk export/backing API; its in-memory disk is lost on restart. It cannot fulfill either production profile. |
| [copy/v86](https://github.com/copy/v86) | BSD-2-Clause; active and much more established | Excellent x86/Wasm emulator, but does not implement x86-64 long mode; Debian 13 only offers partial i386 userspace. Not selected for the required current Debian guest. |
| [QEMU](https://www.qemu.org/) | GPL-2.0-or-later; mature | Strong system model but substantial browser/Wasm porting and size work; core is C rather than the requested Rust. Worth revisiting if RV64 candidate fails compatibility. |

No component is assumed secure because of its name or license. Pin commits and hashes, inventory transitive dependencies, maintain license notices, audit device/host boundaries and regularly check advisories before a release. Upstream claims are not our test results.

## Machine model and boundaries

Target a single RV64 hart initially: instruction execution and Sv39/Sv48 translation in the emulator; bounded guest RAM in Wasm linear memory; emulated interrupt controller and timer, SBI/direct Linux boot or firmware; virtio-mmio block and console, and serial UART. Validate device-tree, IRQs, clock, block flush semantics and driver configuration against actual kernel logs. Worker isolation prevents guest CPU execution from blocking the UI. A serial console passes bytes to a terminal emulator; TypeScript must never parse commands or fabricate their outputs. Input, resizing and worker messages need size limits and runtime validation.

The selected production emulator's framebuffer/graphics device is **not established**. WebGPU can upload and render an actual framebuffer with WGSL, but does not provide native GPU passthrough, accelerate arbitrary guest CPU instructions, or make a serial-only guest graphical. A Canvas 2D renderer is required; both must consume the same genuine virtual display output. Keyboard/mouse mapping follows only once a real input device exists.

## Durable storage design

Guest block writes must reach an origin-private durable backend, separately scoped to each profile. Prefer an OPFS sync access handle owned by a dedicated worker for large random-access disk data, with IndexedDB transactions for versioned metadata. Alternatively an IndexedDB block store can provide transactional updates for modest images. A successful IDB transaction is not equivalent to the guest flushing its own page cache: shutdown must issue guest `sync`/unmount or graceful poweroff and await device flush. Multi-block updates, snapshots and restoration must be atomic or journaled; an interrupted guest may still need filesystem recovery. Quota, eviction, private browsing, permission denial and persistent-storage failures must be surfaced. Do not assume OPFS or `navigator.storage.persist()` prevents eviction. A tab ownership/locking strategy is necessary to prevent concurrent writers.

Snapshots, disk import/export and resets must be explicitly profile-scoped and validate capacities, format, checksums, types and lengths. Never extract arbitrary imported archives or map guest paths to the host filesystem. Verify data from inside the guest after reload and a complete browser restart. Until a real virtual block adapter is integrated, no storage API is evidence that Linux writes persist.

## Network and security

Browsers cannot provide general raw TCP/UDP sockets. A virtual NIC needs an explicit, separately deployed and access-controlled WebSocket relay/proxy, or a limited CORS-permitted fetch bridge. Such a service must prevent SSRF and private/host resource access, and document DNS, TLS, bandwidth, logging and trust boundaries. Debian `apt` requires real authenticated repositories, real networking and a persistent package database; it must not be emulated as hardcoded output. Offline use is limited to already stored files and packages.

The browser origin and its workers are the isolation boundary, not a hostile-guest-proof hardware hypervisor. Guest OS and imported disks are untrusted. Do not expose local host files, credentials or privileged cross-origin resources. Browser vulnerabilities, emulator bugs, storage eviction and an insecure relay remain outside the guest's guarantees. Avoid printing user data in diagnostic logs.

## Environment constraints

The sandbox initially lacked Rust/Cargo, a cross compiler, QEMU and Chromium. Shell downloads are restricted to GitHub, its API/codeload, npm and PyPI. For this diagnostic, a Rust 1.99.0 native distribution was assembled from a GitHub mirror and verified against Rust's published SHA-256 **before** installation. Its Wasm target was copied from an npm mirror only after a separate GitHub Actions job downloaded the official target archive, checked the official archive hash, and verified the complete per-file hash manifest matched the mirror. This is a temporary test toolchain, not a checked-in binary. A target guest cross-compiler and authenticated current Linux/Debian images remain unavailable. See `TEST_REPORT.md` for exact provenance and gaps.

## Implemented diagnostic path (not a deliverable profile)

The application builds pinned `riscv-rust` source and a small Rust diagnostic ABI to Wasm, then runs the locally built module in a dedicated Web Worker. The worker validates the ABI exports, caps the module download at 4 MiB, and checks firmware/rootfs size and SHA-256 before handing them to Rust-owned image buffers. The 512 MiB Wasm linear-memory maximum and bounded cycle slices, image allocations, input/output queues and acknowledged output messages are diagnostic guardrails, not a complete hostile-guest security audit. The guest has 128 MiB of emulated RAM. The experimental page-translation cache is disabled.

The real Linux 5.4/OpenSBI 0.8/BusyBox 1.32 diagnostic runs through xterm.js; stopping or leaving the page terminates the worker and **discards** its in-memory disk. Terminal fit resizes the browser view but does not propagate a guest TTY size. The new host output interface distinguishes a NUL byte from an empty queue, but the upstream UART still uses zero as its input-empty sentinel and the terminal is not a binary transport.

The original diagnostic trapped when BusyBox `awk` executed `FCVT.L.D`; after implementing and testing that instruction, `sleep` reached another missing instruction, `FCVT.D.LU`. Both conversions were implemented with rounding-mode, boundary and flag tests. All original scripted utility assertions now pass in a real browser. A separate tiny RV64 ELF deliberately executes an invalid instruction to verify that a genuine Wasm trap reports the guest PC, preserves the original error, terminates cleanly and permits a fresh boot. This does **not** establish complete RISC-V ISA support; other instructions and workloads can still fail. The guest reports missing `/etc/init.d/rcS` and mounts unjournaled ext2. The historical block device previously advertised 100 MiB for a 50 MiB image; it now advertises the actual full-sector capacity and rejects tested out-of-range requests with an I/O error before transferring data. Its incomplete virtqueue, untrusted guest-memory address handling and other untested operations are not a production block-device security audit. No current kernel, Debian, apt or guest persistence is claimed.

## Independently tested prototypes

The IndexedDB store is not attached to the emulator. It uses profile-scoped disk metadata, sparse 64 KiB blocks, sector-aligned bounded I/O, atomic transactions and block snapshots. Version 2 adds indexes that enumerate even malformed block keys before snapshot/restore; upgrading the earlier schema preserves existing data. Chromium tests cover concurrent transactions, snapshot rollback on corruption, quota abort and a complete browser-process restart. This proves only the browser store's tested behavior, not guest filesystem flush, power-loss durability, eviction immunity, import/export, or multi-tab ownership of a live VM. `durability: "strict"` is a browser request, not proof of physical persistence. No prototype snapshot captures guest RAM or a running guest's cached filesystem writes.

The RGBA presenter creates a real WebGPU texture and WGSL render pipeline when available, falls back to Canvas 2D and recovers a retained frame after device loss. Chromium's SwiftShader test checks pixels and resizing; the normal UI labels its pattern synthetic. The presenter has **no guest display device or guest framebuffer input**. Device availability does not imply a hardware adapter, and this is neither GPU passthrough nor an implementation of a guest GPU.

The exact setup, artifact and license limitations are in `README.md` and `THIRD_PARTY_NOTICES.md`. The temporary toolchain verification is documented in `TEST_REPORT.md`; a release still requires its own reproducible, independently verified build pipeline.
