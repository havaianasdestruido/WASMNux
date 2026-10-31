# Guest and tool provenance

Recorded 2026-10-31. Hashes are SHA-256 of the exact artifacts shipped in `web/public/assets/manifest.json`.

| Component | Version / revision | Source | License | Notes |
|---|---|---|---|---|
| Linux kernel | stable v7.3.1, commit `79a907609d53d5cd645b42c11550d295d43abf99` | https://github.com/gregkh/linux.git | GPL-2.0 | Unmodified upstream source; RV64 defconfig-derived config in `tools/kernel.config`; signature verification not performed (no keyring available). |
| OpenSBI | v1.6 `bd613dd9` | https://github.com/riscv-software-src/opensbi.git | BSD-2-Clause | `fw_jump` built for the WASMNux virt board; local objcopy compatibility patch only. |
| BusyBox | 1.37.0 (`1_37_0`) | https://github.com/vda-linux/busybox_mirror | GPL-2.0 | Static musl RV64 build; config/source retained in research workspace, not all applets behaviorally verified yet. |
| Debian rootfs | 13.7 trixie riscv64, Debuerreotype `dist-riscv64@64a870d7670389268c0d44ec0e0a4392d36d579c` | https://github.com/debuerreotype/docker-debian-artifacts | DFSG-free aggregate | Layer `stable/oci/blobs/rootfs.tar.gz`, sha256 `9d924bd60c86d7c78accc40bd01b73b855edec5d6d502ec982d0f667445094c9`, 47,891,541 bytes; matches OCI index digest. |
| Rust emulator | `evanphx/riscv-emu@0a320bc4d8009eec2438b7544747c22aa121781f` | https://github.com/evanphx/riscv-emu | MIT | Vendored subset; see `vendor/PATCHES.md`. |
| Rust toolchain | 1.83.0-dev (`90b35a623 2024-11-26`) | https://github.com/msft-mirror-aosp/platform.prebuilts.rust | Apache-2.0/MIT | AOSP prebuilt used because static.rust-lang.org was unreachable; `RUSTC_BOOTSTRAP=1` for `-Z build-std`. |
| LLVM/LLD | 20.0.0 (`clang-r547379`) | https://github.com/msft-mirror-aosp/platform.prebuilts.clang.host.linux-x86 | Apache-2.0 with LLVM exceptions | objcopy/lld/objdump for kernel/OpenSBI builds. |
| Zig | 0.13.0 | https://ziglang.org/download/ | MIT | musl cross-compilation for BusyBox. |
| Chromium (test) | @sparticuz/chromium 153.0.0 | npm registry | Apache-2.0 + bundled notices | Test-only; Playwright browser download was blocked in the sandbox. |

## Reproducing guest artifacts

1. Clone the pinned kernel/OpenSBI/BusyBox revisions above.
2. Build OpenSBI `PLATFORM=generic FW_JUMP=yes` with the LLVM tools; build Linux with `tools/kernel.config` and `tools/build-kernel.sh`; build BusyBox static with `tools/build-busybox.sh`.
3. Generate the vanilla initramfs/disk with `tools/make-initramfs.py` and `tools/make-rootfs.sh`.
4. Fetch the pinned Debian OCI layer, verify its SHA-256, extract, add `/wnx-init.sh` and the demo `.deb`, then create a 256 MiB ext2 image (`mkfs.ext2 -F -d ...`).
5. `cargo build -p wasmnux-runtime --target wasm32-unknown-unknown -Z build-std=std,panic_abort --release`, copy the WASM, gzip the disk images as `.imgz`, and run `node tools/hash-assets.mjs`.

Clean-room double-build hash equality and signed-tag verification remain open (TODO P12.2).
