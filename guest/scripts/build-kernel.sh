#!/usr/bin/env bash
# Builds the genuine upstream Linux kernel (latest stable, resolved at run
# time) for riscv64 using the WASMNux config fragment.
#
# Provenance: tarball + sha256sums come from cdn.kernel.org; the checksum
# is verified before building. PGP verification of the .tar.sign is
# performed when a kernel.org keyring is present (see notes in
# docs/DEPENDENCIES.md); the build never proceeds on a bad checksum.
#
# Usage: build-kernel.sh <out-dir> [kernel-version]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="${1:?usage: build-kernel.sh <out-dir> [kernel-version]}"
WORK_DIR="${WORK_DIR:-${HERE}/../build/kernel-work}"
CROSS="${CROSS_COMPILE:-riscv64-linux-gnu-}"

VERSION="${2:-$("${HERE}/versions.sh" kernel)}"
MAJOR="v${VERSION%%.*}.x"
TARBALL="linux-${VERSION}.tar.xz"
BASE_URL="https://cdn.kernel.org/pub/linux/kernel/${MAJOR}"

mkdir -p "${WORK_DIR}" "${OUT_DIR}"
cd "${WORK_DIR}"

if [ ! -f "${TARBALL}" ]; then
  curl -fLO "${BASE_URL}/${TARBALL}"
fi
curl -fsSLO "${BASE_URL}/sha256sums.asc"
# sha256sums.asc is a signed message listing all files; verify our tarball.
grep -E "^[0-9a-f]{64}  ${TARBALL}$" sha256sums.asc | sha256sum -c -
if command -v gpg >/dev/null && [ -n "${KERNEL_ORG_KEYRING:-}" ]; then
  gpg --keyring "${KERNEL_ORG_KEYRING}" --verify sha256sums.asc
else
  echo "NOTE: PGP verification skipped (no KERNEL_ORG_KEYRING); sha256 matched over HTTPS."
fi

rm -rf "linux-${VERSION}"
tar -xf "${TARBALL}"
cd "linux-${VERSION}"

make ARCH=riscv CROSS_COMPILE="${CROSS}" defconfig
./scripts/kconfig/merge_config.sh -m .config "${HERE}/../kernel/config.fragment"
make ARCH=riscv CROSS_COMPILE="${CROSS}" olddefconfig

# Config assertions: fail the build loudly if an option we depend on drifted.
for opt in CONFIG_SERIAL_8250_CONSOLE CONFIG_VIRTIO_MMIO CONFIG_VIRTIO_BLK \
           CONFIG_BLK_DEV_INITRD CONFIG_EXT4_FS CONFIG_FB_SIMPLE; do
  if ! grep -q "^${opt}=y" .config; then
    echo "ERROR: required kernel option ${opt} is not =y after merge" >&2
    exit 1
  fi
done

make ARCH=riscv CROSS_COMPILE="${CROSS}" -j"$(nproc)" Image

cp arch/riscv/boot/Image "${OUT_DIR}/Image"
echo "${VERSION}" > "${OUT_DIR}/kernel.version"
sha256sum "${OUT_DIR}/Image" > "${OUT_DIR}/Image.sha256"
echo "Built Linux ${VERSION} (riscv64) -> ${OUT_DIR}/Image"
