#!/usr/bin/env bash
# Builds a soft-float RISC-V musl cross toolchain (rv64imac / lp64) with
# musl-cross-make. Why: distribution glibc only ships hard-float (lp64d)
# headers/stubs, so a *soft-float* static BusyBox for our pre-F/D CPU
# cannot be built against it (gnu/stubs-lp64.h does not exist). musl is
# the standard answer for static soft-float RISC-V userlands.
#
# After the emulator's F/D milestone lands, this toolchain can be retired
# in favour of a plain lp64d glibc build.
#
# Usage: build-musl-toolchain.sh <out-dir>
set -euo pipefail

OUT_DIR="${1:?usage: build-musl-toolchain.sh <out-dir>}"
WORK_DIR="${WORK_DIR:-$(dirname "${OUT_DIR}")/musl-cross-work}"
# Pinned for reproducibility; bump deliberately.
MUSL_CROSS_MAKE_COMMIT="5e8042e63de29f8473fc3b0b831a587ba284e1a8"

mkdir -p "${WORK_DIR}"
if [ ! -d "${WORK_DIR}/musl-cross-make" ]; then
  git clone https://github.com/richfelker/musl-cross-make "${WORK_DIR}/musl-cross-make"
fi
cd "${WORK_DIR}/musl-cross-make"
git checkout --quiet "${MUSL_CROSS_MAKE_COMMIT}"

# GCC_VER: musl-cross-make's default is the ancient 9.4.0, whose libgcc
# fails to link against its own bundled binutils 2.44 for riscv64
# (observed in guest CI, run 41236543791: libgcc_s.so collect2 failure).
# 14.3.0 has patches shipped in-tree and matches binutils 2.44.
# LINUX_VER: the default reduced sabotage header set (4.19.88) lacks
# UAPI headers BusyBox needs (linux/kd.h — guest CI run after 4b0e627);
# 6.15.7 is the newest full-kernel version musl-cross-make ships a hash
# for, so headers come from a verified kernel.org tarball.
cat > config.mak <<EOF
TARGET = riscv64-linux-musl
OUTPUT = ${OUT_DIR}
GCC_VER = 14.3.0
LINUX_VER = 6.15.7
GCC_CONFIG += --with-arch=rv64imac --with-abi=lp64 --enable-languages=c
EOF

make -j"$(nproc)"
make install

"${OUT_DIR}/bin/riscv64-linux-musl-gcc" -v 2>&1 | tail -n 3
echo "Soft-float musl toolchain -> ${OUT_DIR}/bin/riscv64-linux-musl-"
