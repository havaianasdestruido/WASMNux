# Dependencies — rationale, provenance, licenses

Policy: every dependency earns its place here (necessity, maintenance,
license, WASM/browser compatibility). Transitive lock files:
`Cargo.lock` (Rust) and `web/package-lock.json` (web).

## Project code (this repository)

Apache-2.0 throughout. The Rust emulator is a clean-room, spec-driven
implementation (RISC-V unprivileged + privileged ISAs); public emulator
projects (`rvemu`, `riscv-rust`) were evaluated as design references only —
no code was copied (see `docs/ARCHITECTURE.md` §2 for why they are not
dependencies: maintenance/completeness).

## Rust (build-time; compiled into the WASM module)

| Crate | Where | Why it exists | License |
|---|---|---|---|
| `wasm-bindgen` | `crates/wasmnux-wasm` only | The standard, maintained Rust↔JS binding generator. The emulator core has **zero** dependencies. | MIT / Apache-2.0 |

Tooling: Rust stable (MSRV 1.87), `wasm-bindgen-cli` pinned to the
`Cargo.lock` version by `scripts/build-wasm.sh` and CI.

## Web (npm)

| Package | Use | License |
|---|---|---|
| `@xterm/xterm`, `@xterm/addon-fit` | The guest terminal. Industry-standard, handles ANSI/VT correctly so we never parse terminal output ourselves. | MIT |
| `vite` | Dev server + bundler (worker support, COOP/COEP headers). | MIT |
| `typescript` | Strict type checking (incl. `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`). | Apache-2.0 |
| `vitest` | Unit tests (storage, protocol, profiles, renderers). | MIT |
| `@playwright/test` | Real-browser e2e (persistence across reload, WebGPU/Canvas paths). | Apache-2.0 |
| `eslint`, `typescript-eslint`, `@eslint/js`, `prettier` | Static analysis / formatting gates (zero-warning policy). | MIT |
| `@webgpu/types`, `@types/node` | Type definitions only. | MIT / BSD-style |

## Guest software (unmodified upstream binaries; built by `guest/`, not linked into our code)

| Component | Source (resolved at build time) | License | Notes |
|---|---|---|---|
| Linux kernel | `cdn.kernel.org` latest *stable* (`versions.sh` → 7.3.1 on 2026-10-31), sha256 verified from `sha256sums.asc` | GPL-2.0-only (with syscall note) | Built for riscv64 with `guest/kernel/config.fragment` over upstream defconfig. |
| BusyBox | `busybox.net/downloads` latest stable | GPL-2.0 | Static soft-float (`-march=rv64imac_zicsr -mabi=lp64`) until F/D lands. |
| OpenSBI | GitHub `riscv-software-src/opensbi` latest release | BSD-2-Clause | Generic platform, `FW_JUMP_ADDR=0x80200000`. |
| Debian | `deb.debian.org` stable suite (13 trixie on 2026-10-31), debootstrap with signature-checking keyring | DFSG (per package) | riscv64 is officially supported from Debian 13. First-boot seed; apt needs the Phase-9 relay for network repos (offline dpkg works). |

## Evaluation notes for things deliberately NOT used

- **v86** (BSD-2, very maintained): 32-bit x86 only — cannot run the
  current Debian (13 dropped the i386 kernel). Fails a hard requirement.
- **RVVM** (GPL-3.0): license conflicts with this repo's Apache-2.0; also C.
- **`qemu-wasm`**: research-grade TCG→WASM; huge C surface to audit.
- **CheerpX**: proprietary. Auditability is a hard requirement.
- **`takahirox/riscv-rust` / `d0iasm/rvemu`** (MIT, Rust): unmaintained
  (2023) / Linux-incomplete. Forking either into a bootable current-kernel
  core would be a rewrite of the same CPU we implement here; referenced
  for machine-model knowledge only.
