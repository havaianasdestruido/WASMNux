# WASMNux Security Architecture & Boundaries

## 1. Threat Model

WASMNux executes arbitrary, untrusted guest operating system kernels and userspace binaries (`riscv64` Linux `7.3.1` and Debian `13.7`) inside an untrusted browser environment. We treat:
1. **Guest code running inside the VM** as completely untrusted (a root process inside Linux must never escape WASM linear memory, read host files, or perform SSRF against localhost/private networks).
2. **Imported disk images and snapshots** as untrusted external binary inputs.
3. **Messages crossing the Main Thread <-> Web Worker boundary** as untrusted structured inputs requiring runtime schema validation.

---

## 2. Security Controls

### 2.1 Guest-to-Host Isolation (WASM & MMU Boundary)
- **Linear Memory Sandboxing:** All guest RAM, VRAM, CSRs, and VirtIO rings reside strictly inside the Web Worker's isolated WebAssembly linear memory. No host filesystem handles (`fs`, host paths) are ever exposed to the guest.
- **32-Bit WASM Pointer Bounds:** All physical address translations and JS<->WASM slice transfers validate `offset` and `len` with checked arithmetic (`usize::try_from` and `checked_add`) before indexing guest RAM or WASM linear memory.
- **Hardware-Equivalent Privilege Separation Inside Guest:** `Sv39` MMU enforces `PTE_U`, `SMEP`, and `sstatus.SUM` so unprivileged guest userspace processes cannot read or overwrite supervisor kernel pages via stale TLB entries.
- **Zero Dynamic Code Execution:** Neither the UI nor the Worker ever invokes `eval()`, `new Function()`, or dynamic script injection from guest console, logs, or filesystem strings. Terminal output is rendered strictly through `xterm.js` byte streams, and UI diagnostics use DOM `textContent` (`tests/unit/security-reliability.test.ts` scans the full `src/` tree on every test run).

### 2.2 Storage & Disk Import Hardening
- **Origin Sandboxing & Profile Isolation:** Guest disks live strictly inside browser OPFS (`/wasmnux/profiles/<sanitizedId>/`) and IndexedDB namespaces. Profile and snapshot identifiers are validated against `^[a-z0-9][a-z0-9_-]{0,62}$` — rejecting `..`, `/`, `\`, null bytes, and reserved names.
- **No Host Archive Extraction:** Virtual disk imports (`.wasmnux-disk`) use a flat, fixed-header chunked binary format (`WASMNUXD` magic, format version, profile kind, bounded virtual capacity <= 2 GiB, chunk count bounds check, and per-chunk IEEE 802.3 CRC32 verification). No `tar` or `zip` archive extraction with user-controlled filenames ever occurs in the browser application layer.
- **Crash-Consistent Commits:** Two-phase generation journaling prevents half-written chunks from corrupting committed virtual disks on tab kill or quota exhaustion.

### 2.3 Network Bridge Restrictions
- **Virtual L2/L3 Isolation:** Guest `eth0` (`10.0.2.15/24`) terminates inside `crates/wasmnux-net` in Rust WASM; raw guest Ethernet/IP packets never leave the Worker.
- **SSRF & Host Protection in HTTP Bridge:** `src/net/network-bridge.ts` intercepts `http://apt.wasmnux.local/*` to serve only static same-origin `/debian-repo/*` assets, and for optional external HTTP(S) fetches strictly blocks `localhost`, `127.0.0.0/8`, `0.0.0.0`, `169.254.0.0/16`, `10.0.0.0/8` (except internal virtual `apt.wasmnux.local`), `172.16.0.0/12`, `192.168.0.0/16`, `[::1]`, `file:`, `javascript:`, and `data:` schemes.

### 2.4 Browser Sandbox Limitations
- Browser Same-Origin Policy (SOP) and CORS apply to any external network request not served same-origin.
- Browser storage durability is subject to user site-data clearing and browser quota eviction unless `navigator.storage.persist()` is granted.
- `SharedArrayBuffer` requires Cross-Origin Isolation headers (`Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`). WASMNux operates cleanly in a dedicated Web Worker using transferable/structured messages without requiring `SharedArrayBuffer`, while detecting and reporting cross-origin isolation when available.

---

## 3. External Dependency & Vulnerability Audit (`npm audit --json`)

### 3.1 Production vs. Development Separation
- **Rust / WASM Runtime (`Cargo.toml`)**: **0 external crates** (`crates.io` dependencies = `0`).
- **Browser Production Runtime (`package.json` `dependencies` — `npm audit --omit=dev --json`)**: Only 2 direct packages (`@xterm/xterm@^6.0.0` and `@xterm/addon-fit@^0.11.0`, `prod: 3` total nodes). **0 vulnerabilities** (`info: 0, low: 0, moderate: 0, high: 0, critical: 0`).
- **Development & CI Tooling (`devDependencies` — `npm audit --json`)**: `203` dev nodes (`total: 205`). **0 vulnerabilities** (`info: 0, low: 0, moderate: 0, high: 0, critical: 0`), enforced automatically in `.github/workflows/ci.yml` via `npm audit --audit-level=low`.

### 3.2 Remediation Record of Prior Dev-Only Advisories
During validation prior to release, `npm audit --json` against the initial lockfile reported 14 advisories (`0` production, `14` development-only across two transitive chains):
1. **Vitest v3 test-runner chain (`vitest@3.2.7` -> `tinypool <= 2.1.1` & `@vitest/mocker < 4.1.12`)**:
   - Advisories: `GHSA-5gmw-xhrv-c9v3` & `GHSA-85c8-ppgw-ccpr` (`tinypool` prototype pollution in worker options, `critical`), `GHSA-82fw-gwwq-j7x9` & `GHSA-qxg6-89mg-rqv8` (`@vitest/mocker`, `moderate`).
   - Fix applied: Upgraded `vitest` cleanly to `^5.0.5` (compatible with `vite@^6.4.5` peer range `^6.4.0 || ^7.0.0 || ^8.0.0`), removing `tinypool` and upgrading `@vitest/mocker` to `5.0.5`.
2. **ESLint / TypeScript-ESLint globbing chain (`minimatch < 11.0.1`)**:
   - Advisory: `GHSA-g3hv-75wg-m249` (`minimatch` `globparts` amplification DoS, `moderate`, propagating to 11 transitive `@eslint/*` and `@typescript-eslint/*` nodes because upstream `eslint@10.14.0` still declares `minimatch@^10.2.6` and `npm audit` suggested a broken major downgrade to `eslint@4.0.0`).
   - Fix applied: Upgraded `eslint` (`^10.14.0`), `@eslint/js` (`^10.0.1`), and `typescript-eslint` (`^8.72.1`), and pinned `"overrides": { "minimatch": "^11.0.3" }` (verified dual CJS/ESM export compatibility across the full `npm run lint` suite without using `npm audit fix --force`).

### 3.3 Current Dependency Inventory

| Component | Version | License | Scope | Security Notes |
|---|---|---|---|---|
| Rust Core (`wasmnux-*`) | `0.1.0` | Apache-2.0 | Production WASM | **Zero external crates**; all FFI & MMU bounds checked in-tree. |
| `@xterm/xterm` | `^6.0.0` | MIT | Production UI | Terminal widget; receives raw guest UART bytes only. |
| `@xterm/addon-fit` | `^0.11.0` | MIT | Production UI | Viewport column/row calculation addon for xterm.js. |
| `typescript` | `~5.8.3` | Apache-2.0 | Dev/Build | Strict static type checking (`noImplicitAny`, `noUncheckedIndexedAccess`). |
| `vite` | `^6.4.5` | MIT | Dev/Build | ES module bundler & dev/preview server. |
| `vitest` | `^5.0.5` | MIT | Dev/Test | Unit, integration, security, and benchmark runner (`0` advisories). |
| `eslint` / `@eslint/js` / `typescript-eslint` | `^10.14.0` / `^10.0.1` / `^8.72.1` | MIT | Dev/Lint | `strictTypeChecked` static analysis (`minimatch@^11.0.3` override). |
| `@playwright/test` + `@sparticuz/chromium` | `^1.64.1` / `^153.0.0` | Apache-2.0 | Dev/E2E Test | Headless Chromium 153 E2E verification with SwiftShader WebGPU. |
| `prettier` / `fake-indexeddb` / `@webgpu/types` | `^3.5.3` / `^6.2.7` / `^0.1.74` | MIT / Apache-2.0 / BSD-3-Clause | Dev/Lint & Types | Formatting, IndexedDB Node polyfill for unit tests, W3C WebGPU types. |
