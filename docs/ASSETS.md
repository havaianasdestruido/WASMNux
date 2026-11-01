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

| Field | Value |
| --- | --- |
| Source | `https://github.com/gregkh/linux.git` |
| Tag | `v7.3.1` |
| Tag object | `dfc1145bbaa1c5dd626fd25d9ae8fc277864e7bd` (verified via GitHub API on 2026-10-31) |
| Architecture | `riscv64` (`ARCH=riscv`), RV64GC, `lp64d` |
| Build script | `scripts/build-kernel.sh` (fails if `make kernelversion` != `7.3.1`) |
| Config | `configs/linux-7.3.1-riscv64.config` |
| SHA-256 | `30954ed52d898eb6d18b4d1cfc0f9bd50e23598a0c8a243b7947b916317fb5ba` |
| Bytes | 3,314,317 (gzip) |

The uncompressed image is a valid RISC-V Linux `Image`: it carries `RISCV` at offset
`0x30` and the `RSC\x05` magic at offset `0x38`, and it reports
`Linux version 7.3.1` at boot. `crates/wasmnux-riscv/src/boot.rs` validates that
header before loading.

## Vanilla initramfs — `images/vanilla-initramfs.cpio.gz`

| Field | Value |
| --- | --- |
| Built by | `scripts/build-vanilla-rootfs.sh` |
| Userspace | static BusyBox 1.36.1 (`riscv64-linux-musl`), built by `scripts/build-busybox.sh` |
| SHA-256 | `b4d4aa718ddb2d5ebcc9ffbfe4867b4ffade4dd004b096d9011a03bb13e91807` |
| Bytes | 811,224 (gzip) |

## Vanilla persistent disk — `images/vanilla-disk.ext4.gz`

| Field | Value |
| --- | --- |
| Built by | `scripts/build-vanilla-rootfs.sh` (`mke2fs`) |
| Filesystem | ext4, label `wasmnux-vanilla`, 64 MiB |
| SHA-256 | `3b39afba43fd7ae1b8b10dfca40ef5e735ed68e17a37f296defbe08aabc58e39` |
| Bytes | 71,424 (gzip) |

## Debian rootfs — `images/debian-rootfs.ext4.gz`

| Field | Value |
| --- | --- |
| Source | `debuerreotype/docker-debian-artifacts`, branch `dist-riscv64`, `trixie/slim` |
| Suite | Debian 13 "trixie", `debian_version` `13.7` (checked against `trixie/rootfs.debian_version`) |
| Userspace | glibc 2.41, `dpkg` 1.22.22, `apt` 3.0.3, `bash` (verified inside the booted guest) |
| Build script | `scripts/build-debian-rootfs.sh` (pins SHA-256 of `rootfs.tar.gz`) |
| SHA-256 | `adb1678f414eaffd45c25f49f1c5dd7c2cd9181868447f845edbcdb1ff057032` |
| Bytes | 29,584,790 (gzip), 256 MiB uncompressed |

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

| Package | Architecture | SHA-256 |
| --- | --- | --- |
| `wasmnux-hello_1.0.0_riscv64.deb` | riscv64 | `52c259f77eafe59a7719f4fafc57e1d5adb386c1ad51fea425d617f6d69efa9d` |
| `wasmnux-fbtools_1.0.0_riscv64.deb` | riscv64 | `e29448e052ae79b1e799dccfe95374bdc9b3bb59c49e82e32b21753f182133ba` |
| `wasmnux-notes_1.0.0_all.deb` | all | `b6009abe207af77b138bfecc7e9a81fe290041e77048ad9cfdd00aa55138b477` |
| `wasmnux-sysbench_1.0.0_riscv64.deb` | riscv64 | `0ad2826b6c0d0c58640c82164239a0fd24acc70fad874437f5a8045e57b5b297` |

Repository index digests:

| File | SHA-256 |
| --- | --- |
| `dists/trixie/Release` | `07dc29a91591b99e9e7613d96b32a924603d8697ced4f6f2b4967b0f45ae0c74` |
| `dists/trixie/main/binary-riscv64/Packages` | `320d47c22b6485812296e943d0e0129f9fbd2c887d992048643b02cd9940d7c3` |
| `dists/trixie/main/binary-riscv64/Packages.xz` | `87de90d7e66776cde93094aa52f17f6e89ac922509ad8199ce78fa195bbd3b6d` |

The repository is signed-by-omission: `deb [trusted=yes]` is used, so apt does not
verify a Release signature. This is acceptable only because the repository is served
from the application's own origin; it is **not** a model for third-party repositories.

## BusyBox version

The shipped BusyBox is **1.36.1**. BusyBox 1.37.0 configuration from PR #5 was
reviewed but not adopted: bumping the userspace version requires rebuilding the
initramfs and the ext4 image with the guest toolchain
(`scripts/build-busybox.sh`, `scripts/build-vanilla-rootfs.sh`) and re-verifying the
applet behaviour suite. That rebuild is not reproducible in the current environment,
so the assets remain on 1.36.1 and the bump is tracked as open work in `TODO.MD`
rather than shipping an unverified asset.
