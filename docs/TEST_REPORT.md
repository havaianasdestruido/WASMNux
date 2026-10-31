# Test evidence

Updated 2026-10-31. **Not release-ready.** A pass applies only to the observation described, never to a broader unexecuted requirement. The current Linux and Debian profiles do not exist yet.

## Environment and complete suite

Test host: Linux x86-64, Rust/Cargo 1.99.0, Node `v22.22.3`, npm `10.9.8`, Python `3.11.2`, and Chromium `153.0.8010.0`. Chromium, NSS/NSPR, SwiftShader and the Rust toolchain were temporary sandbox installations, not checked into the repository. Vite served `0.0.0.0:4173`.

```sh
PATH=/tmp/wasmnux-rust/bin:$PATH npm run build:wasm
PLAYWRIGHT_CHROMIUM_EXECUTABLE=/tmp/wasmnux-browser/chromium \
  LD_LIBRARY_PATH=/tmp/wasmnux-browser/al2023/lib:/tmp/wasmnux-browser \
  npm run test:browser
```

The latest complete run: **22 passed, 0 failed** in about 2.7 minutes. It includes a real historical guest utility script, a real intentionally invalid RV64 instruction and fresh boot after the trap, browser instantiation of the locally built Wasm, standalone IndexedDB tests, and synthetic WebGPU/Canvas tests. No requested production profile is booted. An earlier run in this session had **21 passed, 1 failed** because the reconstructed Chromium installation could not locate its SwiftShader libraries; putting those libraries next to the executable fixed the environment, and both the targeted framebuffer test and complete suite were rerun successfully.

The original prebuilt diagnostic failed at BusyBox `awk`; after adding `FCVT.L.D`, it advanced to `sleep` and trapped at the missing `FCVT.D.LU`. Both instructions now have decoder, rounding, flag and boundary tests; the original utility expectations were retained and pass. The error-handling tests now execute a separate tiny RV64 ELF with an intentionally invalid instruction, preserving a genuine emulator-trap regression rather than relying on a formerly broken applet.

| ID | Scope and exact command/procedure | Expected and actual | Status |
| --- | --- | --- | --- |
| AUDIT-01 | `git status --short --branch; git ls-files; ls -la` on the initial checkout | Only the Apache `LICENSE` was tracked; clean starting branch | PASS |
| ARCH-01 | Review upstream kernel architecture and emulator documentation | An upstream Linux kernel executes as a guest, not as a native WASI/Wasm program | PASS (design review) |
| ARCH-02 | Inspect pinned upstream sources, licenses, devices and activity | Candidate comparisons and limitations recorded in `ARCHITECTURE.md`; compatibility remains open | PASS (research only) |
| ARCH-03 | Inspect [kernel.org releases](https://www.kernel.org/releases.json), [Debian releases](https://www.debian.org/releases/) and [Debian 13 notes](https://www.debian.org/releases/trixie/release-notes/whats-new.en.html) | As of 2026-10-31: Linux stable 7.3.1, Debian stable 13.7, official riscv64 support; no image is implied | PASS (metadata only) |
| DOC-01 | `python3 -m unittest discover -s tests -p 'test_*.py' -v` | Seven Python tests include phase/acceptance/evidence structure and architecture coverage | PASS |
| TS-ALL | `npm run format:check && npm run lint && npm run typecheck && npm test && npm run build` | Biome, strict `tsc`, 8 Vitest tests and Vite production build passed | PASS (TypeScript layer only) |
| RUST-ALL | `cargo fmt --all -- --check && cargo check --locked --offline && cargo test --locked --offline && cargo clippy --locked --offline --all-targets --all-features -- -D warnings` | Pinned Rust 1.99.0 workspace: 3 unit tests pass, format/check/clippy pass. Workspace clippy does not establish that vendored upstream code is warning-free under a full clippy audit | PASS (diagnostic workspace) |
| WASM-BUILD | `npm run build:wasm` with pinned Rust and Wasm target | Local diagnostic `cdylib` builds; the ABI is experimental and bit-for-bit reproducibility across clean hosts is not established | PASS (diagnostic only) |
| WASM-BROWSER | `npx playwright test tests/browser/wasm-abi.spec.ts` and real guest browser tests | Chromium instantiates the locally built Rust module, checks memory cap and ABI states, then executes actual guest instructions | PASS (diagnostic only) |
| WASM-BOUNDARY | `npm test` and `tests/browser/wasm-abi.spec.ts` / real trap tests | Checked exports, image ranges, serial and cycle bounds, zero output, status/trap propagation and 512 MiB memory cap verified | PASS (diagnostic only) |
| CAP-01 | `npm test` | Missing APIs and persistence grant/denial/error handling tested | PASS |
| CAP-BROWSER | `npx playwright test tests/browser/app.spec.ts -g 'unavailable APIs'` | Real browser displays unavailable APIs and denied persistence without claiming durable guest data | PASS |
| DIAG-BOOT | Full browser command above; `app.spec.ts` and `worker.spec.ts` | Real Linux 5.4 kernel log, ext2 root mount, interactive shell, `uname` and non-echoed marker observed; worker terminates on stop | PASS (obsolete volatile guest only) |
| DIAG-TRAP | Full browser command; `worker.spec.ts` and `app.spec.ts` | Invalid RV64 instruction produces a genuine Wasm trap at guest PC `0x10078`; cleanup preserves the error and the UI boots a fresh guest | PASS (diagnostic error handling) |
| DIAG-ISA | `cargo test --manifest-path vendor/riscv-rust/Cargo.toml --locked --offline` | 24 vendored emulator tests pass including five conversion tests; 11 pre-existing upstream unit stubs and one documentation example remain ignored | PASS (limited ISA coverage) |
| DIAG-UTILITIES | `npx playwright test tests/browser/guest-utilities.spec.ts` | Unchanged script performs real filesystem, `awk`, archive, process, mount, `sleep`, and `top` operations in the historical guest and returns `CHECK_PASS` | PASS (historical guest only) |
| DIAG-BACKPRESSURE | Full browser command; `worker.spec.ts` | Output stops with an acknowledgement withheld and resumes after release | PASS |
| DIAG-RESPONSIVE | Full browser command; `app.spec.ts` | A 50 ms main-thread timer completes in under 1 s while the worker is running on this test host; not a broad performance benchmark | PASS (diagnostic only) |
| FETCH-01 | `python3 -m unittest discover -s tests -p 'test_*.py' -v` | Four archive tests cover exact members, SHA/size, missing/duplicate/symlink and traversal; diagnostic assets were fetched and pinned | PASS (identity checks, not signatures) |
| STORAGE-UNIT | `npx playwright test tests/browser/storage.spec.ts` | Real Chromium tests sector bounds, cross-block writes, malformed keys, atomic corruption rollback and quota failure rollback | PASS (standalone store) |
| STORAGE-IDB | `npx playwright test tests/browser/storage.spec.ts` | Six tests include isolated profiles, concurrent tabs, v1-to-v2 schema upgrade and committed bytes/snapshots across a complete Chromium process restart | PASS (standalone store, not guest writes) |
| FB-PROTOTYPE | `npx playwright test tests/browser/framebuffer.spec.ts` | Exact RGBA pixels, resize, Canvas fallback and WebGPU device-loss replay passed; WebGPU used SwiftShader with explicit Chromium flags | PASS (synthetic framebuffer only) |
| UI-DISPLAY-RACE | `npx playwright test tests/browser/app.spec.ts -g 'stale display'` | A delayed failed attempt cannot dispose its newer successor after a simulated BFCache suspension | PASS |
| CI-01 | `.github/workflows/ci.yml` | Quality and Rust-target provenance jobs passed on the initial branch push; the initial browser job failed. The updated clean-checkout Rust and browser jobs require a successful new run before this item is complete | PARTIAL |
| DEP-AUDIT | `npm audit --audit-level=low` | Zero known npm advisories at this date; direct versions pinned. Binary guest provenance/license review remains incomplete | PARTIAL |
| KERNEL-SOURCE / KERNEL-BUILD / KERNEL-BOOT / BUSYBOX-ALL | Fetch/authenticate, build and boot the latest stable guest; exercise every requested applet | No authenticated current source build or current guest image | NOT RUN |
| DEBIAN-BOOT / DEBIAN-APT / DEBIAN-PERSIST | Boot Debian, install/remove real packages, restart and verify | No Debian image, bridge or persistent guest disk | NOT RUN |
| STORAGE-CRASH / STORAGE-QUOTA / STORAGE-RESTART | Power/interruption and quota tests on actual guest writes | Store-only quota and browser restart passed; no guest block adapter or guest flush proof | NOT RUN for guest |
| WEBGPU-REAL / GFX-FALLBACK | Render actual guest framebuffer in both paths | No virtual graphics device or guest pixels | NOT RUN |
| E2E-FULL / RELEASE-GATE | Both requested profiles, package operations, guest persistence and browser matrix | Required integrations and current guests are absent | NOT RUN |

## Temporary Rust toolchain verification

The official Rust 1.99.0 native archive was assembled from the `kelexine/rust-toolchain` GitHub mirror and checked **before execution** against Rust's independently published [`rust-1.99.0-x86_64-unknown-linux-gnu.tar.xz.sha256`](https://static.rust-lang.org/dist/rust-1.99.0-x86_64-unknown-linux-gnu.tar.xz.sha256): `891c6366d7100feda0bca4c03ce63f3c9ac827cbebbc283e7433061d42c6a376`.

The Wasm std files were extracted without running the installer from npm package `@toolchain-bin/rust-std-1.99.0-wasm32-unknown-unknown@1.99.0`. A [separate GitHub Actions job](https://github.com/havaianasdestruido/WASMNux/actions/runs/41243005366) downloaded Rust's official target archive, checked its independently published [SHA-256](https://static.rust-lang.org/dist/rust-std-1.99.0-wasm32-unknown-unknown.tar.xz.sha256), `b760b0c9b08a6843c05de0098bd6802e23178ff473c4715ae57c1d679125bf22`, and found the complete sorted filename/content hash manifest matched the npm-extracted files: manifest SHA-256 `be7a9eba42af157dfd4076057dee6bcbec908cdd0e08a7591ac09f9953595586` (54 files). Only after that job passed were those files used locally. The repository's normal setup uses rustup and does not depend on this npm mirror. Hash verification is not a signature audit or a substitute for a release supply-chain review.

## Boundaries and remaining failures

The independent store's quota test uses Chromium DevTools `Storage.overrideQuotaForOrigin` **before** opening the bucket, sets 256 KiB, attempts a random 2 MiB transactional write and verifies the preceding data/revision and an untouched block. This is not a power-failure test. The graphics test uses `--enable-unsafe-webgpu --use-angle=swiftshader --enable-unsafe-swiftshader`; neither physical-GPU compatibility nor guest rendering is established. The guest reports `can't run '/etc/init.d/rcS'`, an ext2 root without a journal, and a virtual disk size larger than the supplied image; these remain diagnostic limitations. Other unimplemented instructions and workloads may still trap.
