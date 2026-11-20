# Guest Asset Provenance and Integrity

WASMNux ships prebuilt guest payloads so that the browser VM can boot without a
guest-side build step. This document records where each payload comes from, how it
can be re-derived, and its exact digest.

`public/assets-manifest.json` is the machine-readable form of the table below and is
verified by `tests/unit/assets-manifest.test.ts`, which recomputes SHA-256 for every
listed file and fails on any drift. Regenerate it after changing an asset:

```bash
node tools/hash-assets.mjs          # rewrite the manifest
node tools/hash-assets.mjs --check  # verify without writing
```

The WASM runtime (`public/wasm/wasmnux_wasm.wasm`) is **not** listed: it is rebuilt
from source by `scripts/build-wasm.sh` on every build, so it is not an immutable
input. Guest assets are never rebuilt by `npm run build`.

## Kernel — `images/vmlinux-7.3.1-riscv64.Image.gz`

| Field        | Value                                                                                                                         |
| ------------ | ----------------------------------------------------------------------------------------------------------------------------- |
| Source       | `https://github.com/gregkh/linux.git`                                                                                         |
| Tag          | `v7.3.1`                                                                                                                      |
| Tag object   | `dfc1145bbaa1c5dd626fd25d9ae8fc277864e7bd` (verified via GitHub API on 2026-10-31)                                            |
| Architecture | `riscv64` (`ARCH=riscv`), RV64GC, `lp64d`                                                                                     |
| Build script | `scripts/build-kernel.sh` (fails if `make kernelversion` != `7.3.1`)                                                          |
| Config       | `configs/linux-7.3.1-riscv64.config`                                                                                          |
| SHA-256      | `feedb149461d505b4dc22a230b5b5585174ce7869bb679ee574ba23d290e1516`                                                            |
| Bytes        | 3,495,318 (gzip); uncompressed `Image` 6,547,296 B, sha256 `2b7647952d593e2873d7800634322b4bd7434ec88948a44cf50ac64482f737be` |

The uncompressed image is a valid RISC-V Linux `Image`: it carries `RISCV` at offset
`0x30` and the `RSC\x05` magic at offset `0x38`, and it reports
`Linux version 7.3.1` at boot. `crates/wasmnux-riscv/src/boot.rs` validates that
header before loading.

**This asset is now built from source in this environment.** The tree is fetched
from `gregkh/linux` at `v7.3.1` and compiled with Clang `23.1.3` and LLD
(`LLVM=1`, `ARCH=riscv`). The build host has no package manager, so
`scripts/build-kernel.sh` does not require flex, bison, bc, or dtc: it resolves
the configuration with `scripts/gen-kconfig.py` (kconfiglib), generates
`timeconst.h` with `scripts/gen-timeconst.py`, and synthesises
`drivers/of/empty_root.dtb` with `scripts/gen-empty-root-dtb.py`. The resulting
`Image` boots the full Phase 5 suite (`tests/unit/vanilla-linux.test.ts`):
31 BusyBox applets, `/proc`, `/sys`, `/dev/fb0`, VirtIO networking, `reboot`,
`ext4` persistence across reboot, and clean `poweroff`.

The release string is exactly `7.3.1` with no `-dirty` suffix. That detail
matters: `CONFIG_LOCALVERSION_AUTO=y` appends `-dirty` when the source tree has
uncommitted changes, so `gen-kconfig.py` restores every Kconfig file it rewrites
before the kernel computes `kernel.release`.

Four config choices keep the image small and fast on an interpreter. All four
were found by measuring `totalSteps` at the point the guest prints
`Linux version`, comparing a from-source build against the previously shipped
image (which retired ~21M instructions before that line, versus ~66M for a naive
build):

| Setting                                                                                                         | Why                                                                                                                                                                                               |
| --------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CONFIG_KERNEL_UNCOMPRESSED=y`                                                                                  | The runtime loads a raw `Image`, so an in-guest decompressor is pure overhead. `KERNEL_GZIP` also links all six decoders (gzip/bzip2/lzma/xz/lzo/lz4/zstd) because the helpers are unconditional. |
| `# CONFIG_STRICT_KERNEL_RWX is not set`                                                                         | It sets `SECTION_ALIGN` to 2 MiB in `arch/riscv/include/asm/set_memory.h`, injecting ~5 MB of alignment padding into the image. The guest never remaps kernel text.                               |
| `CONFIG_INIT_STACK_NONE=y`, `# CONFIG_STACKPROTECTOR is not set`, `# CONFIG_RANDOMIZE_KSTACK_OFFSET is not set` | Per-call hardening. `INIT_STACK_ALL_ZERO` is a _choice_ defaulting to `y` whenever the compiler supports `-ftrivial-auto-var-init=zero`, so it must be overridden by selecting the `none` member. |
| `# CONFIG_SLUB_DEBUG is not set`                                                                                | Red-zoning and sanity checks on every `kmalloc`; boot is allocation-heavy.                                                                                                                        |

The linker script is also deleted before each build
(`rm -f $OUT/arch/riscv/kernel/vmlinux.lds`). It is generated from the Kconfig
headers but the dependency is not always tracked, so a stale copy from an
earlier config silently reintroduces the 2 MiB padding no matter what `.config`
says — that cost a full debugging cycle.

## Vanilla initramfs — `images/vanilla-initramfs.cpio.gz`

| Field     | Value                                                                             |
| --------- | --------------------------------------------------------------------------------- |
| Built by  | `scripts/build-vanilla-rootfs.sh`                                                 |
| Userspace | static BusyBox 1.37.0 (`riscv64-linux-musl`), built by `scripts/build-busybox.sh` |
| SHA-256   | `0775164a75f56050db382346b2233bc841f6c68380df140621534e7d6cac189d`                |
| Bytes     | 814,278 (gzip, 795.2 KiB)                                                         |

## Vanilla persistent disk — `images/vanilla-disk.ext4.gz`

| Field      | Value                                                              |
| ---------- | ------------------------------------------------------------------ |
| Built by   | `scripts/build-vanilla-rootfs.sh` (`mke2fs`)                       |
| Filesystem | ext4, label `wasmnux-vanilla`, 64 MiB                              |
| SHA-256    | `a4741cfa776b4ef1efa4726bf8b451b49e13e4c49bc2cc10b5dfe37a5ecba9fd` |
| Bytes      | 71,402 (gzip, 69.7 KiB)                                            |

## Debian rootfs — `images/debian-rootfs.ext4.gz`

| Field        | Value                                                                                        |
| ------------ | -------------------------------------------------------------------------------------------- |
| Source       | `debuerreotype/docker-debian-artifacts`, branch `dist-riscv64`, `trixie/slim`                |
| Suite        | Debian 13 "trixie", `debian_version` `13.7` (checked against `trixie/rootfs.debian_version`) |
| Userspace    | glibc 2.41, `dpkg` 1.22.22, `apt` 3.0.3, `bash` (verified inside the booted guest)           |
| Build script | `scripts/build-debian-rootfs.sh` (pins SHA-256 of `rootfs.tar.gz`)                           |
| SHA-256      | `adb1678f414eaffd45c25f49f1c5dd7c2cd9181868447f845edbcdb1ff057032`                           |
| Bytes        | 29,584,790 (gzip), 256 MiB uncompressed                                                      |

## Bundled APT repository — `debian-repo/`

A four-package repository built by `scripts/build-debian-rootfs.sh` from C sources
compiled with the riscv64 guest toolchain, plus `dpkg-scanpackages` metadata.

**Limitation (must not be overstated):** the guest's `/etc/apt/sources.list.d/debian.sources`
is renamed to `debian-upstream.sources.disabled` and replaced by
`/etc/apt/sources.list.d/wasmnux.list`, which points only at
`http://apt.wasmnux.local/debian-repo`. Installed packages therefore come from this
bundled repository. This proves that Debian's `apt`/`dpkg` work end-to-end against a
real HTTP index on the emulated NIC — it is **not** evidence of access to the official
Debian archive. See `docs/TEST_REPORT.md` (limitations) and `TODO.MD`.

| Package                              | Architecture | SHA-256                                                            |
| ------------------------------------ | ------------ | ------------------------------------------------------------------ |
| `wasmnux-hello_1.0.0_riscv64.deb`    | riscv64      | `52c259f77eafe59a7719f4fafc57e1d5adb386c1ad51fea425d617f6d69efa9d` |
| `wasmnux-fbtools_1.0.0_riscv64.deb`  | riscv64      | `e29448e052ae79b1e799dccfe95374bdc9b3bb59c49e82e32b21753f182133ba` |
| `wasmnux-notes_1.0.0_all.deb`        | all          | `b6009abe207af77b138bfecc7e9a81fe290041e77048ad9cfdd00aa55138b477` |
| `wasmnux-sysbench_1.0.0_riscv64.deb` | riscv64      | `0ad2826b6c0d0c58640c82164239a0fd24acc70fad874437f5a8045e57b5b297` |

Repository index digests:

| File                                           | SHA-256                                                            |
| ---------------------------------------------- | ------------------------------------------------------------------ |
| `dists/trixie/Release`                         | `07dc29a91591b99e9e7613d96b32a924603d8697ced4f6f2b4967b0f45ae0c74` |
| `dists/trixie/main/binary-riscv64/Packages`    | `320d47c22b6485812296e943d0e0129f9fbd2c887d992048643b02cd9940d7c3` |
| `dists/trixie/main/binary-riscv64/Packages.xz` | `87de90d7e66776cde93094aa52f17f6e89ac922509ad8199ce78fa195bbd3b6d` |

The repository is signed-by-omission: `deb [trusted=yes]` is used, so apt does not
verify a Release signature. This is acceptable only because the repository is served
from the application's own origin; it is **not** a model for third-party repositories.

## BusyBox version

The shipped BusyBox is **1.37.0**, rebuilt from source in this environment rather
than carried over from a previous release.

`github.com/mirror/busybox` publishes no tag beyond `1_36_1`, so
`scripts/build-busybox.sh` builds from trunk with the target version pinned in
`WASMNUX_BUSYBOX_VERSION` (default `1.37.0`) and an optional
`WASMNUX_BUSYBOX_COMMIT`. The script reads `make kernelversion`, which reports a
trunk build as `1.37.0.git`, and accepts it with a prefix comparison. Pin a commit
via `WASMNUX_BUSYBOX_COMMIT` for a byte-reproducible rebuild.

The rebuild was executed end to end:

| Step                | Command                                           | Result                                                                                                                                                                                                                           |
| ------------------- | ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Static BusyBox      | `./scripts/build-busybox.sh`                      | `1,264,056` B ELF64 RISC-V static, no `PT_INTERP`, all 39 mandatory applets, sha256 `e6c7a83b5b3caf7fe3197421ea85934c6bf35d87762552bb19a3d939e802befc`                                                                           |
| Initramfs + disk    | `./scripts/build-vanilla-rootfs.sh`               | `vanilla-initramfs.cpio.gz` (`796` KiB, sha256 `0775164a75f56050db382346b2233bc841f6c68380df140621534e7d6cac189d`), `vanilla-disk.ext4.gz` (`70` KiB, sha256 `a4741cfa776b4ef1efa4726bf8b451b49e13e4c49bc2cc10b5dfe37a5ecba9fd`) |
| Applet + boot suite | `npx vitest run tests/unit/vanilla-linux.test.ts` | PASS — the initramfs is confirmed to contain `BusyBox v1.37`                                                                                                                                                                     |

Reproducing the build needs a static `riscv64-linux-musl` cross toolchain; the repo
standard is Zig, which `scripts/setup-toolchains.sh` installs. `-DBB_GLOBAL_CONST=`
is still required because BusyBox 1.37.0 assumes writable globals.
