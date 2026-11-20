# WASMNux — Browser-Native Linux Virtual Machine (`RV64GC` Rust/WASM + WebGPU + OPFS/IndexedDB)

**WASMNux** is a genuine, zero-mock Linux virtual machine running entirely inside a modern web browser. It executes an unmodified upstream **Linux `7.3.1`** (`riscv64`) kernel inside a **305 KiB zero-dependency Rust WebAssembly (`wasm32-unknown-unknown`) virtualization engine** inside a dedicated Web Worker, paired with **WebGPU WGSL compute + render `/dev/fb0` presentation** (with automatic **Canvas 2D** fallback) and **crash-consistent persistent virtual block storage** backed by **OPFS + IndexedDB**.

> **No fake shells. No mocked commands.** Every shell prompt, file operation, `ext4` journal commit, `/dev/fb0` pixel, TCP/IP packet, and `apt-get install` executes inside the real guest Linux `7.3.1` kernel on `RV64IMAFDC + Sv39`.

---

## Key Capabilities

- **Two Isolated Linux Profiles** (independent `ext4` overlays, snapshots, settings, and factory reset):
  1. **Vanilla Linux (`7.3.1` + BusyBox `1.37.0`)**: Cold-boots to root shell in **~3.84 seconds** median (`[3996.7, 3842.2, 3712.1]` ms across 3 trials on 2-vCPU Xeon @ 2.60 GHz; `63.2M` instructions, `24.6 MIPS`), static `riscv64-linux-musl` BusyBox `1.37.0` (rebuilt from source) with 300+ applets, persistent 64 MiB `ext4` `/dev/vda`, and `/usr/bin/wasmnux-fbtest` `/dev/fb0` graphics generator. Both the kernel and the userspace are now built from source in-repo (`scripts/build-kernel.sh`, `scripts/build-busybox.sh`, `scripts/build-vanilla-rootfs.sh`).
  2. **Debian `13.7` ("trixie") `riscv64`**: Cold-boots official Debian 13.7 `riscv64` (`glibc 2.41`, `bash 5.2`, GNU `coreutils 9.7`, `apt 3.0.3`, `dpkg 1.22.22`, BusyBox) in **~3.25 seconds** median (`[3249.0, 3256.0, 3171.5]` ms across 3 trials; `78.0M` instructions, `24.0 MIPS`), supporting real `apt-get update`, `apt-get install`, dependency resolution, and `apt-get remove` over `VirtIO-Net` (`eth0`) with persistent `/var/lib/dpkg/status` across browser restarts and offline `.deb` usability.
- **Rust -> WebAssembly Virtualization Core (`crates/wasmnux-*`)**:
  - `RV64IMAFDC` + `Zicsr` + `Zifencei`, Machine/Supervisor/User privilege rings, `Sv39` 3-level MMU with direct-mapped `ITLB`/`DTLB` fast paths, `SBI v0.2`, `CLINT` (`WFI` timer fast-forward), `PLIC`, `NS16550A` UART, `Goldfish RTC`, `Syscon` poweroff/reboot, `SimpleFB` (`640x400` `BGRX8888` dirty-row bitmap), and `VirtIO MMIO v2` (`virtio-blk`, `virtio-net`, `virtio-rng`).
  - Runs off the main UI thread in a dedicated Web Worker; works with **or without** `SharedArrayBuffer` / `crossOriginIsolated`.
- **Genuine WebGPU Graphics + Canvas 2D Fallback (`src/gpu/`)**:
  - `CONVERT_BGRX_TO_RGBA_WGSL` `@compute @workgroup_size(64)` shader converts guest `/dev/fb0` `XRGB8888` pixels + dirty-row spans into a `GPUTexture`, presented via `PRESENT_FRAMEBUFFER_WGSL` with Crisp Nearest, Smooth Bilinear, or CRT scanline modes and automatic `device.lost` recovery.
  - Falls back seamlessly to `Canvas2DFramebufferRenderer` when `navigator.gpu` is unavailable.
- **Crash-Consistent Persistent Storage (`src/storage/`)**:
  - 256 KiB copy-on-write sparse chunks with IEEE 802.3 CRC32 verification, generation-numbered OPFS files (`opfs+idb` primary, `idb-only` fallback), atomic IndexedDB manifest commits, orphan-chunk startup recovery, `navigator.storage.persist()` & quota telemetry, gzipped live VM snapshots (`WNUXFULL`), and `.wasmnux-disk` (`WASMNUXD` v1) import/export.
- **Workstation Browser UI & PWA (`src/ui/`, `public/sw.js`)**:
  - `@xterm/xterm` interactive serial console with Copy, chunked flow-controlled Paste, terminal size sync (`stty`), font/theme persistence, Pause/Resume/Reboot/Graceful Shutdown/Hard Reset/Disk Reset controls, live kernel milestone timeline & `dmesg` filter, and offline-capable PWA Service Worker (`wasmnux-static-v1` strictly separated from guest OPFS/IDB disks).

---

## Quick Start

```bash
# Install dependencies (without downloading extra browser binaries if using system/bundled Chromium)
PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci

# Start development server (binds 0.0.0.0:5173 with COOP/COEP headers)
npm run dev

# Build WASM + TypeScript + production bundle into dist/
npm run build

# Preview production build on 0.0.0.0:4173
npm run preview
```

---

## Deploying to GitHub Pages (Fully Static Hosting)

WASMNux has no server-side component: the WASM runtime, the Linux kernel/initramfs/rootfs images, and the `/debian-repo/` APT mirror are all plain files, so GitHub Pages can host the whole machine.

1. **Enable Pages once** — repository **Settings -> Pages -> Build and deployment -> Source: `GitHub Actions`**.
   Pages requires a public repository on GitHub Free; private repositories need a plan that includes Pages.
2. **Publish** — push to `main` (or run the **Deploy to GitHub Pages** workflow manually from the Actions tab). [`.github/workflows/pages.yml`](./.github/workflows/pages.yml) compiles `public/wasm/wasmnux_wasm.wasm` from Rust, runs the strict TypeScript typecheck, bundles with Vite, and publishes `dist/` through `actions/upload-pages-artifact` + `actions/deploy-pages`.
3. **Open** `https://<owner>.github.io/WASMNux/` — the project site is served from a sub-path, which the app supports natively:

   - `src/runtime/asset-url.ts` resolves the WASM runtime, kernel, initramfs, and ext4 base disks against the **application base URL** (`document.baseURI` + Vite `import.meta.env.BASE_URL`) instead of the origin root.
   - The dedicated Worker forwards that base URL to the guest `VirtIO-Net` HTTP bridge (`src/net/network-bridge.ts` `basePath`), so `apt-get update` / `apt-get install` resolve to `.../WASMNux/debian-repo/...`.
   - `public/sw.js` derives its shell URLs and cache name from its own registration scope, keeping the offline PWA working under any prefix.

GitHub Pages cannot emit custom `Cross-Origin-Opener-Policy` / `Cross-Origin-Embedder-Policy` headers; WASMNux detects the missing isolation (`crossOriginIsolated === false`) and uses its structured-clone Worker transport, so Linux still cold-boots (see limitations below). Full setup, local sub-path preview, and troubleshooting notes: [`docs/GITHUB_PAGES.md`](./docs/GITHUB_PAGES.md).

---

## Verification & Benchmarks

```bash
# 1. Rust formatting, static analysis, clippy (-D warnings), & unit tests
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

# 2. TypeScript formatting, strict typecheck, ESLint, & Vitest unit/integration/security tests
npm run format:check
npm run typecheck
npm run lint
npm test

# 3. Reproducible performance benchmarks (cold boot, MIPS, RTT, ext4 I/O, snapshots)
npm run bench

# 4. Full Headless Chromium 153 Playwright E2E suite (Vanilla, Debian APT, WebGPU, Storage, UI/PWA)
npm run test:e2e
```

### Measured Performance Summary (`docs/BENCHMARKS.md` — `2026-10-31T23:35:44.846Z`, 2-vCPU Xeon @ 2.60 GHz)

| Metric                                                              | Measured Value (`N=3` boots / snapshots, `N=5` RTT)                                    |
| ------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| `wasmnux_wasm.wasm` release binary                                  | **317.0 KiB** (`324,631` bytes, 44 ABI exports, 0 external crates)                     |
| **Vanilla Linux `7.3.1`** cold boot to shell (median)               | **3,842.2 ms** (`[3996.7, 3842.2, 3712.1]` ms, **24.6 MIPS**, `145.1 MiB` WASM memory) |
| **Debian `13.7` ("trixie")** cold boot to `bash` (median)           | **4,424.0 ms** (`[4424.0, 4274.0, 4434.8]` ms, **23.7 MIPS**, `344.7 MiB` WASM memory) |
| Terminal serial round-trip latency (min / median / max)             | **1.96 ms / 4.20 ms / 20.78 ms** (`[20.78, 20.74, 2.15, 1.96, 4.20]` ms)               |
| Guest `ext4` `/dev/vda` 4 MiB `urandom+fsync` write / uncached read | **421.1 MiB/s** (`9.5 ms`) / **18.7 MiB/s** (`213.4 ms`)                               |
| Live 128 MiB VM snapshot size (gzip) & restore time                 | **14.24 MiB** (9.3x ratio) / **66.7 ms** median (`89.7 ms` cold Trial 1)               |

---

## Reproducible Guest & Toolchain Build Scripts

All artifacts in `public/wasm/`, `public/images/`, and `public/debian-repo/` are reproducibly built from official upstream sources via `scripts/`:

| Script                              | Output                                                      | Upstream Source                                                                                                                  |
| ----------------------------------- | ----------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `./scripts/setup-toolchains.sh`     | Rust `1.88.0` (`wasm32`), LLVM/Clang `23.1.3`, Zig `0.17.0` | Official toolchain packages                                                                                                      |
| `./scripts/build-wasm.sh`           | `public/wasm/wasmnux_wasm.wasm`                             | `crates/wasmnux-*`                                                                                                               |
| `./scripts/build-kernel.sh`         | `public/images/vmlinux-7.3.1-riscv64.Image.gz`              | Linux `v7.3.1` (`configs/linux-7.3.1-riscv64.config`); falls back to `pip install kconfiglib` when the host has no flex/bison/bc |
| `./scripts/build-busybox.sh`        | Static `riscv64-linux-musl` BusyBox `1.37.0`                | BusyBox trunk (commit pinned in script)                                                                                          |
| `./scripts/build-vanilla-rootfs.sh` | `vanilla-initramfs.cpio.gz` + `vanilla-disk.ext4.gz`        | BusyBox `1.37.0` + `wasmnux-fbtest`                                                                                              |
| `./scripts/build-debian-rootfs.sh`  | `debian-rootfs.ext4.gz` + `public/debian-repo/`             | Official Debian `13.7` ("trixie") `riscv64` OCI rootfs                                                                           |

---

## Networking, Deployment Headers, & Known Browser Sandbox Limitations

1. **Browser Networking Model (`VirtIO-Net` <-> User-Mode TCP/IP <-> Fetch Bridge)**:
   - Browsers forbid raw L2/L3 sockets, raw ICMP to external hosts, and non-CORS TCP connections.
   - `crates/wasmnux-net` implements an in-WASM user-mode switch (`10.0.2.0/24`, guest `10.0.2.15`, gateway `10.0.2.2`, DNS `10.0.2.3`) handling Ethernet, ARP, DHCP, ICMP echo to `10.0.2.2`/`10.0.2.3`, DNS (`apt.wasmnux.local` -> `10.0.2.2`), and TCP port 80 HTTP/1.1 bridged through `src/net/network-bridge.ts` (`fetch()` with SSRF/private-IP blocking).
   - Same-origin `/debian-repo/*` serves a valid Debian `trixie` `riscv64` APT repository out of the box without CORS proxies; external HTTP endpoints require CORS headers (or an optional WebSocket relay).
2. **`Cross-Origin-Opener-Policy` / `Cross-Origin-Embedder-Policy`**:
   - Vite dev/preview sets `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp` so `SharedArrayBuffer` and `crossOriginIsolated` are enabled where supported.
   - WASMNux **never requires** `SharedArrayBuffer`: the Worker RPC transport uses transferable/cloned buffers so deployments on static hosts without custom headers (e.g., GitHub Pages) continue to boot Linux cleanly.
3. **Static Server `.gz` Header Caveat**:
   - Guest images (`*.gz`) are decompressed explicitly via `DecompressionStream('gzip')` (`decompressGzipIfNeeded` auto-detects whether the static host already transparently gunzipped via `Content-Encoding: gzip`). APT metadata uses `Packages` and `Packages.xz` in `public/debian-repo/dists/trixie/Release` so `apt-get update` SHA-256 verification succeeds regardless of static server `Content-Encoding` rules.

---

## Documentation Index

- [`TODO.MD`](./TODO.MD) — Master Phase 0–12 engineering checklist & acceptance criteria.
- [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) — Architecture Decision Document, memory map, MMU/VirtIO/storage/GPU/network design.
- [`docs/SECURITY.md`](./docs/SECURITY.md) — Browser/WASM/Worker/OPFS/import/network threat model & security boundaries.
- [`docs/BENCHMARKS.md`](./docs/BENCHMARKS.md) — Reproducible cold-boot, MIPS, block I/O, RTT, and snapshot benchmarks.
- [`docs/TEST_REPORT.md`](./docs/TEST_REPORT.md) — Full Phase 0–12 automated test evidence ledger.
- [`docs/GITHUB_PAGES.md`](./docs/GITHUB_PAGES.md) — Static deployment to GitHub Pages, sub-path asset resolution, and local verification.
- [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md) & [`LICENSE`](./LICENSE) — Apache-2.0 license & upstream attributions.
