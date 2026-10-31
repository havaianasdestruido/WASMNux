# WASMNux

A real RISC-V Linux VM running entirely in a modern browser. Rust/WASM emulates the guest CPU and devices; strict TypeScript owns the worker, terminal, storage, and WebGPU/Canvas framebuffer. There is no remote shell, no mocked apt, and no claim of guest CPU acceleration or GPU passthrough.

## Status

**Preview / unfinished.** Vanilla Linux 7.3.1 + BusyBox and Debian 13.7 riscv64 boot in Chromium with real OPFS-backed disks. Debian package installation from a bundled `.deb` and dpkg database persistence across reload are verified. Guest networking, VirtIO input devices, browser snapshot/restore, and offline PWA startup are implemented only partially or not at all; see `TODO.MD` and `docs/TEST_REPORT.md` for exact evidence and failures.

## Run

```sh
npm ci
npm run dev       # http://localhost:5173
npm run build && npm run preview
```

Rust gates:

```sh
cargo fmt --all -- --check
cargo check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Browser E2E (Chromium with persistent OPFS quota):

```sh
npx playwright test boot.spec.ts graphics.spec.ts debian.spec.ts
```

## Profiles

- **Vanilla**: upstream Linux 7.3.1, static RV64 BusyBox 1.37.0, 64 MiB ext2 disk.
- **Debian**: Debian 13.7 trixie riscv64, real bash/coreutils/dpkg/apt, 256 MiB ext2 disk, bundled demo package for offline install verification.

Each profile has isolated OPFS disks, journals, metadata, leases, and snapshot namespaces. Storage is durable across reloads but **not guaranteed against browser eviction**; export backups and request persistent storage explicitly.

## Architecture and limitations

See `docs/ARCHITECTURE.md`, `docs/PROVENANCE.md`, `vendor/PATCHES.md`, and `docs/TEST_REPORT.md`. Networking requires a separate constrained relay that is not deployed; raw sockets/ICMP/LAN access are impossible from a web page. The framebuffer is a guest simplefb scanout rendered by WebGPU or Canvas, not a VirtIO-GPU 3D device. Keyboard input currently reaches the serial console only; pointer/clipboard/resize integration is incomplete.
