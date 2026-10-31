# Third-Party Notices and Provenance

WASMNux is licensed under the **Apache License, Version 2.0** (see `LICENSE`).

## 1. Virtualization Core Attribution

- **`emuko` (`https://github.com/wkoszek/emuko`)**
  - Author: Wojciech A. Koszek and contributors
  - License: Apache License, Version 2.0
  - Usage: Portions of `crates/wasmnux-riscv` (RV64IMAFDC instruction decoder/interpreter structure, CSR register file, CLINT, PLIC, FDT builder, and SBI scaffolding) are adapted from `emuko` with architectural modifications for `wasm32-unknown-unknown` bounds safety, `Sv39` `PTE_U`/`SUM`/`MXR`/`PTE_D` spec compliance, `sstatus.FS` dirty tracking, `WFI` timer fast-forward, Goldfish RTC, Syscon, and SimpleFB VRAM integration.

## 2. Guest Operating System Artifacts (Separate Guest Images)

Guest operating system images executed inside the virtual machine are separate guest software distributions and are **not** linked into the WASMNux Rust/WASM or TypeScript runtime:

- **Linux Kernel `7.3.1`**
  - Source: Official Linux Stable git tree (`https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git` / `https://github.com/gregkh/linux`, tag `v7.3.1`, commit `dfc1145bbaa1c5dd626fd25d9ae8fc277864e7bd`)
  - License: GNU General Public License v2.0 only (`GPL-2.0-only` with Linux-syscall-note)
  - Reproducible build script: `scripts/build-kernel.sh` (`configs/linux-7.3.1-riscv64.config`)

- **BusyBox `1.36.1`**
  - Source: Official BusyBox git mirror (`https://busybox.net/` / `https://github.com/mirror/busybox`, tag `1_36_1`)
  - License: GNU General Public License v2.0 (`GPL-2.0-only`)
  - Reproducible build script: `scripts/build-busybox.sh`

- **Debian GNU/Linux `13.7` ("trixie") `riscv64`**
  - Source: Official Debian Docker/OCI reproducible rootfs repository (`https://www.debian.org/` / `https://github.com/debuerreotype/docker-debian-artifacts`, branch `dist-riscv64`, `DEBIAN_VERSION_FULL=13.7`)
  - Licenses: DFSG-compliant licenses per `/usr/share/doc/*/copyright` inside the Debian root filesystem (`dpkg`, `apt`, `bash`, `coreutils`, `glibc`, etc.)
  - Reproducible build script: `scripts/build-debian-image.sh`

## 3. Browser UI Libraries

- **`@xterm/xterm`** and **`@xterm/addon-fit`** (`https://github.com/xtermjs/xterm.js`) — MIT License.
