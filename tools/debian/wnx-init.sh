#!/bin/bash
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
mkdir -p /dev/pts
mount -t devpts devpts /dev/pts 2>/dev/null || true
mount -o remount,rw /
hostname WASMNux-Debian
export PATH=/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin HOME=/root TERM=linux
echo WASMNUX_DEBIAN_READY
echo "Debian $(cat /etc/debian_version) riscv64 — real guest shell. Local packages: /opt/packages"
exec /bin/bash
