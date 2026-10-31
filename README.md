# WASMNux

**Early prototype — the requested Linux and Debian environments are not implemented.** The only bootable guest is a real but obsolete, offline Linux 5.4 / BusyBox diagnostic in a Rust/Wasm RISC-V emulator. Its disk is **volatile**: stopping, refreshing, or closing the page discards every guest change. Some commands, including `awk`, trap the emulator because its instruction support is incomplete. This diagnostic is not a substitute for either requested profile.

The independently tested IndexedDB block store is **not connected to the guest**. The WebGPU/Canvas display check renders a **synthetic** pattern, not a guest framebuffer. There is no Debian image, `apt` bridge, guest persistence, disk import/export, or offline PWA yet. See [TODO.MD](TODO.MD) and [the test report](docs/TEST_REPORT.md) for exact status, including a deliberately retained failing guest-utility test.

## Run the diagnostic

Requirements: Node.js 22+, npm, Python 3, a modern browser, and access to GitHub/npm. The diagnostic images total about 60 MiB and are downloaded separately; they are not committed.

```sh
npm ci
python3 scripts/fetch_probe.py
npm run dev -- --port 4173
```

Open the URL printed by Vite. Select **Boot diagnostic**, wait for `Please press Enter to activate this console`, click the terminal and press Enter. For example, `uname -s` executes in the actual guest. **Stop** immediately terminates the worker and discards the disk; it is not a graceful poweroff. Use a secure context (localhost or HTTPS) for Web Crypto and browser storage APIs.

The pinned fetcher verifies the exact size and SHA-256 of the four historical files before installation. These pins identify what was fetched, but are not signed release verification or proof of reproducible builds. The development and production Vite build serve/copy locally fetched guest binaries; they must not be redistributed as a release until the source and licensing questions in [third-party notices](THIRD_PARTY_NOTICES.md) are resolved. Do not expose the Vite development server to an untrusted network.

## Tests

In one terminal, start the development server on port 4173 as above. In another:

```sh
npm run format:check
npm run lint
npm run typecheck
npm test
python3 -m unittest discover -s tests -p 'test_*.py' -v
npm run build
npm audit
npm run test:browser
```

Install Playwright's Chromium separately (`npx playwright install chromium`) if needed. `PLAYWRIGHT_CHROMIUM_EXECUTABLE` can point at an existing Chromium executable. The full browser suite currently **fails** on the historical guest's `awk` trap; it is not skipped or treated as a pass. Other browser tests exercise actual worker boot, standalone IndexedDB and WebGPU/Canvas rendering. The WebGPU test explicitly requests Chromium's software adapter; it is not a hardware GPU compatibility claim. Rust/Cargo checks and building an owned Wasm runtime remain blocked in this sandbox.

## Design and status

- [Architecture, boundaries and security](docs/ARCHITECTURE.md)
- [Phase-by-phase acceptance plan](TODO.MD)
- [Commands, results and outstanding tests](docs/TEST_REPORT.md)
- [Sources, licenses and provenance](THIRD_PARTY_NOTICES.md)
