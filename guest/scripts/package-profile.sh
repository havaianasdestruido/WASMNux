#!/usr/bin/env bash
# Assembles one profile's web asset directory + manifest.json, in the
# exact shape web/src/profiles.ts validates. The resulting directory is
# dropped under web/public/guest/<profile>/ (or served by CI artifacts).
#
# Usage:
#   package-profile.sh vanilla <kernel-dir> <opensbi-dir> <initramfs> <out-dir>
#   package-profile.sh Debian  <kernel-dir> <opensbi-dir> <disk-img> <out-dir>
set -euo pipefail

PROFILE="${1:?profile}"; KERNEL_DIR="${2:?kernel dir}"; SBI_DIR="${3:?opensbi dir}"
EXTRA="${4:?initramfs or disk image}"; OUT_DIR="${5:?out dir}"
mkdir -p "${OUT_DIR}"

KERNEL_VERSION="$(cat "${KERNEL_DIR}/kernel.version")"
cp "${KERNEL_DIR}/Image" "${OUT_DIR}/Image"
cp "${SBI_DIR}/fw_jump.elf" "${OUT_DIR}/fw_jump.elf"

EXTRA_FIELD=""
case "${PROFILE}" in
  vanilla)
    cp "${EXTRA}" "${OUT_DIR}/initramfs.cpio.gz"
    EXTRA_FIELD='"initrdImage": "initramfs.cpio.gz",'
    ;;
  Debian)
    cp "${EXTRA}" "${OUT_DIR}/debian-root.img"
    EXTRA_FIELD='"diskImage": "debian-root.img",'
    ;;
  *) echo "unknown profile ${PROFILE}" >&2; exit 2 ;;
esac

cat > "${OUT_DIR}/manifest.json" <<EOF
{
  "profile": "${PROFILE}",
  "kernelVersion": "${KERNEL_VERSION}",
  "kernelImage": "Image",
  "firmwareImage": "fw_jump.elf",
  ${EXTRA_FIELD}
  "builtAt": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "notes": "Genuine upstream guests; see docs/DEPENDENCIES.md for provenance."
}
EOF
echo "Packaged ${PROFILE} (kernel ${KERNEL_VERSION}) -> ${OUT_DIR}"
ls -l "${OUT_DIR}"
