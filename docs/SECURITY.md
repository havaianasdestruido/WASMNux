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
- **Zero Dynamic Code Execution:** Neither the UI nor the Worker ever invokes `eval()`, `new Function()`, or dynamic script injection from guest console, logs, or filesystem strings. Terminal output is rendered strictly through `xterm.js` byte streams, and UI diagnostics use DOM `textContent`.

### 2.2 Storage & Disk Import Hardening
- **Origin Sandboxing & Profile Isolation:** Guest disks live strictly inside browser OPFS (`/wasmnux/profiles/<sanitizedId>/`) and IndexedDB namespaces. Profile and snapshot identifiers are validated against `^[a-z0-9][a-z0-9_-]{0,62}$` — rejecting `..`, `/`, `\`, null bytes, and reserved names.
- **No Host Archive Extraction:** Virtual disk imports (`.wasmnux-disk`) use a flat, fixed-header chunked binary format (`WASMNUXD` magic, format version, profile kind, bounded virtual capacity <= 2 GiB, chunk count bounds check, per-chunk CRC32 checksum, and full-payload SHA-256 digest). No `tar` or `zip` archive extraction with user-controlled filenames ever occurs in the browser application layer.
- **Crash-Consistent Commits:** Two-phase generation journaling prevents half-written chunks from corrupting committed virtual disks on tab kill or quota exhaustion.

### 2.3 Network Bridge Restrictions
- **Virtual L2/L3 Isolation:** Guest `eth0` (`10.0.2.15/24`) terminates inside `crates/wasmnux-net` in Rust WASM; raw guest Ethernet/IP packets never leave the Worker.
- **SSRF & Host Protection in HTTP Bridge:** `src/net/network-bridge.ts` intercepts `http://apt.wasmnux.local/*` to serve only static same-origin `/debian-repo/*` assets, and for optional external HTTP(S) fetches strictly blocks `localhost`, `127.0.0.0/8`, `0.0.0.0`, `169.254.0.0/16`, `10.0.0.0/8` (except internal virtual `apt.wasmnux.local`), `172.16.0.0/12`, `192.168.0.0/16`, `[::1]`, `file:`, `javascript:`, and `data:` schemes.

### 2.4 Browser Sandbox Limitations
- Browser Same-Origin Policy (SOP) and CORS apply to any external network request not served same-origin.
- Browser storage durability is subject to user site-data clearing and browser quota eviction unless `navigator.storage.persist()` is granted.
- `SharedArrayBuffer` requires Cross-Origin Isolation headers (`Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`). WASMNux operates cleanly in a dedicated Web Worker using transferable/structured messages without requiring `SharedArrayBuffer`, while detecting and reporting cross-origin isolation when available.

---

## 3. External Dependency Audit

| Component | Version | License | Scope | Security Notes |
|---|---|---|---|---|
| Rust Core (`wasmnux-*`) | `0.1.0` | Apache-2.0 | Production WASM | **Zero external crates (`crates.io` dependencies = 0)**; eliminates supply-chain risk in WASM runtime (`emuko`-derived RISC-V modules audited in-tree). |
| `@xterm/xterm` | `^6.0.0` | MIT | Browser UI | Maintained terminal emulator widget; receives raw UART bytes only. |
| `@xterm/addon-fit` | `^0.11.0` | MIT | Browser UI | Viewport cols/rows calculation addon for xterm.js. |
| `typescript` | `^5.8.0` | Apache-2.0 | Dev/Build | Strict static type checking. |
| `vite` | `^6.0.0` | MIT | Dev/Build | Bundler & dev server. |
| `vitest` | `^3.0.0` | MIT | Dev/Test | Unit & WASM integration test runner. |
| `@playwright/test` + `@sparticuz/chromium` | `^1.50+` / `153.0.0` | Apache-2.0 | Dev/E2E Test | Real headless Chromium browser verification. |
| `eslint` + `prettier` | Latest stable | MIT | Dev/Lint | Static analysis and code formatting. |
