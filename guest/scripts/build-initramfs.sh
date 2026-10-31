#!/usr/bin/env bash
# Assembles the Vanilla profile initramfs (newc cpio + gzip) from a BusyBox
# rootfs staging dir produced by build-busybox.sh.
#
# Usage: build-initramfs.sh <rootfs-dir> <out.cpio.gz>
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOTFS="${1:?usage: build-initramfs.sh <rootfs-dir> <out.cpio.gz>}"
OUT="${2:?usage: build-initramfs.sh <rootfs-dir> <out.cpio.gz>}"

mkdir -p "${ROOTFS}"/{proc,sys,dev,etc,root,tmp,mnt,run}
cp "${HERE}/../initramfs/init" "${ROOTFS}/init"
chmod +x "${ROOTFS}/init"

cat > "${ROOTFS}/etc/inittab" <<'EOF'
::sysinit:/init
EOF
printf 'wasmnux-vanilla\n' > "${ROOTFS}/etc/hostname"
cat > "${ROOTFS}/etc/profile" <<'EOF'
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
export PS1='(vanilla) # '
EOF

# Static nodes the kernel wants before devtmpfs mounts.
if [ "$(id -u)" = "0" ]; then
  mknod -m 600 "${ROOTFS}/dev/console" c 5 1 2>/dev/null || true
  mknod -m 666 "${ROOTFS}/dev/null" c 1 3 2>/dev/null || true
fi

OUT_ABS="$(cd "$(dirname "${OUT}")" && pwd)/$(basename "${OUT}")"
( cd "${ROOTFS}" && find . -print0 | cpio --null -o -H newc --owner=root:root 2>/dev/null | gzip -9 > "${OUT_ABS}" )
echo "initramfs -> ${OUT_ABS} ($(du -h "${OUT_ABS}" | cut -f1))"
