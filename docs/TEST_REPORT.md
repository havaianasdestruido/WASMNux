# Test evidence

Updated 2026-10-31. **Not release-ready. The complete browser suite is not green.** A pass applies only to the observation described, never to a broader unexecuted requirement.

## Environment and complete suite

Test host: Node `v22.22.3`, npm `10.9.8`, Python `3.11.2`, Chromium `153.0.8010.0` on Linux x86-64. The Chromium executable and NSS/NSPR libraries used in this sandbox are temporary; ordinary installations can use Playwright's installed browser instead. Vite served `0.0.0.0:4173`.

```sh
LD_LIBRARY_PATH=/tmp/al2023/lib PLAYWRIGHT_CHROMIUM_EXECUTABLE=/tmp/chromium npm run test:browser
```

Latest complete run after the runtime/storage/graphics changes: **21 tests, 20 passed, 1 failed** in about 2.2 minutes. The failure is `tests/browser/guest-utilities.spec.ts`: the historical guest traps during `awk`, before the remaining utility checks run. It is retained as an ordinary failing test, not skipped or counted as a pass. No requested production profile is booted by this suite.

| ID | Scope and exact command/procedure | Expected and actual | Status |
| --- | --- | --- | --- |
| AUDIT-01 | `git status --short --branch; git ls-files; ls -la` on the initial checkout | Only the Apache `LICENSE` was tracked; clean starting branch | PASS |
| ARCH-01 | Review upstream kernel architecture and emulator documentation | An upstream Linux kernel executes as a guest, not as a native WASI/Wasm program | PASS (design review) |
| ARCH-02 | Inspect pinned upstream sources, licenses, devices and activity | Candidate comparisons and limitations recorded in `ARCHITECTURE.md`; compatibility remains open | PASS (research only) |
| ARCH-03 | Inspect [kernel.org releases](https://www.kernel.org/releases.json), [Debian releases](https://www.debian.org/releases/) and [Debian 13 notes](https://www.debian.org/releases/trixie/release-notes/whats-new.en.html) | As of 2026-10-31: Linux stable 7.3.1, Debian stable 13.7, official riscv64 support; no image is implied | PASS (metadata only) |
| DOC-01 | `python3 -m unittest discover -s tests -p 'test_*.py' -v` | Phase/acceptance/evidence structure and architecture coverage checked by `test_docs.py` | PASS |
| TS-ALL | `npm run format:check && npm run lint && npm run typecheck && npm test && npm run build` | Biome, strict `tsc`, 5 Vitest tests and Vite production build passed; this does not include the failing browser utility test | PASS (TypeScript layer only) |
| CAP-01 | `npm test` | Missing APIs and persistence grant/denial/error handling tested | PASS |
| CAP-BROWSER | `npx playwright test tests/browser/app.spec.ts -g 'unavailable APIs'` | Real browser displays unavailable APIs and denied persistence without claiming durable guest data | PASS |
| DIAG-BOOT | Full browser command above; `app.spec.ts` and `worker.spec.ts` | Real Linux 5.4 kernel log, ext2 root mount, interactive shell, `uname` and non-echoed marker observed; worker terminates on stop | PASS (obsolete volatile guest only) |
| DIAG-TRAP | Full browser command; `worker.spec.ts` and `app.spec.ts` | Actual `awk` trap reports `Emulator trapped: unreachable`, cleanup does not mask it, UI allows a new diagnostic | PASS (error handling, not utility support) |
| DIAG-BACKPRESSURE | Full browser command; `worker.spec.ts` | Output stops with an acknowledgement withheld and resumes after release | PASS |
| FETCH-01 | `python3 -m unittest discover -s tests -p 'test_*.py' -v` | Four archive tests cover exact members, SHA/size, missing/duplicate/symlink and traversal; diagnostic assets were fetched and pinned | PASS (identity checks, not signatures) |
| STORAGE-UNIT | `npx playwright test tests/browser/storage.spec.ts` | Real Chromium tests sector bounds, cross-block writes, malformed keys, atomic corruption rollback and quota failure rollback | PASS (standalone store) |
| STORAGE-IDB | `npx playwright test tests/browser/storage.spec.ts` | Six tests include isolated profiles, concurrent tabs, v1-to-v2 schema upgrade and committed bytes/snapshots across a complete Chromium process restart | PASS (standalone store, not guest writes) |
| FB-PROTOTYPE | `npx playwright test tests/browser/framebuffer.spec.ts` | Exact RGBA pixels, resize, Canvas fallback and WebGPU device-loss replay passed; WebGPU used SwiftShader with explicit Chromium flags | PASS (synthetic framebuffer only) |
| UI-DISPLAY-RACE | `npx playwright test tests/browser/app.spec.ts -g 'stale display'` | A delayed failed attempt cannot dispose its newer successor after a simulated BFCache suspension | PASS |
| DEP-AUDIT | `npm audit --audit-level=low` and `npm ls --depth=0` | Zero known npm advisories at this date; direct versions pinned. Binary guest provenance/license review remains incomplete | PARTIAL |
| DIAG-UTILITIES | `npx playwright test tests/browser/guest-utilities.spec.ts` | Expected all scripted real BusyBox operations; guest traps at `awk` with `unreachable`, later operations not reached | FAIL |
| RUST-ALL | `cargo fmt --all -- --check; cargo check; cargo test; cargo clippy --all-targets --all-features -- -D warnings` | Rust/Cargo are not installed and no own Rust workspace/toolchain was established | NOT RUN (blocked) |
| WASM-BUILD / WASM-BROWSER | Build own runtime for `wasm32-unknown-unknown`, then instantiate in a browser | No trusted Rust compiler/target; only upstream prebuilt Wasm has been executed | NOT RUN (blocked) |
| KERNEL-SOURCE / KERNEL-BUILD / KERNEL-BOOT / BUSYBOX-ALL | Fetch/authenticate, build and boot the latest stable guest; exercise every requested applet | No authenticated current source build or current guest image | NOT RUN |
| DEBIAN-BOOT / DEBIAN-APT / DEBIAN-PERSIST | Boot Debian, install/remove real packages, restart and verify | No Debian image, bridge or persistent guest disk | NOT RUN |
| STORAGE-CRASH / STORAGE-QUOTA / STORAGE-RESTART | Power/interruption and quota tests on actual guest writes | Store-only quota and browser restart passed; no guest block adapter or guest flush proof | NOT RUN for guest |
| WEBGPU-REAL / GFX-FALLBACK | Render actual guest framebuffer in both paths | No virtual graphics device or guest pixels | NOT RUN |
| E2E-FULL / RELEASE-GATE | Both requested profiles, package operations, guest persistence and browser matrix | Required integrations absent and `DIAG-UTILITIES` fails | NOT RUN |

## Failure analysis and boundaries

With the cached translation optimization disabled, a minimal `awk 'BEGIN {print 42}'` still traps the historical emulator. A one-off browser trace of the emulator's public `read_pc` before each emulated cycle found guest PC `0xa611c`; disassembly of the corresponding BusyBox ELF bytes `d3 16 25 c2` identifies `fcvt.l.d`. The pinned emulator's `src/cpu.rs` instruction table does not implement `FCVT.L.D`. The guest test now exits promptly with the real error rather than waiting for a marker; it is still failing. Fixing this requires a verified rebuilt emulator or a compatible alternative and a rerun of the unchanged utility expectations.

The independent store's quota test uses Chromium DevTools `Storage.overrideQuotaForOrigin` **before** opening the bucket, sets 256 KiB, attempts a random 2 MiB transactional write and verifies the preceding data/revision and an untouched block. This is not a power-failure test. The graphics test uses `--enable-unsafe-webgpu --use-angle=swiftshader --enable-unsafe-swiftshader`; neither physical-GPU compatibility nor guest rendering is established. The guest reports `can't run '/etc/init.d/rcS'`, an ext2 root without a journal, and a virtual disk size larger than the supplied image; these remain diagnostic limitations.
