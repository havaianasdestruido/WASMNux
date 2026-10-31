#!/usr/bin/env bash
# Builds a genuine Debian (latest stable) riscv64 root filesystem disk
# image from the official Debian mirror via debootstrap.
#
# IMPORTANT (honest scope): Debian riscv64 userland uses the lp64d ABI,
# which needs the F/D floating-point extensions. Until that CPU milestone
# lands (TODO.MD Phase 2), this image is only as bootable as the emulator;
# this script's job is to produce the *real, unmodified* Debian system so
# the milestone can be validated end-to-end the moment F/D works.
#
# Requirements (typically a CI runner): root, debootstrap,
# debian-archive-keyring, qemu-user-static, e2fsprogs.
#
# Usage: build-Debian.sh <out-dir> [size-mib]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="${1:?usage: build-Debian.sh <out-dir> [size-mib]}"
SIZE_MIB="${2:-2048}"
WORK_DIR="${WORK_DIR:-${HERE}/../build/Debian-work}"
MIRROR="${DEBIAN_MIRROR:-https://deb.debian.org/debian}"

read -r VERSION CODENAME <<< "$("${HERE}/versions.sh" Debian)"
echo "Building Debian ${VERSION} (${CODENAME}) riscv64, ${SIZE_MIB} MiB"

test "$(id -u)" = "0" || { echo "ERROR: debootstrap needs root" >&2; exit 1; }
command -v debootstrap >/dev/null
command -v qemu-riscv64-static >/dev/null

ROOTFS="${WORK_DIR}/rootfs"
rm -rf "${ROOTFS}"
mkdir -p "${ROOTFS}" "${OUT_DIR}"

# minbase + the bits the profile contract requires: BusyBox (static),
# init (systemd), and network tooling for the future relay. apt/dpkg come
# with minbase by construction and are never stubbed.
debootstrap --arch=riscv64 --variant=minbase \
  --include=busybox-static,systemd,udev,sudo,iproute2,console-setup,locales \
  --components=main \
  "${CODENAME}" "${ROOTFS}" "${MIRROR}"

cp "$(command -v qemu-riscv64-static)" "${ROOTFS}/usr/bin/"
chroot "${ROOTFS}" /usr/bin/qemu-riscv64-static /bin/bash -euo pipefail -c '
  export DEBIAN_FRONTEND=noninteractive
  echo "wasmnux-Debian" > /etc/hostname
  # Serial console login (root / Debian — documented dev credentials).
  mkdir -p /etc/systemd/system/serial-getty@ttyS0.service.d
  cat > /etc/systemd/system/serial-getty@ttyS0.service.d/autologin.conf <<CONF
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin root --keep-baud 115200,57600,38400,9600 %I $TERM
CONF
  systemctl enable serial-getty@ttyS0.service || true
  echo "root:Debian" | chpasswd
  cat > /etc/fstab <<FSTAB
/dev/vda  /  ext4  errors=remount-ro  0  1
proc      /proc  proc  defaults  0  0
FSTAB
  cat > /etc/apt/sources.list <<SOURCES
deb http://deb.debian.org/debian CODENAME main
deb http://deb.debian.org/debian CODENAME-updates main
deb http://deb.debian.org/debian-security CODENAME-security main
SOURCES
  sed -i "s/CODENAME/"'"${CODENAME}"'"/g" /etc/apt/sources.list
  apt-get clean
'
rm -f "${ROOTFS}/usr/bin/qemu-riscv64-static"

IMG="${OUT_DIR}/debian-${CODENAME}-riscv64.img"
truncate -s "${SIZE_MIB}M" "${IMG}"
# -d populates the fs from the rootfs tree without loop-mounting.
mkfs.ext4 -q -F -L wasmnux-Debian -d "${ROOTFS}" "${IMG}"
sha256sum "${IMG}" > "${IMG}.sha256"
echo "Debian ${VERSION} (${CODENAME}) riscv64 root image -> ${IMG}"
