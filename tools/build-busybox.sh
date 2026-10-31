#!/usr/bin/env bash
# Reproduce the static musl RV64 BusyBox 1.37.0 binary using Zig 0.13.0 as the
# cross compiler. Source: vda-linux/busybox_mirror tag 1_37_0.
set -euo pipefail
src=${1:?busybox source dir}; out=${2:?output dir}
cp tools/busybox.config "$src/.config"
make -C "$src" olddefconfig
make -C "$src" CC="zig cc -target riscv64-linux-musl" AR="zig ar" -j"$(nproc)" busybox
mkdir -p "$out"; cp "$src/busybox" "$out/busybox"
