#!/usr/bin/env bash
# Reproduce the pinned RV64 kernel Image. Requires the pinned LLVM/LLD tools and
# a checked-out Linux v7.3.1 (79a907609d53d5cd645b42c11550d295d43abf99).
set -euo pipefail
src=${1:?linux source dir}; out=${2:?output dir}
cp tools/kernel.config "$src/.config"
make -C "$src" ARCH=riscv CROSS_COMPILE=riscv64-linux-gnu- LLVM=1 olddefconfig
make -C "$src" ARCH=riscv CROSS_COMPILE=riscv64-linux-gnu- LLVM=1 -j"$(nproc)" Image dtbs
mkdir -p "$out"; cp "$src/arch/riscv/boot/Image" "$out/Image"
