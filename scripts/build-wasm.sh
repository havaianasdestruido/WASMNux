#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

cargo build --package wasmnux-wasm --target wasm32-unknown-unknown --release

mkdir -p "${ROOT_DIR}/public/wasm"
cp -f "${ROOT_DIR}/target/wasm32-unknown-unknown/release/wasmnux_wasm.wasm" \
      "${ROOT_DIR}/public/wasm/wasmnux_wasm.wasm"

SIZE_BYTES="$(wc -c < "${ROOT_DIR}/public/wasm/wasmnux_wasm.wasm")"
echo "[build-wasm] Built public/wasm/wasmnux_wasm.wasm (${SIZE_BYTES} bytes)"
