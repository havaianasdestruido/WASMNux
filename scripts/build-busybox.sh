#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="/tmp/wasmnux-tools/llvm/bin:/tmp/wasmnux-tools/host/bin:/usr/local/bin:/usr/sbin:/sbin:$PATH"

BUSYBOX_TAG="1_36_1"
BUSYBOX_REPO="https://github.com/mirror/busybox.git"
BUILD_ROOT="${WASMNUX_GUEST_BUILD_DIR:-/tmp/wasmnux-guest-build}"
SRC_DIR="${BUILD_ROOT}/busybox-1.36.1"
WRAP_BIN="${BUILD_ROOT}/rv64-musl-wrap"
OUT_BIN="${BUILD_ROOT}/busybox-riscv64-static"

mkdir -p "${BUILD_ROOT}" "${WRAP_BIN}"

# Create deterministic Zig + LLVM cross-toolchain wrappers for BusyBox Kbuild
cat > "${WRAP_BIN}/riscv64-linux-musl-gcc" <<'EOF'
#!/usr/bin/env bash
filtered=()
for arg in "$@"; do
  case "$arg" in
    -Wl,--warn-common|-Wl,--sort-common|-Wl,--sort-section,*|-Wl,-Map,*|-Wl,--verbose|-finline-limit=*|-falign-jumps=*|-falign-labels=*|-static-libgcc)
      ;;
    *)
      filtered+=("$arg")
      ;;
  esac
done
exec zig cc -target riscv64-linux-musl -mcpu=generic_rv64+m+a+f+d+c -DBB_GLOBAL_CONST= -Wno-error "${filtered[@]}"
EOF

cat > "${WRAP_BIN}/riscv64-linux-musl-ar" <<'EOF'
#!/usr/bin/env bash
exec llvm-ar "$@"
EOF

cat > "${WRAP_BIN}/riscv64-linux-musl-ranlib" <<'EOF'
#!/usr/bin/env bash
exec llvm-ranlib "$@"
EOF

cat > "${WRAP_BIN}/riscv64-linux-musl-nm" <<'EOF'
#!/usr/bin/env bash
exec llvm-nm "$@"
EOF

cat > "${WRAP_BIN}/riscv64-linux-musl-objcopy" <<'EOF'
#!/usr/bin/env bash
exec llvm-objcopy "$@"
EOF

cat > "${WRAP_BIN}/riscv64-linux-musl-objdump" <<'EOF'
#!/usr/bin/env bash
exec llvm-objdump "$@"
EOF

cat > "${WRAP_BIN}/riscv64-linux-musl-strip" <<'EOF'
#!/usr/bin/env bash
exec llvm-strip "$@"
EOF

chmod +x "${WRAP_BIN}/riscv64-linux-musl-"*
export PATH="${WRAP_BIN}:${PATH}"

if [ ! -d "${SRC_DIR}/.git" ]; then
  echo "[build-busybox] Cloning official BusyBox ${BUSYBOX_TAG} from ${BUSYBOX_REPO}..."
  rm -rf "${SRC_DIR}"
  git clone --depth 1 --branch "${BUSYBOX_TAG}" "${BUSYBOX_REPO}" "${SRC_DIR}"
fi

echo "[build-busybox] Configuring BusyBox ${BUSYBOX_TAG} (commit $(git -C "${SRC_DIR}" rev-parse HEAD))..."
if [ ! -f "${SRC_DIR}/.config" ]; then
  make -C "${SRC_DIR}" HOSTCC=gcc CROSS_COMPILE=riscv64-linux-musl- defconfig
fi

# Configure static musl build & avoid deprecated Linux >= 6.8 tc CBQ / x86 SHA-NI inline asm
sed -i 's/^# CONFIG_STATIC is not set/CONFIG_STATIC=y/' "${SRC_DIR}/.config"
sed -i 's/^CONFIG_TC=y/# CONFIG_TC is not set/' "${SRC_DIR}/.config"
sed -i 's/^CONFIG_SHA1_HWACCEL=y/# CONFIG_SHA1_HWACCEL is not set/' "${SRC_DIR}/.config"
sed -i 's/^CONFIG_SHA256_HWACCEL=y/# CONFIG_SHA256_HWACCEL is not set/' "${SRC_DIR}/.config"
sed -i 's/^CONFIG_FEATURE_INETD_RPC=y/# CONFIG_FEATURE_INETD_RPC is not set/' "${SRC_DIR}/.config"
(yes "" || true) | make -C "${SRC_DIR}" HOSTCC=gcc CROSS_COMPILE=riscv64-linux-musl- oldconfig >/dev/null

JOBS="$(nproc 2>/dev/null || echo 2)"
echo "[build-busybox] Compiling static riscv64-linux-musl BusyBox with ${JOBS} jobs..."
make -C "${SRC_DIR}" HOSTCC=gcc CROSS_COMPILE=riscv64-linux-musl- SKIP_STRIP=n -j"${JOBS}"

cp -f "${SRC_DIR}/busybox" "${OUT_BIN}"
llvm-strip "${OUT_BIN}"

# Verify 64-bit RISC-V static ELF header (no PT_INTERP dynamic loader segment)
READELF_OUT="$(llvm-readelf -h -l "${OUT_BIN}")"
if ! echo "${READELF_OUT}" | grep -q "Class:.*ELF64"; then
  echo "[build-busybox] ERROR: BusyBox is not ELF64!" >&2
  exit 1
fi
if ! echo "${READELF_OUT}" | grep -q "Machine:.*RISC-V"; then
  echo "[build-busybox] ERROR: BusyBox is not RISC-V!" >&2
  exit 1
fi
if echo "${READELF_OUT}" | grep -q "INTERP"; then
  echo "[build-busybox] ERROR: BusyBox has PT_INTERP (must be statically linked)!" >&2
  exit 1
fi

# Verify mandatory applets enabled in BusyBox .config / applet table
MANDATORY_APPLETS=(
  CONFIG_SH_IS_ASH
  CONFIG_ASH
  CONFIG_LS
  CONFIG_CAT
  CONFIG_CP
  CONFIG_MV
  CONFIG_RM
  CONFIG_MKDIR
  CONFIG_RMDIR
  CONFIG_MOUNT
  CONFIG_UMOUNT
  CONFIG_PS
  CONFIG_KILL
  CONFIG_DMESG
  CONFIG_UNAME
  CONFIG_FREE
  CONFIG_DF
  CONFIG_DU
  CONFIG_GREP
  CONFIG_SED
  CONFIG_AWK
  CONFIG_FIND
  CONFIG_TAR
  CONFIG_GZIP
  CONFIG_TOUCH
  CONFIG_CHMOD
  CONFIG_CHOWN
  CONFIG_LN
  CONFIG_READLINK
  CONFIG_DATE
  CONFIG_SLEEP
  CONFIG_TOP
  CONFIG_HALT
  CONFIG_SETSID
  CONFIG_CTTYHACK
  CONFIG_SWITCH_ROOT
  CONFIG_WGET
  CONFIG_IP
  CONFIG_PING
)

for sym in "${MANDATORY_APPLETS[@]}"; do
  if ! grep -q "^${sym}=y$" "${SRC_DIR}/.config"; then
    echo "[build-busybox] ERROR: Mandatory BusyBox config ${sym}=y missing!" >&2
    exit 1
  fi
done

SIZE_BIN="$(stat -c '%s' "${OUT_BIN}")"
SHA_BIN="$(sha256sum "${OUT_BIN}" | awk '{print $1}')"
echo "[build-busybox] SUCCESS: ${OUT_BIN} (${SIZE_BIN} bytes, ELF64 RISC-V static, sha256: ${SHA_BIN})"
