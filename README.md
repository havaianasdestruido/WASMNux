# WASMNux

**Early prototype — the requested Linux and Debian environments are not implemented.** The only bootable guest is a real but obsolete, offline Linux 5.4 / BusyBox diagnostic in a Rust/Wasm RISC-V emulator. Its disk is **volatile**: stopping, refreshing, or closing the page discards every guest change. The emulator's instruction support remains incomplete, so other guest commands can still trap. This diagnostic is not a substitute for either requested profile.

The independently tested IndexedDB block store is **not connected to the guest**. The WebGPU/Canvas display check renders a **synthetic** pattern, not a guest framebuffer. There is no Debian image, `apt` bridge, guest persistence, disk import/export, or offline PWA yet. See [TODO.MD](TODO.MD) and [the test report](docs/TEST_REPORT.md) for exact status.

## Run the diagnostic

Requirements: Node.js 22+, npm, Python 3, Rust via rustup, a modern browser, and access to GitHub/npm and the Rust toolchain distribution. `rust-toolchain.toml` pins Rust 1.99.0 and the Wasm target. The historical guest images total about 60 MiB and are downloaded separately; they are not committed.

```sh
npm ci
python3 scripts/fetch_probe.py
npm run build:wasm
npm run dev -- --port 4173
```

Open the URL printed by Vite. Select **Boot diagnostic**, wait for `Please press Enter to activate this console`, click the terminal and press Enter. For example, `uname -s` executes in the actual guest. **Stop** immediately terminates the worker and discards the disk; it is not a graceful poweroff. Use a secure context (localhost or HTTPS) for Web Crypto and browser storage APIs.

The pinned fetcher verifies the exact size and SHA-256 of four historical files before installation; the locally built emulator uses only the firmware and root filesystem. These pins identify what was fetched, but are not signed release verification or proof of reproducible guest builds. The development and production Vite build serve/copy locally fetched guest binaries; they must not be redistributed as a release until the source and licensing questions in [third-party notices](THIRD_PARTY_NOTICES.md) are resolved. Do not expose the Vite development server to an untrusted network.

## Tests

```sh
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --manifest-path vendor/riscv-rust/Cargo.toml --locked
npm run format:check
npm run lint
npm run typecheck
npm test
python3 -m unittest discover -s tests -p 'test_*.py' -v
npm run build:wasm
npm run build
npm audit --audit-level=low
npx playwright install chromium  # once, where needed
npm run test:browser
```

Playwright starts Vite automatically or reuses a local server on port 4173. `PLAYWRIGHT_CHROMIUM_EXECUTABLE` can point at an existing Chromium executable. The latest Chromium run passed the historical guest utility test, real invalid-instruction trap/restart, standalone IndexedDB, and WebGPU/Canvas tests. The WebGPU test explicitly requests Chromium's software adapter; it is not a hardware GPU compatibility claim. A passing diagnostic suite does **not** prove current Linux, Debian, `apt`, or persistent guest storage.

## Design and status

- [Architecture, boundaries and security](docs/ARCHITECTURE.md)
- [Phase-by-phase acceptance plan](TODO.MD)
- [Commands, results and outstanding tests](docs/TEST_REPORT.md)
- [Sources, licenses and provenance](THIRD_PARTY_NOTICES.md)
