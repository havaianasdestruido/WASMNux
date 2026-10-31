#!/bin/sh
# Debian guest smoke tests — run INSIDE the Debian profile (as root on the
# serial console). Every check executes the real Debian tooling; package
# operations use a locally-built .deb so they are meaningful even before
# the Phase-9 network relay exists for `apt-get update`.
set -u
PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); echo "PASS: $1"; }
bad() { FAIL=$((FAIL+1)); echo "FAIL: $1"; }

# Identity: this must really be the latest Debian stable, on riscv64.
. /etc/os-release
[ "${ID:-}" = "Debian" ] && ok "os-release ID=Debian" || bad "os-release ID=Debian"
case "${VERSION_ID:-}" in 13*) ok "Debian major is 13 (trixie)" ;; *) bad "Debian major is 13, got ${VERSION_ID:-?}" ;; esac
[ "$(uname -m)" = "riscv64" ] && ok "uname -m is riscv64" || bad "uname -m is riscv64"
uname -r | grep -q . && ok "uname -r reports a kernel release" || bad "uname -r"
ps aux | grep -q . && ok "ps lists processes" || bad "ps"
df / | tail -n +2 | grep -q . && ok "df sees the root filesystem" || bad "df"
free | grep -q Mem && ok "free reports memory" || bad "free"
busybox true 2>/dev/null && ok "busybox executes" || bad "busybox executes"
[ "$(busybox sh -c 'echo $((20+3))')" = "23" ] && ok "busybox sh computes" || bad "busybox sh computes"

# dpkg / apt must be the real tools with a consistent database.
dpkg --version >/dev/null && ok "dpkg runs" || bad "dpkg runs"
apt-get --version >/dev/null && ok "apt-get runs" || bad "apt-get runs"
dpkg -l | grep -q '^ii' && ok "dpkg database has installed packages" || bad "dpkg database"
dpkg --audit >/dev/null 2>&1 && ok "dpkg --audit clean" || bad "dpkg --audit"
apt-cache policy bash 2>/dev/null | grep -q Installed && ok "apt-cache policy answers" || bad "apt-cache policy"

# Real package lifecycle with a package built locally (offline-safe).
TMP="$(mktemp -d)"
mkdir -p "${TMP}/pkg/DEBIAN" "${TMP}/pkg/usr/share/wasmnux-probe"
printf 'installed-by-dpkg-test\n' > "${TMP}/pkg/usr/share/wasmnux-probe/marker.txt"
cat > "${TMP}/pkg/DEBIAN/control" <<'EOF'
Package: wasmnux-probe
Version: 1.0
Section: misc
Priority: optional
Architecture: all
Maintainer: WASMNux tests <tests@example.invalid>
Description: Behaviour probe package for guest package-management tests.
EOF
if dpkg-deb --build "${TMP}/pkg" "${TMP}/probe.deb" >/dev/null 2>&1; then
  ok "dpkg-deb builds a package"
else
  bad "dpkg-deb builds a package"
fi
dpkg -i "${TMP}/probe.deb" >/dev/null && ok "dpkg installs the probe" || bad "dpkg installs the probe"
[ "$(cat /usr/share/wasmnux-probe/marker.txt 2>/dev/null)" = "installed-by-dpkg-test" ] \
  && ok "installed file content is real" || bad "installed file content"
dpkg-query -W -f='${Status}' wasmnux-probe | grep -q "install ok installed" \
  && ok "dpkg-query reports installed" || bad "dpkg-query reports installed"
# Record for the cross-reboot persistence test harness.
echo "wasmnux-probe" > /root/.wasmnux-expected-package
dpkg -r wasmnux-probe >/dev/null && ok "dpkg removes the probe" || bad "dpkg removes the probe"
[ ! -e /usr/share/wasmnux-probe/marker.txt ] && ok "removal deletes files" || bad "removal deletes files"
sync

echo "== Debian smoke: PASS=${PASS} FAIL=${FAIL} =="
[ "${FAIL}" -eq 0 ]
