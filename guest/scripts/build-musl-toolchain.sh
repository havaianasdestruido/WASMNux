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
MUSL_CROSS_MAKE_COMMIT="fe9158265703bb7d11797cf3ff70fc250bbacdde"

mkdir -p "${WORK_DIR}"
if [ ! -d "${WORK_DIR}/musl-cross-make" ]; then
  git clone https://github.com/richfelker/musl-cross-make "${WORK_DIR}/musl-cross-make"
fi
cd "${WORK_DIR}/musl-cross-make"
git checkout --quiet "${MUSL_CROSS_MAKE_COMMIT}"

cat > config.mak <<EOF
TARGET = riscv64-linux-musl
OUTPUT = ${OUT_DIR}
GCC_CONFIG += --with-arch=rv64imac --with-abi=lp64 --enable-languages=c
EOF

make -j"$(nproc)"
make install

"${OUT_DIR}/bin/riscv64-linux-musl-gcc" -v 2>&1 | tail -n 3
echo "Soft-float musl toolchain -> ${OUT_DIR}/bin/riscv64-linux-musl-"
