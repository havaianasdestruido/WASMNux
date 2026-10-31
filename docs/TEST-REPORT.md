# Test report — WASMNux

Living evidence log (Mission §8). A requirement is only ever reported here
the way it actually ran: **PASS** (executed, observed), **FAIL** (executed,
observed failing, with the fix recorded), or **NOT EXECUTED** (with the
reason and what substitute validation exists). Nothing is extrapolated.

## Environments

- **Dev sandbox (local):** Debian 12 container, Node 22.22, gcc 12, Python
  3.11, BusyBox 1.36.1 (x86_64), **no Rust toolchain**, network limited to
  github.com / npm / PyPI. Consequences are called out per row.
- **GitHub Actions:** `ubuntu-24.04`, Rust stable (+ `wasm32-unknown-unknown`),
  Node 22. CI failure output is mirrored to PR #1 comments ("CI diagnostics")
  because the sandbox cannot reach Actions log/artifact hosts; run ids below
  let any reader with normal access inspect the full logs.

## Executed — results

| # | Requirement / area | Exact command / procedure | Expected | Actual | Status |
|---|---|---|---|---|---|
| T-01 | TypeScript strict typecheck | `npm --prefix web run typecheck` (local, and CI web job) | no errors | clean (after fixing 12 initial errors: ImageData typing, OPFS `values()`, protocol widening, node types) | **PASS** |
| T-02 | TypeScript lint (esp. no-`any`, no non-null assertions) | `npm --prefix web run lint` (local + CI) | 0 warnings | clean | **PASS** |
| T-03 | TS unit tests: CRC-32 vectors incl. Rust parity, capabilities, ChunkStore key safety, **SparseDisk** (chunk-boundary IO, holes, unflushed-write crash consistency, CRC corruption detection, CoW snapshots, profile isolation, export/import dedup, deallocation, destroy), profile + manifest validation, worker-protocol validation, pixel swizzle | `npm --prefix web test` (vitest 2.1, local; CI web job repeats) | all pass | **27 passed / 0 failed** (5 files) | **PASS** |
| T-04 | Web production build | `npm --prefix web run build` (local + CI) | succeeds | built in ~1.4 s; bundle ~330 KiB | **PASS** |
| T-05 | Rust formatting gate | CI `cargo fmt --all -- --check` | clean | initially FAIL (hand-written without rustfmt locally) → applied CI-generated patch via PR diagnostics → **success** from run 41233191488 on | **PASS** (after fix) |
| T-06 | Rust clippy with `-D warnings` | CI `cargo clippy --all-targets --all-features -- -D warnings` | clean | 10 real lints found (dead consts, `map_or`, manual checked div, `matches!`, MSRV, Range::contains) + 5 test-only lints → all fixed in code, no `#[allow]` shortcuts → **clean** (evidence: run 41233191488 moved past clippy to the wasm-bindgen step) | **PASS** (after fixes) |
| T-07 | Rust unit tests: RAM bounds, CRC, UART regs/DLAB/IRQ, PLIC claim-priority, CLINT timer, VirtIO queue read/write/flush + malformed chains, ELF validation/loading, DTB structure walk + reservations, CPU (arith/M, div-by-zero spec results, load/store sign extension, branches, CSR/misa honesty, illegal-instruction trap + mcause, SBI ecall interception, **Sv39 translation/protection + Sv48 WARL rejection**), SBI shim, machine lifecycle + **end-to-end tiny-guest boot** (S-mode program prints via SBI and powers off through the real boot path) | CI `cargo test --workspace --all-features` | all pass | **40 passed / 0 failed** (run 41233191488 diagnostics). One earlier failure was a *test harness* bug (avail.idx not advanced monotonically) — fixed in the harness, not the device. | **PASS** |
| T-08 | WASM compilation of the runtime | CI `cargo build -p wasmnux-wasm --target wasm32-unknown-unknown --release` | succeeds | `Finished release profile` (run 41233191488) | **PASS** |
| T-09 | wasm-bindgen JS package generation | CI step after T-08 | package produced | Initially **FAIL** (CLI 0.2.100 vs library 0.2.130); both sides pinned `=0.2.130` → package produced and consumed by the e2e job. | **PASS (after fix)** |
| T-10 | BusyBox behaviour harness logic | `sh guest/tests/busybox-applets.sh /usr/bin/busybox` locally (host x86_64 BusyBox 1.36.1) | all non-root checks pass | **40 PASS / 0 FAIL / 3 SKIP** (mount/dmesg/reboot skip without root). Two harness expectation bugs (sed first-`e`, `top` needing `-b`) were found and fixed. This validates the harness and expected semantics only — **not** the riscv64 guest binary. | **PASS (harness only)** |
| T-11 | Guest BusyBox cross-build (riscv64) | Guest-images CI `build-busybox.sh` | static riscv64 busybox, all applets present | **FAIL (found real issue):** Ubuntu glibc lacks `gnu/stubs-lp64.h` — soft-float (`lp64`) static build impossible against lp64d glibc. Adopted fix: pinned musl-cross-make soft-float toolchain (rv64imac/lp64), built+ cached in CI. First toolchain build failed during the GCC stage (diagnostics initially truncated the log before the error line; error-line extraction added). Rebuild in progress at report time. | **FAIL → fix in flight (toolchain)** |
| T-12 | Shell-script syntax | `bash -n` / `sh -n` over `guest/scripts`, `guest/tests`, `scripts` | clean | clean | **PASS** |
| T-13 | Workflow YAML validity | `python -c yaml.safe_load` for both workflows | parse | ok | **PASS** |
| T-15 | **Browser end-to-end suite** (Playwright, real Chromium + real CI-built WASM core): (a) capabilities render + Rust core instantiates (`WASM core: ready`, storeKind=`opfs`); (b) boot stays honestly disabled with enumerated blockers when guest assets are absent; (c) disk marker written through SparseDisk→OPFS **survives a full page reload**; (d) snapshot restore reverts later writes, profiles stay isolated, reset of one profile preserves the other — across reload; (e) renderer self-test changes the framebuffer canvas (screenshot diff) on the Canvas2D fallback in adapter-less headless | CI e2e job `npx playwright test` (Chromium headless shell, SwiftShader flags) | all pass | **5 passed / 1 skipped** (the gated full-boot test skips loudly without guest assets) — job green from run 41236543791's CI sibling onward | **PASS** |
| T-14 | Feasibility & version ground truth | Live queries: kernel.org `releases.json` (kernel **7.3.1** stable, 2026-10-30), debian.org releases (Debian **13 trixie**, pt. 13.7, first official riscv64), GitHub API for v86/RVVM/qemu-wasm/rvemu/riscv-rust (licenses + activity) | evidence-based ADR | recorded in `docs/ARCHITECTURE.md` | **PASS** |

## NOT EXECUTED — explicitly, with reasons and substitutes

| # | Test | Why not | Substitute validation performed | What remains |
|---|---|---|---|---|
| N-01 | Local `cargo *` | No Rust toolchain in the sandbox; rustup/apt/deb hosts are proxy-blocked (verified by curl/apt attempts). | Full Rust matrix on GitHub Actions (T-05…T-09) with logs mirrored to PR #1. | Nothing for correctness; local turnaround is slower by design. |
| N-02 | Playwright locally | Browser binaries download from a blocked CDN (`install chromium` → download failure, observed). | Suite executes in the CI e2e job — see T-15 (PASS). Local non-execution is environmental only. | Nothing for correctness; local browser iteration unavailable. |
| N-03 | Booting the real Linux kernel (any of: native harness or browser) | Needs a cross-built kernel Image (kernel.org unreachable locally; the guest CI kernel job is manual-dispatch) **and** only proves anything once it reaches userspace. | Boot *path* validated by the tiny-guest S-mode integration test (T-07) + DTB/boot-protocol unit tests; kernel config + build script enforce required options. | Kernel-to-shell boot = Phase 5, unchecked in TODO.MD. |
| N-04 | Debian anything (boot, dpkg, apt, persistence) | As N-03, plus the hard blocker: Debian `lp64d` needs CPU F/D extensions, not yet implemented (`misa` honestly excludes them). | Real-image build script (debootstrap from deb.debian.org) exists; smoke script written for in-guest execution. | Phase 2 F/D, then Phase 6. All unchecked. |
| N-05 | WebGPU on a real GPU / device-loss | No browser at all locally (N-02). | Renderer code paths typecheck; pixel-conversion pure functions unit-tested; Playwright uses SwiftShader WebGPU in CI. | Real-hardware validation on user machines. |
| N-06 | Storage across a real *browser restart* | N-02 locally. | Crash-consistency model proven at the ChunkStore layer by the unflushed-write test (T-03); Playwright reload test covers page reload in CI. | Full browser-process restart evidence. |
| N-07 | riscv-tests compliance suites, performance benchmarks | Toolchain/time — not yet pulled in. | Hand-built ISA tests per opcode group (T-07). | Phase 2/11 items, unchecked. |
| N-08 | Guest kernel/OpenSBI/Debian builds | Manual-dispatch CI jobs (tens of minutes); not dispatched yet (also: `workflow_dispatch` requires the workflow on the default branch — post-merge). | Scripts are syntax-checked, version-resolved, checksum-enforcing; the fast BusyBox job runs on every trigger-file push. | First dispatched full builds. |

## Failure history worth remembering (from CI diagnostics)

- rustfmt drift (no local rustfmt) → patches generated in CI, applied verbatim.
- 10 clippy findings → all fixed in code (one MSRV bump to 1.87, documented).
- VirtIO test harness wrote `avail.idx` absolutely; real guests advance it
  monotonically — harness fixed (device was correct).
- Two CPU *test* encodings built `0x80000000` with a bare `lui` (sign
  extension makes that wrong) — caught by review, fixed with `1<<31`.
- glibc soft-float stubs (T-11) → musl toolchain.
- Vite dev rewrote the worker's `/wasm/` dynamic import into its module
  pipeline (`?import`), which refuses /public files → worker URL now built
  from the runtime origin. The e2e diagnostics (page snapshots) pinpointed it.
- Playwright `page.evaluate` serialises returned objects (functions are
  dropped) → e2e helpers call app test-hooks only *inside* evaluate; async
  UI handlers (snapshot restore/reset) are asserted with `expect.poll`
  instead of racing `click()`.
- Dev-server SPA fallback returns `index.html` with HTTP 200 for missing
  guest manifests → asset probe checks `content-type` and reports
  "missing assets" honestly (regression test added).
- A `git stash` mishap mid-session dropped uncommitted edits; recovered via
  the recorded stash object hash and re-verified. Process fix: commit
  early, never stash across CI iterations.
