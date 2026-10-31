# WASMNux — Linux in WebAssembly

A **real** Linux system running entirely in a browser tab:

- A **Rust RISC-V 64 virtual machine** (CPU, Sv39 MMU, PLIC, CLINT, UART,
  VirtIO block, generated device tree) compiled to **WebAssembly** and run
  in a Web Worker.
- **Genuine, unmodified guests**: the latest stable upstream Linux kernel
  with a BusyBox initramfs (**Vanilla** profile) and the latest stable
  **Debian riscv64** rootfs (**Debian** profile).
- **Persistent virtual disks** in OPFS/IndexedDB (chunked, CRC-checked,
  copy-on-write snapshots) that survive reloads and browser restarts.
- **WebGPU framebuffer presentation** (WGSL) with a Canvas 2D fallback.

There is no terminal simulator and no mocked command anywhere: the browser
terminal *is* the guest's serial console. If the WASM core or the guest
images are absent, the app says so and refuses to "boot".

> Status: foundation phase. Read [`TODO.MD`](TODO.MD) for the honest,
> evidence-gated checklist, [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
> for the design (including why compiling the kernel itself to WASM is
> impossible and what we do instead), and
> [`docs/TEST-REPORT.md`](docs/TEST-REPORT.md) for exactly what has been
> executed and verified so far.

## Quick start

Prerequisites: Node 22+. For a bootable system you also need the WASM core
and guest images — prebuilt by CI, or build them locally (Rust for the
core; a riscv64 cross-compiler for guests).

```bash
# 1. Web app
cd web && npm ci && cd ..

# 2. WASM core — either from CI artifacts:
./scripts/fetch-ci-assets.sh
# …or locally (rustup required):
./scripts/build-wasm.sh

# 3. Run
cd web && npm run dev   # http://localhost:5173
```

Without guest images the app still opens and fully exercises capability
detection, storage, snapshots and the renderer — Boot stays disabled with
the reason (build guests with the pipeline in [`guest/`](guest/), or let
the **Guest images** workflow do it and `fetch-ci-assets.sh` download them).

## Repository layout

```
crates/wasmnux-core    Dependency-free RV64 virtual machine (Rust)
crates/wasmnux-wasm    wasm-bindgen surface used by the worker
web/                   TypeScript app (profiles, OPFS disks, terminal, WebGPU)
guest/                 Kernel config, initramfs /init, build + test scripts
scripts/               WASM build + CI asset fetching
docs/                  Architecture, security, dependencies, test evidence
TODO.MD                The phase tracker / single source of truth
```

## Verification (nothing is taken on faith)

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cd web && npm run typecheck && npm run lint && npm test && npm run build
npx playwright test            # real-Chromium persistence/renderer suite
sh guest/tests/busybox-applets.sh   # applet behaviour (host or in-guest)
```

CI repeats all of this on every push. License: Apache-2.0 (guest software
keeps its own licenses — see `docs/DEPENDENCIES.md`).
