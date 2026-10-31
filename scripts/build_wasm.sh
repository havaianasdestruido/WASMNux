#!/bin/sh
# Builds the historical diagnostic ABI; this is not a production profile.
set -eu
cd "$(dirname "$0")/.."
cargo build --locked --release --target wasm32-unknown-unknown
mkdir -p public/guest
cp target/wasm32-unknown-unknown/release/wasmnux_diagnostic.wasm public/guest/wasmnux_diagnostic.wasm
printf '%s\n' 'Built public/guest/wasmnux_diagnostic.wasm (volatile historical diagnostic only).'
