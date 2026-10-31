#!/usr/bin/env bash
# Build a genuine persistent RV64 BusyBox root filesystem; no host package execution.
set -euo pipefail
out=${1:?output image}; bb=${2:?static RV64 BusyBox}
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
mkdir -p "$work"/{bin,sbin,etc/init.d,proc,sys,dev,home/user,root,tmp,var/lib,run}
cp "$bb" "$work/bin/busybox"; chmod 755 "$work/bin/busybox"
for applet in sh ash ls cat cp mv rm mkdir rmdir ln mount umount sync reboot poweroff dmesg uname pwd cd echo grep sed awk vi touch chmod chown ps kill free df du head tail sleep date hostname login su adduser addgroup passwd env export clear reset; do ln -sf busybox "$work/bin/$applet"; done
ln -sf ../bin/busybox "$work/sbin/init"
printf 'root:x:0:0:root:/root:/bin/sh\nuser:x:1000:1000:User:/home/user:/bin/sh\n' > "$work/etc/passwd"
printf 'root:x:0:\nuser:x:1000:\n' > "$work/etc/group"
printf 'WASMNux\n' > "$work/etc/hostname"
printf '::sysinit:/etc/init.d/rcS\nttyS0::respawn:/bin/sh\n::ctrlaltdel:/sbin/reboot\n::shutdown:/bin/umount -a -r\n' > "$work/etc/inittab"
ln -sf ../bin/reboot "$work/sbin/reboot"; ln -sf ../bin/poweroff "$work/sbin/poweroff"
printf '#!/bin/sh\nmount -t proc proc /proc\nmount -t sysfs sysfs /sys\nmount -o remount,rw /\nhostname -F /etc/hostname\nexport HOME=/root PATH=/bin:/sbin\necho WASMNUX_PERSISTENT_ROOT_READY\n' > "$work/etc/init.d/rcS"; chmod 755 "$work/etc/init.d/rcS"
truncate -s 64M "$out"
/sbin/mkfs.ext2 -q -F -b 1024 -d "$work" -L WASMNUX "$out"
