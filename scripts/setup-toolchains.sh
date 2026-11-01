#!/usr/bin/env bash
set -euo pipefail

TOOLS_DIR="${WASMNUX_TOOLS_DIR:-/tmp/wasmnux-tools}"
RUST_DIR="${TOOLS_DIR}/rust"
LLVM_DIR="${TOOLS_DIR}/llvm"
DL_DIR="${TOOLS_DIR}/dl"
UNPACK_DIR="${TOOLS_DIR}/unpack"

mkdir -p "${RUST_DIR}" "${LLVM_DIR}" "${DL_DIR}" "${UNPACK_DIR}"

# `rustc --print target-libdir --target wasm32-unknown-unknown` prints a path even when
# the target's `std` is absent, so it cannot be used to decide whether the target is
# usable. Compile a `std`-using program instead; that only succeeds when it really is.
wasm_std_available() {
  local probe
  probe="$(mktemp -d)"
  printf 'fn main() { println!("ok"); }\n' > "${probe}/probe.rs"
  if rustc --target wasm32-unknown-unknown --emit=metadata \
       -o "${probe}/probe.metadata" "${probe}/probe.rs" >/dev/null 2>&1; then
    rm -rf "${probe}"
    return 0
  fi
  rm -rf "${probe}"
  return 1
}

if command -v rustup >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
  echo "[setup-toolchains] Ensuring wasm32-unknown-unknown + rustfmt + clippy via rustup..."
  rustup target add wasm32-unknown-unknown
  rustup component add rustfmt clippy || true
elif ! command -v rustc >/dev/null 2>&1 || ! rustc --print target-libdir --target wasm32-unknown-unknown >/dev/null 2>&1; then
  echo "[setup-toolchains] Installing Rust 1.88.0 + wasm32-unknown-unknown + rustfmt + clippy + llvm-tools..."
  pushd "${DL_DIR}" >/dev/null
  rm -f ./*.tgz
  for pkg in \
    "@rustbin/rustc-1.88.0-x86_64-unknown-linux-gnu@1.88.0" \
    "@rustbin/cargo-1.88.0-x86_64-unknown-linux-gnu@0.89.0" \
    "@rustbin/rustfmt-1.88.0-x86_64-unknown-linux-gnu@1.8.0" \
    "@rustbin/clippy-1.88.0-x86_64-unknown-linux-gnu@0.1.88" \
    "@rustbin/rust-std-1.88.0-x86_64-unknown-linux-gnu@1.88.0" \
    "@rustbin/rust-std-1.88.0-wasm32-unknown-unknown@1.88.0" \
    "@rustbin/llvm-tools-1.88.0-x86_64-unknown-linux-gnu@1.88.0"
  do
    npm pack "${pkg}" --quiet
  done
  for tgz in "${DL_DIR}"/rustbin-*.tgz; do
    rm -rf "${UNPACK_DIR:?}"/*
    tar -xzf "${tgz}" -C "${UNPACK_DIR}"
    bash "${UNPACK_DIR}/package/install.sh" --prefix="${RUST_DIR}" --disable-ldconfig >/dev/null
  done
  rm -f "${DL_DIR}"/*.tgz
  popd >/dev/null
  if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    sudo ln -sf "${RUST_DIR}"/bin/* /usr/local/bin/
  fi
fi

if ! command -v clang >/dev/null 2>&1 || ! command -v ld.lld >/dev/null 2>&1; then
  echo "[setup-toolchains] Installing LLVM/Clang 23.1.3 + LLD..."
  pushd "${DL_DIR}" >/dev/null
  rm -f ./*.tgz
  npm pack "llvm-runtime-clang-linux-x64@23.1.3" --quiet
  npm pack "llvm-runtime-lld-linux-x64@23.1.3" --quiet
  for tgz in "${DL_DIR}"/*.tgz; do
    rm -rf "${UNPACK_DIR:?}"/*
    tar -xzf "${tgz}" -C "${UNPACK_DIR}"
    cp -rf "${UNPACK_DIR}"/package/* "${LLVM_DIR}/"
  done
  rm -f "${DL_DIR}"/*.tgz
  popd >/dev/null
  if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    sudo ln -sf "${LLVM_DIR}"/bin/* /usr/local/bin/
  fi
fi

if ! command -v zig >/dev/null 2>&1; then
  echo "[setup-toolchains] Installing Zig 0.17.0 cross-compiler..."
  pip3 install --quiet --break-system-packages "ziglang==0.17.0"
  if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    printf '#!/bin/sh\nexec python3 -m ziglang "$@"\n' | sudo tee /usr/local/bin/zig >/dev/null
    sudo chmod +x /usr/local/bin/zig
  fi
fi

echo "[setup-toolchains] Verified toolchains:"
echo "  rustc:   $(rustc --version)"
echo "  cargo:   $(cargo --version)"
echo "  rustfmt: $(cargo fmt --version)"
echo "  clippy:  $(cargo clippy --version)"
echo "  clang:   $(clang --version | head -n 1)"
echo "  lld:     $(ld.lld --version)"
echo "  zig:     $(zig version)"

if wasm_std_available; then
  echo "  wasm32:  usable (std probes compile)"
else
  echo "[setup-toolchains] ERROR: rustc cannot compile for wasm32-unknown-unknown." >&2
  echo "  rustc: $(command -v rustc) ($(rustc --version 2>/dev/null || echo unknown))" >&2
  exit 1
fi
