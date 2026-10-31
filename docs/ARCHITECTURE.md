# WASMNux Architecture Decision Document

## 1. Repository Audit & Baseline Inventory

At project start (`c0237518262da49dd1941669eb47ff8d9e398e16`), `/home/user/WASMNux` contained only an Apache-2.0 `LICENSE` file and an initialized Git repository on branch `arena/5741c179-wasmnux`. No prior Rust, TypeScript, WebAssembly, or Linux artifacts existed.

Host build environment baseline:
- **Host OS:** Debian GNU/Linux 12 (`x86_64`), Linux `6.1.158+`, 2 vCPUs, 3.8 GiB RAM, 20 GiB free disk.
- **Preinstalled Host Tools:** `gcc 12.2.0`, `make 4.3`, `perl 5.36`, `python3 3.11.2`, `node v22.22.3`, `npm 10.9.8`, `e2fsprogs 1.47.0` (`/usr/sbin/mke2fs`, `/usr/sbin/debugfs`, `/usr/sbin/e2fsck`), `dpkg-deb` & `dpkg-scanpackages 1.21.23`, `git`, `gh`.
- **Provisioned Build Toolchains (`scripts/setup-toolchains.sh`):**
  - **Rust `1.88.0` stable** (`rustc`, `cargo`, `rustfmt 1.8.0`, `clippy 0.1.88`, `rust-std` for `x86_64-unknown-linux-gnu` and `wasm32-unknown-unknown`, `llvm-tools`).
  - **LLVM / Clang `23.1.3`** (`clang`, `ld.lld`, `llvm-ar`, `llvm-nm`, `llvm-objcopy`, `llvm-objdump`, `llvm-readelf`, `llvm-strip`).
  - **Zig `0.17.0`** (`zig cc` cross-compiler with `riscv64-linux-musl` sysroot for static userspace utilities).
  - **Headless Chromium `153.0.0`** (`@sparticuz/chromium`) + **Playwright** for real browser end-to-end verification.

---

## 2. Feasibility Analysis: Linux Kernel Execution in WebAssembly

### 2.1 Why Direct Linux Kernel Compilation to `wasm32` Is Rejected

WebAssembly (`wasm32`) is a Harvard-architecture, stack-based virtual ISA without hardware page tables (MMU), supervisor/user privilege rings, hardware timer interrupts, arbitrary instruction-pointer jumps, `fork()` address-space cloning, or direct execution of native ELF binaries (`riscv64` / `x86_64`).

1. **Upstream Linux Kernel:** Mainline `torvalds/linux` and `gregkh/linux` (`v7.3.1`) have no `arch/wasm` port. Historical research forks (e.g., out-of-tree Linux Kernel Library / Emscripten ports) heavily patch outdated kernels, lack hardware virtual memory (`CONFIG_MMU=y`), and cannot execute unmodified upstream ELF binaries via `execve()`.
2. **Debian Requirement:** Official Debian stable (`13.7 "trixie"`) distributes precompiled native ELF binaries (`dpkg`, `apt`, `bash`, `glibc 2.41`) expecting a real MMU (`fork()`, `mmap()`, Copy-on-Write page faults) and standard Linux ABI syscalls.

**Conclusion:** Compiling the upstream Linux C kernel directly into `wasm32` is **not technically valid** for running unmodified upstream Linux and official Debian userspace. Instead, running a **full-system hardware virtual machine / CPU + MMU + VirtIO emulator compiled from Rust to `wasm32-unknown-unknown`** boots an unmodified upstream Linux kernel (`v7.3.1`) and unmodified official Debian `13.7` ELF userspace cleanly inside the browser.

---

## 3. Evaluation of Open-Source Emulators & Guest Architecture Selection

### 3.1 Candidate Guest CPU Architectures

| Architecture | Upstream Linux `7.3.1` Support | Official Debian `13.7 ("trixie")` Status | Emulator Complexity in Rust/WASM | Verdict |
|---|---|---|---|---|
| **`riscv64` (`RV64GC` / `lp64d`)** | First-class (`arch/riscv`) | **Official release architecture** (added as first-class in Debian 13; official rootfs in `debuerreotype/docker-debian-artifacts` `dist-riscv64`) | Clean, orthogonal fixed/compressed ISA (`RV64IMAFDC`), well-defined `Sv39` 3-level MMU, SBI firmware interface, VirtIO MMIO | **SELECTED** |
| `i386` (32-bit x86) | Legacy (`arch/x86`) | **Dropped as regular architecture in Debian 13** ("no official kernel and no Debian installer; partial legacy userland only") | Complex x86 segmentation, GDT/IDT/TSS, PIC/APIC, legacy PC I/O ports | Rejected (fails full official Debian 13 arch standard) |
| `amd64` (`x86_64`) | First-class (`arch/x86`) | Official release architecture | Very high decoding/JIT complexity, 4-level paging + x87/SSE2/AVX + ACPI/APIC; no mature pure-Rust browser-WASM `x86_64` system emulator exists | Rejected |
| `arm64` (`aarch64`) | First-class (`arch/arm64`) | Official release architecture | Large A64 ISA surface + GICv3 + ARM timer/PSCI complexity in pure WASM interpreter | Rejected in favor of `riscv64` |

### 3.2 Audit of Existing Open-Source Virtualization Engines

We audited five open-source projects at the source level:

1. **`copy/v86`** (BSD-2-Clause, Rust + JS): Mature browser x86 emulator with WASM JIT, **but restricted strictly to 32-bit `i386`** (no 64-bit `x86_64` support) and ~60% JavaScript device layer. Incompatible with official 64-bit Debian 13 kernels/installations.
2. **`d0iasm/rvemu`** (MIT, Rust, last updated 2024): Designed around `xv6`; incomplete Linux 6.x/7.x CSR/MMU semantics and tightly coupled to older `wasm-bindgen` DOM assumptions.
3. **`takahirox/riscv-rust`** (MIT, Rust, last updated 2023): Unmaintained since 2023; partial `F`/`D`/`A`/`C` instruction coverage marked "almost" in upstream documentation, risking silent `glibc 2.41` hard-float (`lp64d`) misexecution in `apt`/`dpkg`.
4. **`leesum1/RV64emu-rs`** (MIT, Rust): Embedded-oriented (`no_std`), external proc-macro dependencies (`enum_dispatch`, `bitfield-struct`), incomplete browser peripheral suite.
5. **`wkoszek/emuko`** (Apache-2.0, Rust, actively maintained in 2026): Clean, zero-dependency pure-Rust `RV64IMAFDC` core (`Hart`, `CsrFile`, `Sv39` page walker, 2-tier TLB, decode cache, `Clint`, `Plic`, `Uart16550`, `VirtualSbi`, `FdtBuilder`, deterministic snapshots) proven to boot Debian RISC-V kernels.

### 3.3 Architectural Decision & Critical Hardening Over `emuko`

We adopt and adapt the Apache-2.0 `RV64IMAFDC` CPU/CSR/CLINT/PLIC/FDT core concepts from `emuko` into our modular `crates/wasmnux-riscv` crate (attributed in `THIRD_PARTY_NOTICES.md`), while engineering major architectural fixes and new subsystems required for `wasm32` safety, Linux `7.3.1`, and Debian `13.7`:

1. **Strict `Sv39` MMU Privilege & Dirty-Bit Compliance (Fixing 4 Critical Bugs):**
   - **Bug 1 fixed (`PTE_U` isolation):** Enforce User vs. Supervisor page protection (`PTE_U` bit 4), Supervisor Mode Execution Prevention (`SMEP` — Supervisor cannot fetch `PTE_U=1` pages), and `sstatus.SUM` (Supervisor cannot read/write `PTE_U=1` pages when `SUM=0`, required by Linux `__copy_from_user` fault fixup tables).
   - **Bug 2 fixed (`sstatus.MXR`):** Permit loads on execute-only (`X=1, R=0`) pages when `sstatus.MXR=1`.
   - **Bug 3 fixed (`PTE_D` CoW integrity):** Gate `TLB_PERM_W` in the software TLB on `(pte & PTE_D) != 0` so the first store to a clean writable page (`W=1, D=0`) walks the page table and sets hardware `PTE_D=1` in RAM, preserving Linux `fork()` Copy-on-Write and `ext4` mmap integrity.
   - **Bug 4 fixed (PPN upper-bit masking & TLB tagging):** Mask Sv39 PPN strictly to bits `[53:10]` and tag TLB lookups with privilege mode + `SUM`/`MXR` state.
2. **32-Bit `wasm32` Address Bounds Safety:** Replace truncating `addr as usize` casts with `usize::try_from(addr)` so >4 GiB physical address probes trap cleanly as `MemoryOutOfBounds` on 32-bit `wasm32`.
3. **FPU Context & U-Mode Counter Spec Compliance:** Enforce `sstatus.FS` (`Off` traps with `IllegalInstruction`; FP writes mark `FS = Dirty`) and `mcounteren`/`scounteren` `rdtime`/`rdcycle`/`rdinstret` access for zero-trap Linux vDSO `clock_gettime()`.
4. **`WFI` Idle Fast-Forward:** Advance `mtime` directly toward `mtimecmp` (bounded by quantum) or yield to the Web Worker event loop on `wfi` when no IRQ is pending, eliminating idle spin waste.
5. **New Virtual Subsystems Built from Scratch in Rust:**
   - `crates/wasmnux-virtio`: Spec-compliant **VirtIO MMIO v2** split-virtqueue engine (`VirtioBlk`, `VirtioNet`, `VirtioRng`).
   - `crates/wasmnux-storage`: Sparse 64 KiB Copy-on-Write block disk with CRC32 integrity, atomic commit generations, snapshots, and validated `.wasmnux-disk` import/export.
   - `crates/wasmnux-net`: Pure-Rust user-mode virtual network switch (`10.0.2.0/24`, ARP, ICMPv4 ping, DHCPv4, UDP DNS, stateful TCPv4 HTTP bridge).
   - `SimpleFb` (`0x5000_0000`, `640x400` `XRGB8888` with dirty-scanline tracking), `GoldfishRtc` (`0x0010_1000`), `Syscon` (`0x0010_0000` poweroff/reboot), and expanded `SBI v2.0` (`BASE`, `TIME`, `IPI`, `RFENCE`, `HSM`, `SRST`, `DBCN`).

---

## 4. Virtual Machine Physical Memory Map (`riscv64` `virt`-Compatible)

| Physical Address Range | Size | Device / Subsystem | IRQ / Interface |
|---|---|---|---|
| `0x0010_0000 .. 0x0010_0FFF` | 4 KiB | `sifive_test` / `syscon` (`0x5555` poweroff, `0x7777` reboot) | MMIO |
| `0x0010_1000 .. 0x0010_1FFF` | 4 KiB | `google,goldfish-rtc` wall-clock UTC Real-Time Clock | MMIO |
| `0x0200_0000 .. 0x0200_FFFF` | 64 KiB | `riscv,clint0` Core-Local Interruptor (`msip`, `mtimecmp`, `mtime`) | Timer / IPI |
| `0x0C00_0000 .. 0x0FFF_FFFF` | 64 MiB | `sifive,plic-1.0.0` Platform-Level Interrupt Controller | `SEIP` |
| `0x1000_0000 .. 0x1000_00FF` | 256 B | `ns16550a` Serial Console UART (`/dev/ttyS0`) | PLIC IRQ `10` |
| `0x1000_1000 .. 0x1000_1FFF` | 4 KiB | `virtio,mmio` v2 Block Device (`VirtioBlk` `/dev/vda`) | PLIC IRQ `1` |
| `0x1000_2000 .. 0x1000_2FFF` | 4 KiB | `virtio,mmio` v2 Network NIC (`VirtioNet` `eth0`) | PLIC IRQ `2` |
| `0x1000_3000 .. 0x1000_3FFF` | 4 KiB | `virtio,mmio` v2 Hardware RNG (`VirtioRng` `/dev/hwrng`) | PLIC IRQ `3` |
| `0x5000_0000 .. 0x500F_FFFF` | 1 MiB | `simple-framebuffer` (`640x400x4`, `x8r8g8b8`, `/dev/fb0` + `fbcon`) | Dirty-scanline VRAM |
| `0x8000_0000 .. 0x8000_0000 + RAM` | 128–512 MiB | Main Guest DRAM (Kernel `Image` @ `0x8020_0000`, Initrd & FDT near top) | Cached RAM |

---

## 5. Runtime, Storage, Graphics & Networking Architecture

```
┌────────────────────────────────────────────────────────────────────────────┐
│                        Main Browser Thread (TypeScript)                    │
│  ┌──────────────┐  ┌──────────────────────┐  ┌──────────────────────────┐  │
│  │ Profile & VM │  │ xterm.js Interactive │  │ WebGPU Compute+Render    │  │
│  │ Controls UI  │  │ Console (/dev/ttyS0) │  │ Pipeline (+ Canvas2D FB) │  │
│  └──────┬───────┘  └──────────┬───────────┘  └────────────▲─────────────┘  │
│         │                     │                           │ Dirty Scanlines │
│  ┌──────▼─────────────────────▼───────────────────────────┴─────────────┐  │
│  │ VmClient Orchestrator, Capability Detector, Diagnostics, Snapshots   │  │
│  └──────┬─────────────────────────────────────────────────▲─────────────┘  │
└─────────┼─────────────────────────────────────────────────┼────────────────┘
          │ Validated Structured Worker RPC                 │
┌─────────▼─────────────────────────────────────────────────┼────────────────┐
│                     Dedicated Web Worker (`vm.worker.ts`)                  │
│  ┌─────────────────────────┐       ┌────────────────────────────────────┐  │
│  │ Persistent Block Store  │       │ Browser Network Bridge (`fetch` /  │  │
│  │ OPFS + IDB 2-Phase Jrnl │       │ `/debian-repo` + CORS/WS Relay)    │  │
│  └────────────▲────────────┘       └──────────────────▲─────────────────┘  │
│               │ 64 KiB Dirty Chunks                   │ HTTP Req/Resp      │
│  ┌────────────┴───────────────────────────────────────┴─────────────────┐  │
│  │              `wasmnux-wasm` (`wasm32-unknown-unknown`)               │  │
│  │  ┌────────────────┐ ┌────────────────┐ ┌──────────────┐ ┌─────────┐  │  │
│  │  │ wasmnux-riscv  │ │ wasmnux-virtio │ │ wasmnux-stor │ │ wasmnux │  │  │
│  │  │ RV64GC + Sv39  │ │ Blk / Net /Rng │ │ Sparse CoW   │ │  -net   │  │  │
│  │  └────────────────┘ └────────────────┘ └──────────────┘ └─────────┘  │  │
│  └──────────────────────────────────────────────────────────────────────┘  │
│  Guest OS: [Profile 1: Vanilla Linux 7.3.1 + BusyBox]                      │
│            [Profile 2: Debian 13.7 "trixie" riscv64 + apt/dpkg + BusyBox]  │
└────────────────────────────────────────────────────────────────────────────┘
```

### 5.1 Linux Profiles & Provenance
1. **Vanilla Linux Profile:**
   - **Kernel:** Official upstream Linux stable **`7.3.1`** (`git://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git`, mirrored at `github.com/gregkh/linux`, tag `v7.3.1`, commit `dfc1145bbaa1c5dd626fd25d9ae8fc277864e7bd`), compiled from C source with Clang/LLVM 23.1.3 (`ARCH=riscv`).
   - **Userspace:** Official BusyBox **`1.36.1`** (`github.com/mirror/busybox` tag `1_36_1`), compiled statically for `riscv64-linux-musl` (`RV64GC` `lp64d`) with custom `/init` and an automatic `ext4` persistent disk (`/dev/vda`) mounted for durable `/root`, `/home`, and `/etc/persist` state.
2. **Debian Profile:**
   - **Distribution:** Official **Debian GNU/Linux `13.7` (`"trixie"`) `riscv64`** from Debian's official reproducible rootfs artifact repository (`github.com/debuerreotype/docker-debian-artifacts`, branch `dist-riscv64`, `DEBIAN_VERSION_FULL=13.7`), including `glibc 2.41`, `bash 5.2.37`, `coreutils 9.7`, `apt 3.0.3`, `dpkg 1.22.22`, plus `busybox` `riscv64`.
   - **Package Management:** Backed by a genuine `ext4` root filesystem on `/dev/vda` and a real standards-compliant `riscv64` Debian APT repository (`/debian-repo/dists/trixie/...`) served over `VirtioNet` (`eth0` -> Rust TCP/IP -> Browser `fetch` bridge), supporting real `apt update`, `apt-cache search/show`, `apt install`, `dpkg -l/-s/-L`, and `apt remove`, with full offline usability and cross-reload persistence.

### 5.2 Persistent Storage Design
- **Base vs. Overlay Separation:** Read-only compressed base disk images live in HTTP/PWA cache (`/images/*.gz`); per-profile mutable blocks (`64 KiB` chunks) live exclusively in **OPFS** (`/wasmnux/profiles/<profileId>/`) and **IndexedDB** (`wasmnux-storage-v1`).
- **Crash-Consistent Two-Phase Commit Journal:** Updates write chunk payloads + CRC32 checksums into staging slots first, then atomically flip the active commit generation record in metadata. Interrupted sessions automatically discard uncommitted partial writes and recover the last verified generation.

### 5.3 WebGPU Framebuffer Pipeline & Fallback
- **WebGPU Mode:** Acquires `GPUAdapter` + `GPUDevice`, uploads dirty guest VRAM scanlines (`XRGB8888` at `0x5000_0000` driven by Linux `simplefb` + `fbcon` `/dev/fb0`) to a storage buffer, executes a **WGSL `@compute` shader** performing GPU color-channel swizzling (`BGRX -> RGBA8`), alpha normalization, and optional CRT scanline filtering into an `rgba8unorm` storage texture, and presents via a **WGSL `@vertex`/`@fragment` pipeline** with `device.lost` recovery.
- **Fallback Mode:** `Canvas2DRenderer` converts dirty scanlines into `ImageData` and renders via `ctx.putImageData()`, ensuring full usability when WebGPU is absent or disabled.

---

## 6. Known Technical & Environmental Limitations

1. **Browser Sandbox Networking:** Browsers prohibit raw L2/L3 sockets, raw ICMP, and arbitrary outbound TCP sockets. Our `wasmnux-net` stack terminates Ethernet/ARP/ICMP/DHCP/DNS/TCP inside WASM and bridges HTTP requests via browser `fetch()` (subject to CORS for external servers unless routed through an optional WebSocket proxy). Same-origin `/debian-repo` works out-of-the-box with zero external proxy needed.
2. **Interpreter Throughput vs. Hardware KVM:** WebAssembly does not expose host hardware virtualization (`KVM`) or executable RWX memory mappings in standard linear memory without dynamic WASM module compilation. Our cached RV64GC interpreter + software TLB + `WFI` fast-forward achieves responsive interactive shells and sub-few-second boots, but CPU-bound compilation workloads are slower than native KVM.
3. **WebGPU Scope:** WebGPU accelerates guest framebuffer conversion, scaling, and presentation (`/dev/fb0`), not guest OpenGL/Vulkan 3D command-stream passthrough.
4. **Browser Storage Eviction:** Unless `navigator.storage.persist()` is granted by the browser, OPFS and IndexedDB remain subject to browser LRU origin eviction under extreme host disk pressure; the UI explicitly probes and displays persistence grant status and quota usage.
5. **Build-Time Egress Environment:** In the build sandbox, direct egress is restricted to GitHub, npm, and PyPI hosts (`busybox.net` and `deb.debian.org` are unreachable directly from build scripts). Accordingly, kernel `7.3.1` is built from official `gregkh/linux` on GitHub, Debian `13.7` rootfs is retrieved from official `debuerreotype/docker-debian-artifacts` on GitHub, BusyBox `1.36.1` is built from official `mirror/busybox` on GitHub, and installable `riscv64` `.deb` repository packages are built reproducibly from real C sources using `zig cc` + `dpkg-deb` + `dpkg-scanpackages`.

---

## 7. Acceptance Criteria & Verification Matrix

| Requirement Area | Acceptance Standard | Verification Suite |
|---|---|---|
| Rust Virtualization Core | Zero clippy warnings, formatted, `RV64IMAFDC` + `Sv39` MMU + VirtIO + Net + Storage unit & mini-OS tests pass | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace` |
| WASM Runtime & Worker | Compiles to `wasm32-unknown-unknown`, strict TS types, zero UI-thread blocking, structured errors | `scripts/build-wasm.sh`, `npm run typecheck`, `npm run lint`, Vitest WASM tests |
| Vanilla Linux `7.3.1` | Upstream `7.3.1` boots to shell; all 35+ BusyBox applets behaviorally verified; `/dev/vda` `ext4` persists | Rust/Vitest VM boot & applet suite + Playwright E2E |
| Debian `13.7` (`trixie`) | Official `13.7` `riscv64` boots; `bash`, `BusyBox`, `dpkg`, `apt update/install/remove` verified & persistent | Vitest Debian integration suite + Playwright E2E |
| Persistent Storage | Survives page reload & browser restart; crash journal recovery; snapshots; import/export; quota handling | Storage fault-injection unit tests + Playwright persistent-context tests |
| WebGPU & Fallback | Real WGSL compute+render pipeline + `device.lost` recovery + Canvas 2D fallback verified | Renderer unit tests + Playwright WebGPU-on / WebGPU-off tests |
