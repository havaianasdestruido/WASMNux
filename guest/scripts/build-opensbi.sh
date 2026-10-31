#!/usr/bin/env bash
# Builds OpenSBI (genuine RISC-V firmware, BSD-2-Clause) for the generic
# platform with FW_JUMP to the kernel Image load address (RAM + 0x200000).
# This is the production M-mode firmware path; the Rust core's built-in
# SBI shim is a documented development alternative, never a replacement
# claim.
#
# Usage: build-opensbi.sh <out-dir> [opensbi-tag]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="${1:?usage: build-opensbi.sh <out-dir> [opensbi-tag]}"
WORK_DIR="${WORK_DIR:-${HERE}/../build/opensbi-work}"
CROSS="${CROSS_COMPILE:-riscv64-linux-gnu-}"
TAG="${2:-$("${HERE}/versions.sh" opensbi)}"

mkdir -p "${WORK_DIR}" "${OUT_DIR}"
if [ ! -d "${WORK_DIR}/opensbi-${TAG}" ]; then
  curl -fsSL "https://github.com/riscv-software-src/opensbi/archive/refs/tags/${TAG}.tar.gz" \
    | tar -xz -C "${WORK_DIR}"
fi
cd "${WORK_DIR}/opensbi-${TAG#v}" 2>/dev/null || cd "${WORK_DIR}/opensbi-${TAG}"

# Kernel Image is loaded by the machine at 0x80200000 (RAM_BASE + 0x200000).
make CROSS_COMPILE="${CROSS}" PLATFORM=generic \
  FW_JUMP=y FW_JUMP_ADDR=0x80200000 -j"$(nproc)"

ELF="build/platform/generic/firmware/fw_jump.elf"
test -f "${ELF}"
"${CROSS}readelf" -h "${ELF}" | grep -q "RISC-V" || { echo "ERROR: fw_jump.elf is not RISC-V" >&2; exit 1; }
cp "${ELF}" "${OUT_DIR}/fw_jump.elf"
echo "${TAG}" > "${OUT_DIR}/opensbi.version"
sha256sum "${OUT_DIR}/fw_jump.elf" > "${OUT_DIR}/fw_jump.elf.sha256"
echo "OpenSBI ${TAG} -> ${OUT_DIR}/fw_jump.elf"
