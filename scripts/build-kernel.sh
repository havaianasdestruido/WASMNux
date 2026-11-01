#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="/tmp/wasmnux-tools/llvm/bin:/tmp/wasmnux-tools/host/bin:/usr/local/bin:/usr/sbin:/sbin:$PATH"

LINUX_VERSION="7.3.1"
LINUX_TAG="v${LINUX_VERSION}"
LINUX_REPO="https://github.com/gregkh/linux.git"
BUILD_ROOT="${WASMNUX_GUEST_BUILD_DIR:-/tmp/wasmnux-guest-build}"
SRC_DIR="${BUILD_ROOT}/linux-${LINUX_VERSION}"
OUT_DIR="${BUILD_ROOT}/linux-out"
CONFIG_FRAGMENT="${ROOT_DIR}/configs/linux-7.3.1-riscv64.config"
DEST_DIR="${ROOT_DIR}/public/images"

mkdir -p "${BUILD_ROOT}" "${OUT_DIR}" "${DEST_DIR}"

# Ensure Clang 23 resource headers resolve cleanly when built via standalone llvm-runtime
if [ ! -e /tmp/wasmnux-tools/llvm-runtime-resources-linux/include ] && command -v zig >/dev/null 2>&1; then
  ZIG_LIB="$(zig env | sed -n 's/.*\.lib_dir = "\([^"]*\)".*/\1/p')"
  if [ -n "${ZIG_LIB}" ] && [ -d "${ZIG_LIB}/include" ]; then
    mkdir -p /tmp/wasmnux-tools/llvm-runtime-resources-linux
    ln -sfn "${ZIG_LIB}/include" /tmp/wasmnux-tools/llvm-runtime-resources-linux/include
  fi
fi

if [ ! -d "${SRC_DIR}/.git" ]; then
  echo "[build-kernel] Cloning official stable Linux ${LINUX_TAG} from ${LINUX_REPO}..."
  rm -rf "${SRC_DIR}"
  git clone --depth 1 --branch "${LINUX_TAG}" "${LINUX_REPO}" "${SRC_DIR}"
fi

ACTUAL_VER="$(make -s -C "${SRC_DIR}" kernelversion)"
ACTUAL_SHA="$(git -C "${SRC_DIR}" rev-parse HEAD)"
echo "[build-kernel] Verified Linux ${ACTUAL_VER} (commit ${ACTUAL_SHA})"
if [ "${ACTUAL_VER}" != "${LINUX_VERSION}" ]; then
  echo "[build-kernel] ERROR: Expected kernelversion ${LINUX_VERSION}, got ${ACTUAL_VER}" >&2
  exit 1
fi

echo "[build-kernel] Generating deterministic allnoconfig + ${CONFIG_FRAGMENT}..."
make -C "${SRC_DIR}" O="${OUT_DIR}" ARCH=riscv LLVM=1 HOSTCC=gcc HOSTCXX=g++ allnoconfig
KCONFIG_ALLCONFIG="${OUT_DIR}/.config" >/dev/null || true
"${SRC_DIR}/scripts/kconfig/merge_config.sh" -m -O "${OUT_DIR}" "${OUT_DIR}/.config" "${CONFIG_FRAGMENT}"
make -C "${SRC_DIR}" O="${OUT_DIR}" ARCH=riscv LLVM=1 HOSTCC=gcc HOSTCXX=g++ olddefconfig

# Verify mandatory hardware driver & syscall symbols in resolved .config
REQUIRED_SYMS=(
  "CONFIG_ARCH_RV64I=y"
  "CONFIG_FPU=y"
  "CONFIG_RISCV_ISA_C=y"
  "CONFIG_FUTEX=y"
  "CONFIG_EPOLL=y"
  "CONFIG_SECCOMP=y"
  "CONFIG_NAMESPACES=y"
  "CONFIG_CGROUPS=y"
  "CONFIG_BLK_DEV_INITRD=y"
  "CONFIG_DEVTMPFS=y"
  "CONFIG_EXT4_FS=y"
  "CONFIG_OVERLAY_FS=y"
  "CONFIG_TMPFS=y"
  "CONFIG_PROC_FS=y"
  "CONFIG_SYSFS=y"
  "CONFIG_INET=y"
  "CONFIG_SERIAL_8250=y"
  "CONFIG_SERIAL_8250_CONSOLE=y"
  "CONFIG_SERIAL_OF_PLATFORM=y"
  "CONFIG_HVC_RISCV_SBI=y"
  "CONFIG_VIRTIO_MMIO=y"
  "CONFIG_VIRTIO_BLK=y"
  "CONFIG_VIRTIO_NET=y"
  "CONFIG_HW_RANDOM_VIRTIO=y"
  "CONFIG_FB_SIMPLE=y"
  "CONFIG_FB_DEVICE=y"
  "CONFIG_FRAMEBUFFER_CONSOLE=y"
  "CONFIG_RTC_DRV_GOLDFISH=y"
  "CONFIG_POWER_RESET_SYSCON=y"
  "CONFIG_POWER_RESET_SYSCON_POWEROFF=y"
)

for sym in "${REQUIRED_SYMS[@]}"; do
  if ! grep -q "^${sym}$" "${OUT_DIR}/.config"; then
    echo "[build-kernel] ERROR: Missing required resolved symbol ${sym} in ${OUT_DIR}/.config!" >&2
    exit 1
  fi
done
echo "[build-kernel] All ${#REQUIRED_SYMS[@]} mandatory Kconfig symbols verified!"

JOBS="$(nproc 2>/dev/null || echo 2)"
echo "[build-kernel] Compiling Linux ${LINUX_VERSION} arch/riscv/boot/Image with ${JOBS} jobs..."
make -C "${SRC_DIR}" O="${OUT_DIR}" ARCH=riscv LLVM=1 HOSTCC=gcc HOSTCXX=g++ -j"${JOBS}" Image

IMAGE_PATH="${OUT_DIR}/arch/riscv/boot/Image"
if [ ! -f "${IMAGE_PATH}" ]; then
  echo "[build-kernel] ERROR: ${IMAGE_PATH} was not produced!" >&2
  exit 1
fi

# Verify RISC-V Linux Image magic ("RSC\x05" at offset 0x38)
MAGIC_HEX="$(od -An -tx1 -j 56 -N 4 "${IMAGE_PATH}" | tr -d ' \n')"
if [ "${MAGIC_HEX}" != "52534305" ]; then
  echo "[build-kernel] ERROR: Unexpected RISC-V Image magic 0x${MAGIC_HEX} (expected 52534305 'RSC\\x05')" >&2
  exit 1
fi

gzip -9 -n -c "${IMAGE_PATH}" > "${DEST_DIR}/vmlinux-${LINUX_VERSION}-riscv64.Image.gz"
SHA256_RAW="$(sha256sum "${IMAGE_PATH}" | awk '{print $1}')"
SHA256_GZ="$(sha256sum "${DEST_DIR}/vmlinux-${LINUX_VERSION}-riscv64.Image.gz" | awk '{print $1}')"
SIZE_RAW="$(stat -c '%s' "${IMAGE_PATH}")"
SIZE_GZ="$(stat -c '%s' "${DEST_DIR}/vmlinux-${LINUX_VERSION}-riscv64.Image.gz")"

echo "[build-kernel] SUCCESS:"
echo "  Raw Image : ${SIZE_RAW} bytes (sha256: ${SHA256_RAW}, magic: RSC\\x05)"
echo "  Gzipped   : ${DEST_DIR}/vmlinux-${LINUX_VERSION}-riscv64.Image.gz (${SIZE_GZ} bytes, sha256: ${SHA256_GZ})"
