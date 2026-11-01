#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="/tmp/wasmnux-tools/llvm/bin:/tmp/wasmnux-tools/host/bin:/usr/local/bin:/usr/sbin:/sbin:$PATH"

BUILD_ROOT="${WASMNUX_GUEST_BUILD_DIR:-/tmp/wasmnux-guest-build}"
BUSYBOX_SRC="${BUILD_ROOT}/busybox-1.36.1"
BUSYBOX_BIN="${BUILD_ROOT}/busybox-riscv64-static"
STAGE_DIR="${BUILD_ROOT}/vanilla-initramfs-stage"
DISK_STAGE_DIR="${BUILD_ROOT}/vanilla-disk-stage"
DEST_DIR="${ROOT_DIR}/public/images"

if [ ! -x "${BUSYBOX_BIN}" ]; then
  "${ROOT_DIR}/scripts/build-busybox.sh"
fi

rm -rf "${STAGE_DIR}" "${DISK_STAGE_DIR}"
mkdir -p "${STAGE_DIR}" "${DISK_STAGE_DIR}" "${DEST_DIR}"

# 1. Install BusyBox applets into staging hierarchy (/bin, /sbin, /usr/bin, /usr/sbin)
WRAP_BIN="${BUILD_ROOT}/rv64-musl-wrap"
export PATH="${WRAP_BIN}:${PATH}"
make -C "${BUSYBOX_SRC}" HOSTCC=gcc CROSS_COMPILE=riscv64-linux-musl- CONFIG_PREFIX="${STAGE_DIR}" install >/dev/null
cp -f "${BUSYBOX_BIN}" "${STAGE_DIR}/bin/busybox"
chmod 0755 "${STAGE_DIR}/bin/busybox"

mkdir -p \
  "${STAGE_DIR}/dev" \
  "${STAGE_DIR}/dev/pts" \
  "${STAGE_DIR}/dev/shm" \
  "${STAGE_DIR}/etc/init.d" \
  "${STAGE_DIR}/home" \
  "${STAGE_DIR}/mnt" \
  "${STAGE_DIR}/persist" \
  "${STAGE_DIR}/proc" \
  "${STAGE_DIR}/root" \
  "${STAGE_DIR}/run" \
  "${STAGE_DIR}/sys" \
  "${STAGE_DIR}/tmp" \
  "${STAGE_DIR}/var/log" \
  "${STAGE_DIR}/var/tmp"

# 2. Compile static riscv64-linux-musl /usr/bin/wasmnux-fbtest framebuffer utility
FBTEST_SRC="${BUILD_ROOT}/wasmnux-fbtest.c"
cat > "${FBTEST_SRC}" <<'EOF'
#include <fcntl.h>
#include <linux/fb.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

int main(int argc, char **argv) {
    int banner_only = (argc > 1 && strcmp(argv[1], "--banner") == 0);
    int fd = open("/dev/fb0", O_RDWR);
    if (fd < 0) {
        perror("open /dev/fb0");
        return 1;
    }

    struct fb_var_screeninfo vinfo;
    struct fb_fix_screeninfo finfo;
    if (ioctl(fd, FBIOGET_FSCREENINFO, &finfo) < 0 ||
        ioctl(fd, FBIOGET_VSCREENINFO, &vinfo) < 0) {
        perror("ioctl fb0");
        close(fd);
        return 1;
    }

    size_t screensize = (size_t)finfo.line_length * (size_t)vinfo.yres;
    uint8_t *fbp = (uint8_t *)mmap(0, screensize, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (fbp == MAP_FAILED) {
        perror("mmap fb0");
        close(fd);
        return 1;
    }

    uint32_t max_y = banner_only ? (vinfo.yres < 36 ? vinfo.yres : 36) : vinfo.yres;
    for (uint32_t y = 0; y < max_y; y++) {
        uint32_t *row = (uint32_t *)(fbp + (size_t)y * finfo.line_length);
        for (uint32_t x = 0; x < vinfo.xres; x++) {
            uint32_t r = (x * 255u) / (vinfo.xres ? vinfo.xres : 1u);
            uint32_t g = ((y * 180u) / (vinfo.yres ? vinfo.yres : 1u)) + 32u;
            uint32_t b = 220u - ((x * 160u) / (vinfo.xres ? vinfo.xres : 1u));
            if (y < 4 || y == max_y - 1) {
                row[x] = 0x0038BDF8u; /* Cyan accent border */
            } else if (!banner_only && y >= vinfo.yres - 48) {
                /* SMPTE-style color verification bars at bottom */
                uint32_t band = (x * 6u) / (vinfo.xres ? vinfo.xres : 1u);
                static const uint32_t bars[6] = {
                    0x00EF4444u, 0x00F59E0Bu, 0x0010B981u,
                    0x0006B6D4u, 0x003B82F6u, 0x00A855F7u
                };
                row[x] = bars[band % 6u];
            } else {
                row[x] = ((r & 0xFFu) << 16) | ((g & 0xFFu) << 8) | (b & 0xFFu);
            }
        }
    }

    msync(fbp, screensize, MS_SYNC);
    munmap(fbp, screensize);
    close(fd);
    if (!banner_only) {
        printf("Rendered %ux%u (%u bpp) pattern to /dev/fb0\n",
               vinfo.xres, vinfo.yres, vinfo.bits_per_pixel);
    }
    return 0;
}
EOF

zig cc -target riscv64-linux-musl -mcpu=generic_rv64+m+a+f+d+c -O2 -static \
  "${FBTEST_SRC}" -o "${STAGE_DIR}/usr/bin/wasmnux-fbtest"
llvm-strip "${STAGE_DIR}/usr/bin/wasmnux-fbtest"
chmod 0755 "${STAGE_DIR}/usr/bin/wasmnux-fbtest"

# 3. Populate /etc configuration files
cat > "${STAGE_DIR}/etc/passwd" <<'EOF'
root:x:0:0:root:/root:/bin/ash
nobody:x:65534:65534:nobody:/nonexistent:/bin/false
EOF

cat > "${STAGE_DIR}/etc/group" <<'EOF'
root:x:0:
tty:x:5:
disk:x:6:
video:x:44:
nogroup:x:65534:
EOF

cat > "${STAGE_DIR}/etc/hostname" <<'EOF'
wasmnux-vanilla
EOF

cat > "${STAGE_DIR}/etc/hosts" <<'EOF'
127.0.0.1	localhost wasmnux-vanilla
10.0.2.2	host.wasmnux.internal
10.0.2.15	wasmnux-vanilla
EOF

cat > "${STAGE_DIR}/etc/resolv.conf" <<'EOF'
nameserver 10.0.2.3
EOF

cat > "${STAGE_DIR}/etc/os-release" <<'EOF'
NAME="WASMNux Vanilla Linux"
ID=wasmnux-vanilla
VERSION="7.3.1"
VERSION_ID="7.3.1"
PRETTY_NAME="WASMNux Vanilla Linux 7.3.1 (RISC-V 64-bit / BusyBox 1.36.1)"
HOME_URL="https://github.com/havaianasdestruido/WASMNux"
EOF

cat > "${STAGE_DIR}/etc/profile" <<'EOF'
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export HOME="/root"
export TERM="${TERM:-xterm-256color}"
export PS1='\u@\h:\w\# '
alias ll='ls -la'
cd "$HOME" 2>/dev/null || true
EOF

cat > "${STAGE_DIR}/etc/inittab" <<'EOF'
::sysinit:/etc/init.d/rcS
ttyS0::respawn:-/bin/ash
::ctrlaltdel:/sbin/reboot
::shutdown:/etc/init.d/rcK
EOF

cat > "${STAGE_DIR}/etc/init.d/rcS" <<'EOF'
#!/bin/sh
export PATH="/usr/sbin:/usr/bin:/sbin:/bin"

mount -t proc proc /proc 2>/dev/null || true
mount -t sysfs sysfs /sys 2>/dev/null || true
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
mkdir -p /dev/pts /dev/shm /run /tmp /var/log /persist
mount -t devpts devpts /dev/pts 2>/dev/null || true
mount -t tmpfs -o mode=1777 tmpfs /tmp 2>/dev/null || true
mount -t tmpfs -o mode=0755 tmpfs /run 2>/dev/null || true

# Mount persistent ext4 virtual block disk (/dev/vda)
if [ -b /dev/vda ]; then
  if mount -t ext4 -o rw,noatime /dev/vda /persist 2>/dev/null; then
    mkdir -p /persist/root /persist/home /persist/etc
    mount --bind /persist/root /root
    mount --bind /persist/home /home
    if [ -f /persist/etc/hostname ]; then
      cp -f /persist/etc/hostname /etc/hostname
    fi
  else
    echo "[WASMNux] WARNING: /dev/vda mount failed; continuing with volatile ramfs"
  fi
fi

hostname -F /etc/hostname 2>/dev/null || hostname wasmnux-vanilla

# Configure loopback + VirtIO-Net eth0 (10.0.2.15/24 via 10.0.2.2)
ip link set lo up 2>/dev/null || true
ip addr add 127.0.0.1/8 dev lo 2>/dev/null || true
if [ -d /sys/class/net/eth0 ]; then
  ip link set eth0 up 2>/dev/null || true
  ip addr add 10.0.2.15/24 dev eth0 2>/dev/null || true
  ip route add default via 10.0.2.2 dev eth0 2>/dev/null || true
fi

# Paint top SimpleFB graphic status bar if /dev/fb0 is present
if [ -c /dev/fb0 ] && [ -x /usr/bin/wasmnux-fbtest ]; then
  /usr/bin/wasmnux-fbtest --banner >/dev/null 2>&1 || true
fi

echo ""
echo "============================================================"
echo " WASMNux Vanilla Linux $(uname -r) ($(uname -m))"
echo " Persistent disk: $(grep -q ' /persist ' /proc/mounts && echo '/dev/vda mounted on /persist (/root, /home)' || echo 'ramfs')"
echo " Run 'wasmnux-diag' for hardware & storage diagnostics."
echo "============================================================"
EOF

cat > "${STAGE_DIR}/etc/init.d/rcK" <<'EOF'
#!/bin/sh
export PATH="/usr/sbin:/usr/bin:/sbin:/bin"
echo "[WASMNux] Syncing filesystems and unmounting persistent storage..."
sync
umount /root 2>/dev/null || true
umount /home 2>/dev/null || true
umount /persist 2>/dev/null || true
sync
EOF

cat > "${STAGE_DIR}/init" <<'EOF'
#!/bin/sh
export PATH="/usr/sbin:/usr/bin:/sbin:/bin"
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
exec /sbin/init
EOF

cat > "${STAGE_DIR}/usr/bin/wasmnux-diag" <<'EOF'
#!/bin/sh
set -e
echo "=== WASMNux Guest Diagnostics ==="
echo "Kernel   : $(uname -srmo)"
echo "Hostname : $(hostname)"
echo "Uptime   : $(cat /proc/uptime)"
echo "CPU      : $(grep -m1 'isa' /proc/cpuinfo || true)"
echo "Memory   :"
free -m
echo "Mounts   :"
df -h / /persist 2>/dev/null || df -h
echo "Network  :"
ip -4 addr show 2>/dev/null || true
EOF

chmod 0755 \
  "${STAGE_DIR}/init" \
  "${STAGE_DIR}/etc/init.d/rcS" \
  "${STAGE_DIR}/etc/init.d/rcK" \
  "${STAGE_DIR}/usr/bin/wasmnux-diag"

# 4. Pack deterministic SVR4 newc gzipped initramfs archive (uid=0, gid=0)
python3 - "${STAGE_DIR}" "${DEST_DIR}/vanilla-initramfs.cpio.gz" <<'PY'
import gzip, os, stat, sys

stage_dir, out_gz = sys.argv[1], sys.argv[2]
entries = []

for root, dirs, files in os.walk(stage_dir):
    dirs.sort()
    files.sort()
    rel_root = os.path.relpath(root, stage_dir)
    if rel_root != ".":
        entries.append((rel_root, root))
    for f in files:
        full = os.path.join(root, f)
        rel = f if rel_root == "." else os.path.join(rel_root, f)
        entries.append((rel, full))

# Ensure dirs whose links appear are also included (os.walk puts symlinked dirs in dirs or files depending on stat)
seen = set()
ino = 1

def pad4(n):
    rem = n % 4
    return b"\x00" * ((4 - rem) % 4)

buf = bytearray()
for rel, full in entries:
    rel = rel.replace("\\", "/")
    if rel in seen:
        continue
    seen.add(rel)
    st = os.lstat(full)
    mode = st.st_mode
    data = b""
    if stat.S_ISLNK(mode):
        data = os.readlink(full).encode("utf-8")
        nlink = 1
    elif stat.S_ISDIR(mode):
        nlink = 2
    elif stat.S_ISREG(mode):
        with open(full, "rb") as fp:
            data = fp.read()
        nlink = 1
    else:
        continue

    name_bytes = rel.encode("utf-8") + b"\x00"
    hdr = (
        "070701"
        f"{ino:08x}"
        f"{mode:08x}"
        f"{0:08x}"
        f"{0:08x}"
        f"{nlink:08x}"
        f"{1700000000:08x}"
        f"{len(data):08x}"
        f"{0:08x}{0:08x}{0:08x}{0:08x}"
        f"{len(name_bytes):08x}"
        f"{0:08x}"
    ).encode("ascii")
    ino += 1
    buf.extend(hdr)
    buf.extend(name_bytes)
    buf.extend(pad4(len(hdr) + len(name_bytes)))
    if data:
        buf.extend(data)
        buf.extend(pad4(len(data)))

# Append TRAILER!!!
trailer_name = b"TRAILER!!!\x00"
hdr = (
    "070701"
    + "00000000" * 4
    + f"{1:08x}"
    + "00000000" * 6
    + f"{len(trailer_name):08x}"
    + "00000000"
).encode("ascii")
buf.extend(hdr)
buf.extend(trailer_name)
buf.extend(pad4(len(hdr) + len(trailer_name)))

with open(out_gz, "wb") as raw_out:
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw_out, compresslevel=9, mtime=0) as gz:
        gz.write(buf)
PY

# 5. Build 64 MiB ext4 persistent base disk image (vanilla-disk.ext4.gz)
mkdir -p "${DISK_STAGE_DIR}/root" "${DISK_STAGE_DIR}/home" "${DISK_STAGE_DIR}/etc"
cat > "${DISK_STAGE_DIR}/root/WELCOME.txt" <<'EOF'
Welcome to WASMNux Vanilla Linux 7.3.1 (riscv64)!
Files saved in /root, /home, and /persist are stored on /dev/vda (ext4)
and persist across reboots and browser restarts via OPFS + IndexedDB.
EOF

RAW_EXT4="${BUILD_ROOT}/vanilla-disk.ext4"
rm -f "${RAW_EXT4}"
truncate -s 64M "${RAW_EXT4}"
/sbin/mkfs.ext4 -q -F -L "wasmnux-vanilla" -E root_owner=0:0 -d "${DISK_STAGE_DIR}" "${RAW_EXT4}"
gzip -9 -n -c "${RAW_EXT4}" > "${DEST_DIR}/vanilla-disk.ext4.gz"

echo "[build-vanilla-rootfs] SUCCESS:"
ls -lh "${DEST_DIR}/vanilla-initramfs.cpio.gz" "${DEST_DIR}/vanilla-disk.ext4.gz"
