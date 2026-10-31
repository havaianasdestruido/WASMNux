#!/usr/bin/env bash
# Build the 256 MiB ext2 Debian disk from the pinned Debuerreotype riscv64 OCI
# rootfs layer. Verify the layer digest before extraction.
set -euo pipefail
layer=${1:?rootfs.tar.gz}; out=${2:?debian.img}
echo '9d924bd60c86d7c78accc40bd01b73b855edec5d6d502ec982d0f667445094c9  '"$layer" | sha256sum -c -
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
mkdir -p "$work/root"; tar -xzf "$layer" -C "$work/root"
cp tools/debian/wnx-init.sh "$work/root/wnx-init.sh"; chmod 755 "$work/root/wnx-init.sh"
mkdir -p "$work/root/opt/packages"; cp tools/debian/*.deb "$work/root/opt/packages/"
truncate -s 256M "$out"
/sbin/mkfs.ext2 -q -F -d "$work/root" -L WASMNUX-Debian "$out"
