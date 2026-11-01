# Third-Party Notices and Provenance

WASMNux is licensed under the **Apache License, Version 2.0** (see `LICENSE`).

## 1. Rust / WebAssembly Virtualization Core (`Cargo.toml`)

- **External `crates.io` dependencies:** **None (`0` external crates)** across `wasmnux-storage`, `wasmnux-net`, `wasmnux-riscv`, `wasmnux-virtio`, and `wasmnux-wasm`.
- **`emuko` (`https://github.com/wkoszek/emuko`)**
  - Author: Wojciech A. Koszek and contributors
  - License: Apache License, Version 2.0
  - Usage: Portions of `crates/wasmnux-riscv` (initial RV64 instruction decode structure, CSR definitions, CLINT/PLIC register layout, and FDT scaffolding) were informed by / adapted from `emuko`, rewritten for zero-dependency `wasm32-unknown-unknown` safety, `Sv39` `ITLB`/`DTLB` fast-path translation (`PTE_U`/`SUM`/`MXR`/`PTE_A`/`PTE_D`), IEEE-754 `RV64FD`, `WFI` timer fast-forwarding, Goldfish RTC, Syscon poweroff/reboot, VirtIO MMIO v2 (`blk`, `net`, `rng`), and `SimpleFB` dirty-row tracking.

## 2. Guest Operating System Artifacts (Separate Guest Software Distributions)

Guest OS images executed inside the virtual machine are separate guest software distributions loaded as data at runtime and are **not** linked into the WASMNux Rust/WASM or TypeScript host code:

- **Linux Kernel `7.3.1` (`public/images/vmlinux-7.3.1-riscv64.Image.gz`)**
  - Source: Official Linux Stable git tree (`https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git` / `https://github.com/gregkh/linux.git`, tag `v7.3.1`, commit `79a907609d53d5cd645b42c11550d295d43abf99`)
  - License: `GPL-2.0-only` WITH `Linux-syscall-note`
  - Reproducible build script: `scripts/build-kernel.sh` (`configs/linux-7.3.1-riscv64.config`)

- **BusyBox `1.36.1` (`public/images/vanilla-initramfs.cpio.gz`)**
  - Source: Official BusyBox git mirror (`https://busybox.net/` / `https://github.com/mirror/busybox.git`, tag `1_36_1`, commit `1a64f6a20aaf6ea4dbba68bbfa8cc1ab7e5c57c4`)
  - License: `GPL-2.0-only`
  - Reproducible build scripts: `scripts/build-busybox.sh` and `scripts/build-vanilla-rootfs.sh`

- **Debian GNU/Linux `13.7` ("trixie") `riscv64` (`public/images/debian-rootfs.ext4.gz`)**
  - Source: Official Debian reproducible OCI rootfs artifacts (`https://www.debian.org/` / `https://github.com/debuerreotype/docker-debian-artifacts.git`, branch `dist-riscv64`, `DEBIAN_VERSION_FULL=13.7`, tarball SHA-256 `a81ae23cfcc12d03cee9e3704fa559c027af15fefe8c60924add8fd17e4c98c1`)
  - Licenses: DFSG-compliant package licenses documented inside `/usr/share/doc/*/copyright` on the Debian root filesystem (`apt 3.0.3`, `dpkg 1.22.22`, `glibc 2.41`, `bash 5.2.37`, `coreutils 9.7`, `util-linux 2.41.5`, etc.)
  - Reproducible build script: `scripts/build-debian-rootfs.sh`

## 3. Browser Runtime Dependencies (`package.json` `dependencies`)

| Package | Version | License | Purpose |
|---|---|---|---|
| `@xterm/xterm` | `^6.0.0` | MIT | Interactive ANSI/VT100 terminal widget connected to guest NS16550A UART |
| `@xterm/addon-fit` | `^0.11.0` | MIT | Viewport-aware terminal column/row calculation |

## 4. Build & Test Tooling (`package.json` `devDependencies` — Not Shipped in Production Bundle)

| Package | Version | License | Purpose |
|---|---|---|---|
| `typescript` | `~5.8.3` | Apache-2.0 | Strict TypeScript typechecker |
| `vite` | `^6.4.5` | MIT | ES module dev server & production bundler |
| `vitest` | `^5.0.5` | MIT | Unit, integration, security, and benchmark test runner |
| `@playwright/test` | `^1.64.1` | Apache-2.0 | End-to-end Chromium browser automation |
| `@sparticuz/chromium` | `^153.0.0` | Apache-2.0 | Headless Chromium 153 binary with SwiftShader WebGPU |
| `eslint` / `@eslint/js` / `typescript-eslint` | `^10.14.0` / `^10.0.1` / `^8.72.1` | MIT | `strictTypeChecked` static analysis (`overrides.minimatch@^11.0.3`) |
| `prettier` | `^3.5.3` | MIT | Deterministic code formatting |
| `fake-indexeddb` | `^6.2.7` | Apache-2.0 | IndexedDB W3C spec test polyfill for Node unit tests |
| `@webgpu/types` | `^0.1.74` | BSD-3-Clause | Official W3C WebGPU TypeScript definitions |
| `@types/node` | `^22.15.0` | MIT | Node.js TypeScript definitions for test/build scripts |
