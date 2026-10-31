#!/usr/bin/env bash
# Builds the Rust VM core to WebAssembly and installs the wasm-bindgen
# package where the web app loads it from (/wasm/wasmnux_wasm.js).
# Requires: rustup with the wasm32-unknown-unknown target; wasm-bindgen-cli
# is installed at the version pinned by Cargo.lock when missing.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT}"

if ! rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown; then
  rustup target add wasm32-unknown-unknown
fi

cargo build -p wasmnux-wasm --target wasm32-unknown-unknown --release

BINDGEN_VERSION="$(python3 - <<'PY'
import re
lock = open("Cargo.lock").read()
match = re.search(r'name = "wasm-bindgen"\nversion = "([^"]+)"', lock)
print(match.group(1) if match else "")
PY
)"
if ! command -v wasm-bindgen >/dev/null; then
  if [ -n "${BINDGEN_VERSION}" ]; then
    cargo install wasm-bindgen-cli --version "${BINDGEN_VERSION}" --locked
  else
    cargo install wasm-bindgen-cli --locked
  fi
fi

mkdir -p web/public/wasm
wasm-bindgen --target web --out-dir web/public/wasm \
  target/wasm32-unknown-unknown/release/wasmnux_wasm.wasm
echo "WASM package -> web/public/wasm/"
ls -l web/public/wasm
