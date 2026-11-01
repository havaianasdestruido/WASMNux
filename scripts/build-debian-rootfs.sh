#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="/tmp/wasmnux-tools/llvm/bin:/tmp/wasmnux-tools/host/bin:/usr/local/bin:/usr/sbin:/sbin:$PATH"

BUILD_ROOT="${WASMNUX_GUEST_BUILD_DIR:-/tmp/wasmnux-guest-build}"
DEBIAN_ARTIFACTS_DIR="${BUILD_ROOT}/debian-artifacts"
DEBIAN_SLIM_DIR="${DEBIAN_ARTIFACTS_DIR}/trixie/slim"
DEBIAN_ROOTFS_TAR="${DEBIAN_SLIM_DIR}/oci/blobs/rootfs.tar.gz"
EXPECTED_DEBIAN_SHA256="a81ae23cfcc12d03cee9e3704fa559c027af15fefe8c60924add8fd17e4c98c1"

BUSYBOX_BIN="${BUILD_ROOT}/busybox-riscv64-static"
DEST_IMAGES_DIR="${ROOT_DIR}/public/images"
DEST_REPO_DIR="${ROOT_DIR}/public/debian-repo"
PKG_BUILD_DIR="${BUILD_ROOT}/debian-pkg-build"
STAGE_DIR="${BUILD_ROOT}/debian-rootfs-stage"
RAW_EXT4="${BUILD_ROOT}/debian-rootfs.ext4"

mkdir -p "${BUILD_ROOT}" "${DEST_IMAGES_DIR}"

# 1. Ensure static riscv64 BusyBox 1.36.1 binary is available for /usr/bin/busybox.
if [ ! -x "${BUSYBOX_BIN}" ]; then
  echo "[build-debian-rootfs] Building static riscv64 BusyBox 1.36.1..."
  "${ROOT_DIR}/scripts/build-busybox.sh"
fi

# 2. Fetch & cryptographically verify official Debian 13.7 ("trixie") riscv64 rootfs.
if [ ! -f "${DEBIAN_ROOTFS_TAR}" ]; then
  echo "[build-debian-rootfs] Fetching official Debian 13 (trixie) riscv64 rootfs..."
  rm -rf "${DEBIAN_ARTIFACTS_DIR}"
  git clone --depth 1 --filter=blob:none --sparse --branch dist-riscv64 \
    https://github.com/debuerreotype/docker-debian-artifacts.git "${DEBIAN_ARTIFACTS_DIR}"
  git -C "${DEBIAN_ARTIFACTS_DIR}" sparse-checkout set trixie/slim
fi

ACTUAL_SHA256="$(sha256sum "${DEBIAN_ROOTFS_TAR}" | awk '{print $1}')"
if [ "${ACTUAL_SHA256}" != "${EXPECTED_DEBIAN_SHA256}" ]; then
  echo "[build-debian-rootfs] ERROR: SHA-256 mismatch on ${DEBIAN_ROOTFS_TAR}" >&2
  echo "  expected: ${EXPECTED_DEBIAN_SHA256}" >&2
  echo "  actual:   ${ACTUAL_SHA256}" >&2
  exit 1
fi

DEB_ARCH="$(tr -d '[:space:]' < "${DEBIAN_SLIM_DIR}/rootfs.dpkg-arch")"
DEB_VER="$(tr -d '[:space:]' < "${DEBIAN_SLIM_DIR}/rootfs.debian_version")"
if [ "${DEB_ARCH}" != "riscv64" ]; then
  echo "[build-debian-rootfs] ERROR: unexpected Debian architecture '${DEB_ARCH}'" >&2
  exit 1
fi
echo "[build-debian-rootfs] Verified official Debian ${DEB_VER} (trixie) ${DEB_ARCH} rootfs (${ACTUAL_SHA256})"

# 3. Compile genuine riscv64 ELF binaries for guest tools & .deb APT repository packages.
rm -rf "${PKG_BUILD_DIR}"
mkdir -p "${PKG_BUILD_DIR}/src" "${PKG_BUILD_DIR}/bin"

cat > "${PKG_BUILD_DIR}/src/wasmnux-hello.c" <<'EOF'
#include <stdio.h>
#include <sys/utsname.h>

int main(int argc, char **argv) {
    struct utsname u;
    if (uname(&u) != 0) {
        perror("uname");
        return 1;
    }
    printf("WASMNUX_HELLO_OK kernel=%s-%s arch=%s", u.sysname, u.release, u.machine);
    for (int i = 1; i < argc; i++) {
        printf(" arg[%d]=%s", i, argv[i]);
    }
    printf("\n");
    return 0;
}
EOF

cat > "${PKG_BUILD_DIR}/src/wasmnux-sysbench.c" <<'EOF'
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static double now_sec(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec * 1e-9;
}

int main(void) {
    double t0 = now_sec();
    uint32_t primes = 0;
    for (uint32_t n = 2; n <= 50000u; n++) {
        int is_p = 1;
        for (uint32_t d = 2; d * d <= n; d++) {
            if (n % d == 0u) {
                is_p = 0;
                break;
            }
        }
        if (is_p) primes++;
    }
    double dt_cpu = now_sec() - t0;

    size_t sz = 4u * 1024u * 1024u;
    uint8_t *src = (uint8_t *)malloc(sz);
    uint8_t *dst = (uint8_t *)malloc(sz);
    if (!src || !dst) {
        fprintf(stderr, "alloc failed\n");
        return 1;
    }
    for (size_t i = 0; i < sz; i++) src[i] = (uint8_t)(i * 131u + 17u);

    double m0 = now_sec();
    for (int iter = 0; iter < 8; iter++) {
        memcpy(dst, src, sz);
        src[iter] ^= dst[sz - 1 - (size_t)iter];
    }
    double dt_mem = now_sec() - m0;
    double mib_s = (32.0) / (dt_mem > 1e-6 ? dt_mem : 1e-6);

    printf("WASMNUX_SYSBENCH_OK primes_upto_50k=%u cpu_ms=%.2f memcpy_mib_s=%.1f checksum=%u\n",
           primes, dt_cpu * 1000.0, mib_s, (unsigned)dst[1024]);
    free(src);
    free(dst);
    return 0;
}
EOF

cat > "${PKG_BUILD_DIR}/src/wasmnux-fbtest.c" <<'EOF'
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
    int crimson = (argc > 1 && strcmp(argv[1], "--debian") == 0);
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
            if (y < 4 || y == max_y - 1) {
                row[x] = crimson ? 0x00D70A53u : 0x0038BDF8u;
            } else if (!banner_only && y >= vinfo.yres - 48) {
                uint32_t band = (x * 6u) / (vinfo.xres ? vinfo.xres : 1u);
                static const uint32_t bars[6] = {
                    0x00D70A53u, 0x00F59E0Bu, 0x0010B981u,
                    0x0006B6D4u, 0x003B82F6u, 0x00A855F7u
                };
                row[x] = bars[band % 6u];
            } else {
                uint32_t r = crimson
                    ? (140u + ((x * 115u) / (vinfo.xres ? vinfo.xres : 1u)))
                    : ((x * 255u) / (vinfo.xres ? vinfo.xres : 1u));
                uint32_t g = ((y * 140u) / (vinfo.yres ? vinfo.yres : 1u)) + 16u;
                uint32_t b = 180u - ((x * 120u) / (vinfo.xres ? vinfo.xres : 1u));
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

zig cc -target riscv64-linux-musl -static -O2 -s \
  -o "${PKG_BUILD_DIR}/bin/wasmnux-hello" "${PKG_BUILD_DIR}/src/wasmnux-hello.c"
zig cc -target riscv64-linux-musl -static -O2 -s \
  -o "${PKG_BUILD_DIR}/bin/wasmnux-sysbench" "${PKG_BUILD_DIR}/src/wasmnux-sysbench.c"
zig cc -target riscv64-linux-musl -static -O2 -s \
  -o "${PKG_BUILD_DIR}/bin/wasmnux-fbtest" "${PKG_BUILD_DIR}/src/wasmnux-fbtest.c"

# 4. Build genuine Debian (.deb) packages and local APT repository (`public/debian-repo`).
rm -rf "${DEST_REPO_DIR}"
mkdir -p \
  "${DEST_REPO_DIR}/pool/main/w/wasmnux-hello" \
  "${DEST_REPO_DIR}/pool/main/w/wasmnux-sysbench" \
  "${DEST_REPO_DIR}/pool/main/w/wasmnux-fbtools" \
  "${DEST_REPO_DIR}/pool/main/w/wasmnux-notes" \
  "${DEST_REPO_DIR}/dists/trixie/main/binary-riscv64"

build_deb_pkg() {
  local pkg_name="$1"
  local pkg_ver="$2"
  local pkg_arch="$3"
  local pkg_deps="$4"
  local pkg_desc="$5"
  local bin_src="$6"
  local bin_dst="$7"
  local out_deb="$8"

  local root="${PKG_BUILD_DIR}/pkg-${pkg_name}"
  rm -rf "${root}"
  mkdir -p "${root}/DEBIAN" "${root}/usr/bin" "${root}/usr/share/doc/${pkg_name}"

  cp -f "${bin_src}" "${root}/usr/bin/${bin_dst}"
  chmod 0755 "${root}/usr/bin/${bin_dst}"

  cat > "${root}/usr/share/doc/${pkg_name}/copyright" <<EOC
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: ${pkg_name}
License: Apache-2.0
EOC
  chmod 0644 "${root}/usr/share/doc/${pkg_name}/copyright"

  local inst_kb
  inst_kb="$(du -sk "${root}/usr" | awk '{print $1}')"

  {
    echo "Package: ${pkg_name}"
    echo "Version: ${pkg_ver}"
    echo "Section: utils"
    echo "Priority: optional"
    echo "Architecture: ${pkg_arch}"
    if [ -n "${pkg_deps}" ]; then
      echo "Depends: ${pkg_deps}"
    fi
    echo "Installed-Size: ${inst_kb}"
    echo "Maintainer: WASMNux Maintainers <maintainers@wasmnux.local>"
    echo "Description: ${pkg_desc}"
  } > "${root}/DEBIAN/control"

  (
    cd "${root}"
    find usr -type f -exec md5sum {} + | sort -k 2 > DEBIAN/md5sums
  )
  chmod 0644 "${root}/DEBIAN/control" "${root}/DEBIAN/md5sums"

  dpkg-deb --root-owner-group -Zgzip --build "${root}" "${out_deb}" >/dev/null
}

cat > "${PKG_BUILD_DIR}/bin/wasmnux-info" <<'EOF'
#!/bin/sh
set -eu
echo "WASMNUX_NOTES_OK distro=$(. /etc/os-release && printf '%s' "$PRETTY_NAME") kernel=$(uname -r)"
EOF
chmod 0755 "${PKG_BUILD_DIR}/bin/wasmnux-info"

cat > "${PKG_BUILD_DIR}/bin/wasmnux-fbpaint" <<'EOF'
#!/bin/sh
exec /usr/bin/wasmnux-fbtest --debian "$@"
EOF
chmod 0755 "${PKG_BUILD_DIR}/bin/wasmnux-fbpaint"

build_deb_pkg \
  "wasmnux-hello" "1.0.0" "riscv64" "libc6 (>= 2.38)" \
  "WASMNux RISC-V native verification greeting utility" \
  "${PKG_BUILD_DIR}/bin/wasmnux-hello" "wasmnux-hello" \
  "${DEST_REPO_DIR}/pool/main/w/wasmnux-hello/wasmnux-hello_1.0.0_riscv64.deb"

build_deb_pkg \
  "wasmnux-sysbench" "1.0.0" "riscv64" "wasmnux-hello (>= 1.0.0)" \
  "WASMNux RISC-V CPU and memory throughput microbenchmark" \
  "${PKG_BUILD_DIR}/bin/wasmnux-sysbench" "wasmnux-sysbench" \
  "${DEST_REPO_DIR}/pool/main/w/wasmnux-sysbench/wasmnux-sysbench_1.0.0_riscv64.deb"

build_deb_pkg \
  "wasmnux-fbtools" "1.0.0" "riscv64" "" \
  "WASMNux Linux /dev/fb0 framebuffer pattern generator" \
  "${PKG_BUILD_DIR}/bin/wasmnux-fbpaint" "wasmnux-fbpaint" \
  "${DEST_REPO_DIR}/pool/main/w/wasmnux-fbtools/wasmnux-fbtools_1.0.0_riscv64.deb"

build_deb_pkg \
  "wasmnux-notes" "1.0.0" "all" "bash" \
  "WASMNux Debian profile metadata helper" \
  "${PKG_BUILD_DIR}/bin/wasmnux-info" "wasmnux-info" \
  "${DEST_REPO_DIR}/pool/main/w/wasmnux-notes/wasmnux-notes_1.0.0_all.deb"

(
  cd "${DEST_REPO_DIR}"
  dpkg-scanpackages --multiversion pool /dev/null > dists/trixie/main/binary-riscv64/Packages 2>/dev/null
  xz -9 -c dists/trixie/main/binary-riscv64/Packages > dists/trixie/main/binary-riscv64/Packages.xz

  PKG_FILE="main/binary-riscv64/Packages"
  PKGXZ_FILE="main/binary-riscv64/Packages.xz"
  PKG_SZ="$(wc -c < "dists/trixie/${PKG_FILE}" | tr -d ' ')"
  PKGXZ_SZ="$(wc -c < "dists/trixie/${PKGXZ_FILE}" | tr -d ' ')"
  PKG_MD5="$(md5sum "dists/trixie/${PKG_FILE}" | awk '{print $1}')"
  PKGXZ_MD5="$(md5sum "dists/trixie/${PKGXZ_FILE}" | awk '{print $1}')"
  PKG_SHA256="$(sha256sum "dists/trixie/${PKG_FILE}" | awk '{print $1}')"
  PKGXZ_SHA256="$(sha256sum "dists/trixie/${PKGXZ_FILE}" | awk '{print $1}')"

  cat > dists/trixie/Release <<EOR
Origin: WASMNux
Label: WASMNux Debian Repository
Suite: stable
Version: 13.7
Codename: trixie
Date: Sat, 31 Oct 2026 00:00:00 UTC
Architectures: riscv64 all
Components: main
Description: WASMNux browser-bridged APT repository for Debian 13 (trixie) riscv64
MD5Sum:
 ${PKG_MD5} ${PKG_SZ} ${PKG_FILE}
 ${PKGXZ_MD5} ${PKGXZ_SZ} ${PKGXZ_FILE}
SHA256:
 ${PKG_SHA256} ${PKG_SZ} ${PKG_FILE}
 ${PKGXZ_SHA256} ${PKGXZ_SZ} ${PKGXZ_FILE}
EOR
)

# 5. Assemble & format 256 MiB ext4 rootfs inside `unshare -r` so all inodes are owned by root (0:0).
rm -rf "${STAGE_DIR}" "${RAW_EXT4}"
mkdir -p "${STAGE_DIR}"

export DEBIAN_ROOTFS_TAR STAGE_DIR BUSYBOX_BIN PKG_BUILD_DIR DEST_REPO_DIR RAW_EXT4

unshare -r bash -ceu '
  tar --no-same-owner --numeric-owner -xzf "${DEBIAN_ROOTFS_TAR}" -C "${STAGE_DIR}"

  mkdir -p \
    "${STAGE_DIR}/dev" \
    "${STAGE_DIR}/dev/pts" \
    "${STAGE_DIR}/dev/shm" \
    "${STAGE_DIR}/etc/init.d" \
    "${STAGE_DIR}/proc" \
    "${STAGE_DIR}/sys" \
    "${STAGE_DIR}/run" \
    "${STAGE_DIR}/tmp" \
    "${STAGE_DIR}/var/tmp" \
    "${STAGE_DIR}/var/cache/apt/archives/partial" \
    "${STAGE_DIR}/var/cache/wasmnux-offline" \
    "${STAGE_DIR}/etc/apt/apt.conf.d" \
    "${STAGE_DIR}/etc/apt/sources.list.d" \
    "${STAGE_DIR}/etc/profile.d" \
    "${STAGE_DIR}/root"

  chmod 1777 "${STAGE_DIR}/tmp" "${STAGE_DIR}/var/tmp"
  chmod 0700 "${STAGE_DIR}/root"

  # Install BusyBox alongside untouched official Debian GNU/APT/DPKG binaries.
  cp -f "${BUSYBOX_BIN}" "${STAGE_DIR}/usr/bin/busybox"
  chmod 0755 "${STAGE_DIR}/usr/bin/busybox"

  for app in ip ifconfig route ping wget udhcpc ps top free kill killall pgrep pkill \
             less vi clear reset strings hexdump xxd nc netstat ss traceroute \
             setsid cttyhack init halt poweroff reboot lsmod insmod modprobe; do
    if [ ! -e "${STAGE_DIR}/usr/bin/${app}" ] && [ ! -L "${STAGE_DIR}/usr/bin/${app}" ] && \
       [ ! -e "${STAGE_DIR}/usr/sbin/${app}" ] && [ ! -L "${STAGE_DIR}/usr/sbin/${app}" ]; then
      case "${app}" in
        ifconfig|route|init|halt|poweroff|reboot|lsmod|insmod|modprobe)
          ln -s /usr/bin/busybox "${STAGE_DIR}/usr/sbin/${app}"
          ;;
        *)
          ln -s /usr/bin/busybox "${STAGE_DIR}/usr/bin/${app}"
          ;;
      esac
    fi
  done

  # Install framebuffer & diagnostic helpers.
  cp -f "${PKG_BUILD_DIR}/bin/wasmnux-fbtest" "${STAGE_DIR}/usr/bin/wasmnux-fbtest"
  chmod 0755 "${STAGE_DIR}/usr/bin/wasmnux-fbtest"

  cat > "${STAGE_DIR}/usr/bin/wasmnux-diag" <<\EOD
#!/bin/bash
set -euo pipefail
echo "=== WASMNux Debian Diagnostics ==="
. /etc/os-release
echo "Distro:  ${PRETTY_NAME} (Debian $(cat /etc/debian_version))"
echo "Kernel:  $(uname -srm)"
echo "Bash:    $(bash --version | head -n 1)"
echo "APT:     $(apt --version)"
echo "DPKG:    $(dpkg --version | head -n 1)"
echo "Packages installed: $(dpkg-query -f '\''.${binary:Package}\n'\'' -W | wc -l)"
df -hT /
EOD
  chmod 0755 "${STAGE_DIR}/usr/bin/wasmnux-diag"

  # Cache offline .deb package so dpkg -i works even when networkEnabled=false.
  cp -f "${DEST_REPO_DIR}/pool/main/w/wasmnux-hello/wasmnux-hello_1.0.0_riscv64.deb" \
    "${STAGE_DIR}/var/cache/wasmnux-offline/wasmnux-hello_1.0.0_riscv64.deb"

  # Configure passwordless root account for `login -f root` / `su`.
  sed -i "s|^root:[^:]*:|root::|" "${STAGE_DIR}/etc/passwd"
  if [ -f "${STAGE_DIR}/etc/shadow" ]; then
    sed -i "s|^root:[^:]*:|root::|" "${STAGE_DIR}/etc/shadow"
  fi

  echo "wasmnux-debian" > "${STAGE_DIR}/etc/hostname"
  cat > "${STAGE_DIR}/etc/hosts" <<\EOH
127.0.0.1       localhost wasmnux-debian
10.0.2.2        gateway.wasmnux.local apt.wasmnux.local
10.0.2.15       wasmnux-debian.local
EOH

  cat > "${STAGE_DIR}/etc/resolv.conf" <<\EOR
nameserver 10.0.2.3
EOR

  cat > "${STAGE_DIR}/etc/fstab" <<\EOFSTAB
/dev/vda  /          ext4     rw,relatime  0 1
proc      /proc      proc     defaults     0 0
sysfs     /sys       sysfs    defaults     0 0
devtmpfs  /dev       devtmpfs mode=0755    0 0
devpts    /dev/pts   devpts   gid=5,mode=620 0 0
tmpfs     /run       tmpfs    mode=0755,nosuid,nodev 0 0
tmpfs     /tmp       tmpfs    mode=1777,nosuid,nodev 0 0
EOFSTAB

  # Configure APT to use the browser-bridged local/same-origin Debian repository cleanly.
  if [ -f "${STAGE_DIR}/etc/apt/sources.list.d/debian.sources" ]; then
    mv "${STAGE_DIR}/etc/apt/sources.list.d/debian.sources" \
      "${STAGE_DIR}/etc/apt/sources.list.d/debian-upstream.sources.disabled"
  fi
  rm -f "${STAGE_DIR}/etc/apt/sources.list"

  cat > "${STAGE_DIR}/etc/apt/sources.list.d/wasmnux.list" <<\EOSRC
# Browser-bridged same-origin Debian 13 (trixie) riscv64 repository served via VirtIO-Net eth0:
deb [trusted=yes] http://apt.wasmnux.local/debian-repo trixie main
EOSRC

  cat > "${STAGE_DIR}/etc/apt/README.wasmnux" <<\EOREADME
WASMNux APT Networking Note:
Guest `apt` communicates over real Linux TCP/IP (`eth0` VirtIO-Net -> WASMNux VirtualNetworkSwitch).
Because web browsers enforce Same-Origin Policy (CORS) on `fetch()` requests and `deb.debian.org`
does not emit `Access-Control-Allow-Origin: *`, `http://apt.wasmnux.local/debian-repo` is mapped
to the same-origin `/debian-repo/` APT mirror containing genuine riscv64 `.deb` packages:
  - wasmnux-hello
  - wasmnux-sysbench (depends on wasmnux-hello)
  - wasmnux-fbtools
  - wasmnux-notes
Offline `.deb` archives are also available in `/var/cache/wasmnux-offline/`.
EOREADME

  cat > "${STAGE_DIR}/etc/apt/apt.conf.d/99wasmnux" <<\EOAPT
APT::Sandbox::User "root";
Acquire::http::Pipeline-Depth "0";
Acquire::Languages "none";
Acquire::Retries "1";
Dpkg::Use-Pty "0";
EOAPT

  cat > "${STAGE_DIR}/etc/profile.d/wasmnux.sh" <<\EOPROF
export HOME="${HOME:-/root}"
export USER="${USER:-root}"
export LOGNAME="${LOGNAME:-root}"
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export TERM="${TERM:-xterm-256color}"
export PS1="\u@\h:\w# "
cd "$HOME" 2>/dev/null || true
EOPROF
  chmod 0644 "${STAGE_DIR}/etc/profile.d/wasmnux.sh"

  cat > "${STAGE_DIR}/root/WELCOME.txt" <<\EOWEL
Welcome to Official Debian GNU/Linux 13.7 ("trixie") riscv64 on WASMNux!
Try:
  cat /etc/os-release
  apt-get update && apt-get install -y wasmnux-sysbench
  wasmnux-hello
  wasmnux-sysbench
  wasmnux-fbpaint
  dpkg -l
Files and installed packages on /dev/vda (ext4) persist across reboots and browser reloads.
EOWEL

  cat > "${STAGE_DIR}/etc/inittab" <<\EOINIT
::sysinit:/etc/init.d/rcS
ttyS0::respawn:-/bin/bash
::ctrlaltdel:/usr/sbin/reboot
::shutdown:/etc/init.d/rcK
EOINIT

  cat > "${STAGE_DIR}/etc/init.d/rcK" <<\EOSHUT
#!/bin/sh
echo "[wasmnux-debian] Syncing and remounting /dev/vda read-only..."
sync
mount -o remount,ro / 2>/dev/null || true
sync
EOSHUT
  chmod 0755 "${STAGE_DIR}/etc/init.d/rcK"

  cat > "${STAGE_DIR}/etc/init.d/rcS" <<\EOPID1
#!/bin/sh
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

mount -t proc proc /proc 2>/dev/null || true
mount -t sysfs sysfs /sys 2>/dev/null || true
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
mkdir -p /dev/pts /dev/shm /run /tmp /var/lock
mount -t devpts devpts /dev/pts 2>/dev/null || true
mount -t tmpfs -o mode=0755 tmpfs /run 2>/dev/null || true
mount -t tmpfs -o mode=1777 tmpfs /tmp 2>/dev/null || true

mount -o remount,rw / 2>/dev/null || true
hostname -F /etc/hostname 2>/dev/null || true

ip link set lo up 2>/dev/null || true
ip addr add 127.0.0.1/8 dev lo 2>/dev/null || true
if ip link show eth0 >/dev/null 2>&1; then
  ip link set eth0 up 2>/dev/null || true
  ip addr add 10.0.2.15/24 dev eth0 2>/dev/null || true
  ip route add default via 10.0.2.2 dev eth0 2>/dev/null || true
fi

if [ -c /dev/fb0 ] && [ -x /usr/bin/wasmnux-fbtest ]; then
  /usr/bin/wasmnux-fbtest --debian >/dev/null 2>&1 || true
fi

DEB_FULL="$(cat /etc/debian_version 2>/dev/null || echo 13)"
printf "\n\033[1;35mWASMNux Debian GNU/Linux %s (trixie)\033[0m · Linux %s (%s)\n" \
  "${DEB_FULL}" "$(uname -r)" "$(uname -m)"
echo "Rootfs: /dev/vda (ext4 persistent) · APT repo: http://apt.wasmnux.local/debian-repo"
echo "WASMNUX_DEBIAN_BOOT_COMPLETE"
EOPID1
  chmod 0755 "${STAGE_DIR}/etc/init.d/rcS"

  ln -sf /usr/sbin/init "${STAGE_DIR}/sbin/wasmnux-init"

  truncate -s 256M "${RAW_EXT4}"
  /usr/sbin/mkfs.ext4 -q -F -b 4096 -L "wasmnux-debian" -E root_owner=0:0 -d "${STAGE_DIR}" "${RAW_EXT4}"
'

/usr/sbin/e2fsck -fy "${RAW_EXT4}" >/dev/null 2>&1 || true
gzip -9 -n -c "${RAW_EXT4}" > "${DEST_IMAGES_DIR}/debian-rootfs.ext4.gz"
rm -rf "${STAGE_DIR}" "${RAW_EXT4}"

echo "[build-debian-rootfs] SUCCESS:"
ls -lh "${DEST_IMAGES_DIR}/debian-rootfs.ext4.gz"
ls -lh "${DEST_REPO_DIR}/dists/trixie/Release" "${DEST_REPO_DIR}/dists/trixie/main/binary-riscv64/"
