# WASMNux — Architecture Decision Document

## Context
Goal: run a genuine Linux kernel + userspace inside a browser tab, driven by a Rust core compiled to WebAssembly, with a TypeScript browser UI, WebGPU rendering (Canvas 2D fallback), and persistent browser storage.

## Key feasibility findings (honest)

### 1. The upstream Linux kernel cannot be compiled to WASM
The upstream Linux kernel is written in C/assembly for conventional CPU architectures (x86_64, arm64, riscv64, …). There is no genuine, verifiable upstream-compatible WASM kernel target. WebAssembly is not a CPU architecture the Linux kernel boots on. Per the mission constraints, we do NOT compile the kernel to WASM and do NOT fake a kernel.

### 2. Selected approach: a from-scratch VM/emulator in Rust compiled to WASM
Instead of compiling the kernel, we implement a genuine virtual machine/emulator whose runtime executes in WebAssembly and boots an actual guest Linux kernel binary. The emulator implements CPU instruction emulation, guest RAM, interrupts, timers, a bootloader/kernel-boot protocol, virtual block storage, a serial console, and a framebuffer.

### 3. Guest CPU architecture: RISC-V 64-bit (RV64GC)
Selected because:
- RISC-V is a genuine upstream Linux architecture (Tier 1) with a real `riscv64` Linux port.
- Debian officially publishes a `riscv64` port (latest stable Debian supports it).
- The ISA is a small, fixed-length, clean RISC design — the simplest architecture to implement correctly from scratch in Rust without external crates.
- A machine can be booted via the RISC-V boot protocol (a0 = hart id, a1 = device-tree pointer) with an initramfs.

### 4. No external Rust crates
The build sandbox cannot reach `crates.io` or `static.rust-lang.org`. The preinstalled Rust 1.97.0 toolchain has `wasm32-unknown-unknown` std prebuilt. Therefore `wasmnux-core` is pure `std` only — no `wasm-bindgen`, no `serde`, no third-party emulator crate. WASM/TypeScript interop uses the raw WebAssembly JavaScript API (exports/imports + linear memory), which is an equivalent stable interoperability mechanism.

### 5. Storage: OPFS + IndexedDB
Persistent virtual disks live in Origin Private File System (OPFS) files; VM metadata/configs/snapshots live in IndexedDB. `navigator.storage.persist()` and `estimate()` are used to detect persistence status and quota. All guest storage is sandboxed to the origin — no host filesystem access.

### 6. Graphics: WebGPU with Canvas 2D fallback
Guest framebuffer memory is read by the TypeScript runtime and uploaded as a texture each frame. WebGPU is used where available; Canvas 2D (`putImageData`/`drawImage`) is the functional fallback. No host GPU passthrough; WebGPU does not accelerate guest CPU instructions.

### 7. Networking: offline by design
Browsers provide no raw TCP/UDP sockets and no bridge to host network interfaces. Only `fetch`/`WebSocket` are available. Guest networking is documented as not feasible inside the sandbox; both Linux profiles run offline. Package management (apt) therefore cannot reach repositories in this deployment model; it works offline with locally cached packages only.

## Sandbox build restrictions (this checkout)
- `crates.io`, `static.rust-lang.org`, `kernel.org`, `deb.debian.org`, `raw.githubusercontent.com`: UNREACHABLE.
- Reachable: `github.com`, `api.github.com`, `registry.npmjs.org`, `pypi.org`, `files.pythonhosted.org`.
- No browser binary installed → browser/OPFS/WebGPU/E2E tests cannot run here.
- No Linux kernel image or Debian rootfs can be obtained → real boot cannot be verified in this sandbox.

## Consequences (limitations, not hidden)
The following Definition-of-Done items cannot be implemented or verified in this sandbox and are left explicitly incomplete in TODO.MD:
- Real Linux kernel boot (no kernel image; MMU/sv39 not yet implemented in the from-scratch emulator).
- Debian boot with apt/dpkg (no rootfs; no network repo access).
- File persistence across browser reload (no browser to test OPFS).
- WebGPU rendering on real browsers (no browser).
- E2E Playwright tests (no browser).

The closest valid implementation is delivered instead: a fully unit-tested, WASM-compilable Rust emulator core plus a strict TypeScript browser runtime (OPFS storage, Canvas/WebGPU renderer, terminal, lifecycle) that is structurally ready to load a guest kernel + initramfs once those assets are available.

## Module layout
- `runtime/` — Rust crate `wasmnux-core` (pure std): guest memory, RISC-V RV64 CPU, CSRs, CLINT timer, UART, device-tree emitter, ELF loader, machine orchestrator, raw WASM C ABI exports.
- `web/` — TypeScript browser app: WASM bindings, VM lifecycle, OPFS storage, WebGPU + Canvas 2D renderer, terminal, profile selector, snapshots.

## Security boundaries
- Guest memory is a single bounded WASM linear memory; all guest loads/stores bounds-checked in Rust.
- No host filesystem access exposed to the guest.
- Guest is untrusted; no `eval`/dynamic code execution in the app context.
- Imported disk images/metadata validated (path traversal, archive structure) before use.
- WASM boundary is typed; no unsafe JS/WASM interfaces.
