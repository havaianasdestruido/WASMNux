# WASMNux test report

Date: 2026-10-31. Environment: Linux sandbox, AOSP Rust 1.83.0-dev (`90b35a623`), LLVM/LLD 20.0.0, Node 22, Chromium via `@sparticuz/chromium` 153.0.0 with Amazon Linux 2023 runtime libraries. Playwright browser download was blocked; the pinned npm Chromium bundle was used instead. Kernel.org, deb.debian.org, static.rust-lang.org and busybox.net were unreachable; GitHub mirrors and npm registry were used. **This report does not claim the project is finished.**

## Rust gates

Commands (with `PATH=/home/user/toolchains/bin:/home/user/toolchains/aosp-rust/linux-x86/1.83.0/bin:$PATH CARGO_HOME=/home/user/toolchains/cargo-home RUSTC_BOOTSTRAP=1`):

| Command | Expected | Actual | Status |
|---|---|---|---|
| `cargo fmt --all -- --check` | no diff | RC=0 | PASS |
| `cargo check` | compile | RC=0 | PASS |
| `cargo test` | all unit/integration tests | riscv-core 20/0, native 2/0 | PASS |
| `cargo clippy --all-targets --all-features -- -D warnings` | no warnings | RC=0 | PASS |
| `cargo build -p wasmnux-runtime --target wasm32-unknown-unknown -Z build-std=std,panic_abort --release` | wasm module | built; exports/imports verified in browser | PASS |

Coverage includes snapshot CPU/RAM round-trip, VirtIO block bounds/error completion, journal-independent block serialization, lifecycle allocation, and malformed snapshot rejection. Guest DMA/ABI fuzzing and repeated long-run memory measurement remain open.

## TypeScript/web gates

| Command | Expected | Actual | Status |
|---|---|---|---|
| `npx tsc --noEmit` | strict clean | RC=0 | PASS |
| `npx eslint .` | typed lint clean | RC=0 | PASS |
| `npx prettier --check web/src web/tests *.ts *.json` | formatted | RC=0 | PASS |
| `npx vitest run` | unit tests | 7 passed / 4 files (journal 4, protocol 1, storage-admin 1, metadata 1) | PASS |
| `npm run build` | production bundle | ~299.8 kB JS, gzip ~72.4 kB; asset manifest rehashed | PASS |
| `npm audit --omit=dev` | no production advisories | 0 vulnerabilities | PASS |
| `npm audit` | full tree | 11 moderate (eslint→minimatch DoS); no non-breaking fix available | KNOWN ISSUE |

## Native guest evidence

- Vanilla: `crates/native` with `--seconds 26 --input 'cat /root/persist.txt'` printed `hello` after a prior guest write+sync; retired 2,682,701,900 instructions. Log: `tool-downloads/native-*.log` (workspace-local, not committed).
- Debian 256 MiB ext2: `--no-initrd --cmdline 'console=ttyS0 earlycon=sbi root=/dev/vda rw rootwait init=/wnx-init.sh'` reached `WASMNUX_DEBIAN_READY`; earlier 512 MiB ext4 run installed the bundled demo `.deb` (`APT_INSTALL_RC=0`) and after restart `dpkg-query` reported `wasmnux-demo 1.0`. Logs: `native-debian-256.log`, `native-debian-ext2.log`.
- Framebuffer: kernel registered `fb0`, reserved `0x8f000000..0x8f0f9fff`, `a8r8g8b8 640x400x32 stride 2560`; retired 2,826,093,705.

## Browser E2E (real guest, no mocks)

Command: `LD_LIBRARY_PATH=/tmp/al2023/lib CHROMIUM_PATH=/tmp/chromium npx playwright test <spec>` against `vite preview` on port 4173.

| Test | Expected | Actual | Status |
|---|---|---|---|
| `boot.spec.ts` | Linux boot, guest file write, shutdown, reload, content verification | PASS 49.1 s; `browser-ok` survived reload; no page errors | PASS |
| `debian.spec.ts` | Debian boot, local apt install, dpkg query, shutdown, reload, package executable + database persistence, apt remove | PASS 1.5 m; `APT_RC=0`, `wasmnux-demo 1.0`, `WASMNUX_DEMO_PACKAGE_OK`, `REMOVE_RC=0`, `no packages found matching wasmnux-demo`; no page errors | PASS |
| `graphics.spec.ts` | guest framebuffer presented, non-empty after guest `dd` to `/dev/fb0` | PASS 26.0 s; renderer reported WebGPU or Canvas 2D and `data-frame-checksum` non-zero | PASS (does not yet prove WebGPU specifically, forced fallback, or device-loss recovery) |
| `snapshot.spec.ts` | create/restore/delete snapshot from UI | FAIL: worker became `unreachable` after Snapshot; likely 256 MiB snapshot allocation pressure. Not fixed. | FAIL / OPEN |
| `offline.spec.ts` | cached app boots offline after first online boot | FAIL: offline reload reached Ready but guest boot produced no terminal output; service-worker/worker asset caching incomplete. Not fixed. | FAIL / OPEN |

Debian E2E requires `launchPersistentContext`: ephemeral Chromium contexts reported ~1 GiB quota, denied `persist()`, and failed OPFS initialization around 143 MiB; persistent contexts reported ~10 GiB and succeeded. All OPFS writes are chunked to 64 KiB after large `SyncAccessHandle.write` calls silently short-wrote and `createWritable` positioned writes corrupted images.

## Requirement traceability summary

- Real kernel/BusyBox/Debian execution: verified natively and in Chromium (above). No remote shell or mocked commands.
- Persistence: vanilla file and Debian package database/files verified across reload in persistent contexts. Fresh-process restart, quota exhaustion, interrupted-transaction recovery, and cross-profile reset isolation are not yet fully verified.
- Graphics: real guest scanout pixels presented; WebGPU-specific success/fallback/device-loss matrix open. Guest keyboard/pointer VirtIO input devices are absent; serial keyboard only.
- Networking: no relay deployed; guest VirtIO-net unimplemented. Online apt from Debian repositories is blocked, not faked. Local `.deb` install/remove/query is real.
- Snapshots: Rust CPU/RAM snapshot test passes; browser snapshot/restore currently fails and is not claimed complete.
- Offline PWA: currently fails and is not claimed complete.
- Benchmarks: instruction counts and E2E wall times above are measured; boot-time distributions, memory soak, block throughput and terminal latency benchmarks remain open.
- Provenance: `docs/PROVENANCE.md`, `vendor/PATCHES.md`, `web/public/assets/manifest.json`. Signed-tag verification and clean-room double-build hash equality remain open.

## Known bugs and limitations

1. Browser snapshot creation can kill the worker (suspected memory pressure from full 256 MiB RAM snapshot plus existing WASM heap).
2. Offline PWA boot fails; service worker does not yet reliably cache worker/runtime/guest assets.
3. No guest networking, VirtIO input, clipboard, or guest tty resize signal.
4. `Machine::load_state` mutates CPU/bus before RAM validation; no rollback on corrupt snapshot.
5. 11 moderate dev-only npm advisories remain.
6. ext2 Debian disk retained after OPFS write fix; ext4 journal path not retested.
7. GitHub Actions CI (rust + web jobs) passed on PR #5; real guest/browser E2E is still run locally, not on GitHub Actions.
