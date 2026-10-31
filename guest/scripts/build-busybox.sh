#!/usr/bin/env bash
# Builds static soft-float BusyBox for riscv64 (RV64IMAC, mabi=lp64) so it
# runs on the emulator before F/D lands, and installs it into a rootfs
# staging dir. Applet presence is enforced against the Mission §3 list —
# behaviour itself is tested in-guest by guest/tests/busybox-applets.sh.
#
# Usage: build-busybox.sh <rootfs-dir> [busybox-version]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOTFS="${1:?usage: build-busybox.sh <rootfs-dir> [busybox-version]}"
WORK_DIR="${WORK_DIR:-${HERE}/../build/busybox-work}"
CROSS="${CROSS_COMPILE:-riscv64-linux-gnu-}"
VERSION="${2:-$("${HERE}/versions.sh" busybox)}"

mkdir -p "${WORK_DIR}" "${ROOTFS}"
cd "${WORK_DIR}"
if [ ! -f "busybox-${VERSION}.tar.bz2" ]; then
  curl -fLO "https://busybox.net/downloads/busybox-${VERSION}.tar.bz2"
fi
rm -rf "busybox-${VERSION}"
tar -xf "busybox-${VERSION}.tar.bz2"
cd "busybox-${VERSION}"

make ARCH=riscv64 CROSS_COMPILE="${CROSS}" defconfig
# Static binary: no dynamic loader assumptions inside the initramfs.
sed -i 's/^# CONFIG_STATIC is not set/CONFIG_STATIC=y/' .config
if ! grep -q '^CONFIG_STATIC=y' .config; then
  echo "ERROR: CONFIG_STATIC could not be enabled" >&2
  exit 1
fi

SOFTFLOAT="-march=rv64imac_zicsr -mabi=lp64"
make ARCH=riscv64 CROSS_COMPILE="${CROSS}" EXTRA_CFLAGS="${SOFTFLOAT}" -j"$(nproc)"
make ARCH=riscv64 CROSS_COMPILE="${CROSS}" EXTRA_CFLAGS="${SOFTFLOAT}" \
  CONFIG_PREFIX="${ROOTFS}" install

file "${ROOTFS}/bin/busybox"
if ! file "${ROOTFS}/bin/busybox" | grep -q "RISC-V"; then
  echo "ERROR: busybox binary is not a RISC-V binary" >&2
  exit 1
fi

# Applet presence gate (the binary is foreign-arch on the build host, so
# interrogate it through qemu-user when available; otherwise inspect the
# applet table strings — presence only, behaviour is tested in-guest).
REQUIRED="sh ash ls cat cp mv rm mkdir rmdir mount umount ps kill dmesg uname free df du grep sed awk find tar gzip touch chmod chown ln readlink date sleep top reboot poweroff"
if command -v qemu-riscv64-static >/dev/null; then
  APPLETS="$(qemu-riscv64-static "${ROOTFS}/bin/busybox" --list | tr '\n' ' ')"
else
  APPLETS="$(strings "${ROOTFS}/bin/busybox" | tr '\n' ' ')"
  echo "NOTE: qemu-riscv64-static not present; applet check used string scan."
fi
MISSING=""
for applet in ${REQUIRED}; do
  case " ${APPLETS} " in
    *" ${applet} "*) ;;
    *) MISSING="${MISSING} ${applet}" ;;
  esac
done
if [ -n "${MISSING}" ]; then
  echo "ERROR: busybox is missing required applets:${MISSING}" >&2
  exit 1
fi
echo "BusyBox ${VERSION} installed to ${ROOTFS} (all required applets present)"
