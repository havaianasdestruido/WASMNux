#!/usr/bin/env bash
# Resolves "latest stable" versions from official upstreams at build time.
# Nothing is hardcoded: the resolved values are printed and also recorded
# into each profile's manifest by the build scripts.
#
# Verified 2026-10-31 (audit trail):
#   Linux kernel  -> 7.3.1  (kernel.org releases.json, "stable" moniker)
#   Debian        -> 13 "trixie" (first stable with official riscv64)
set -euo pipefail

resolve_kernel() {
  curl -fsSL https://www.kernel.org/releases.json \
    | python3 -c '
import json, sys
data = json.load(sys.stdin)
stable = [r for r in data["releases"] if r["moniker"] == "stable" and not r["iseol"]]
# kernel.org lists the newest series first; pick the highest version.
def key(v):
    return [int(x) for x in v.split(".")]
best = max(stable, key=lambda r: key(r["version"]))
print(best["version"])
print(best["source"], file=sys.stderr)
'
}

resolve_Debian() {
  # Prints "VERSION CODENAME" e.g. "13 trixie".
  curl -fsSL https://deb.debian.org/debian/dists/stable/Release \
    | awk -F': ' '
        /^Version:/ { v = $2 }
        /^Codename:/ { c = $2 }
        END { print v, c }'
}

resolve_busybox() {
  curl -fsSL https://busybox.net/downloads/ \
    | grep -oE 'busybox-[0-9]+\.[0-9]+(\.[0-9]+)?\.tar\.bz2' \
    | sed -E 's/^busybox-//; s/\.tar\.bz2$//' \
    | sort -V | tail -n1
}

resolve_opensbi() {
  curl -fsSL https://api.github.com/repos/riscv-software-src/opensbi/releases/latest \
    | grep -oE '"tag_name": *"v[^"]+"' | head -n1 | grep -oE 'v[0-9.]+'
}

case "${1:-all}" in
  kernel)  resolve_kernel ;;
  Debian)  resolve_Debian ;;
  busybox) resolve_busybox ;;
  opensbi) resolve_opensbi ;;
  all)
    echo "kernel=$(resolve_kernel)"
    echo "Debian=$(resolve_Debian)"
    echo "busybox=$(resolve_busybox)"
    echo "opensbi=$(resolve_opensbi)"
    ;;
  *) echo "usage: $0 [kernel|Debian|busybox|opensbi|all]" >&2; exit 2 ;;
esac
