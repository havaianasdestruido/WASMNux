# Third-party provenance and notices

This repository's own work is under the existing [Apache-2.0 LICENSE](LICENSE). This inventory is incomplete for redistribution of the historical binary guest; it is not a legal clearance or a reproducible-build attestation.

## Historical diagnostic (not committed)

`scripts/fetch_probe.py` retrieves a GitHub source archive at commit [`8ee69d7a5dc7ef6d8b2bda96bf86d2923f2cf176`](https://github.com/takahirox/riscv-rust/tree/8ee69d7a5dc7ef6d8b2bda96bf86d2923f2cf176) of `takahirox/riscv-rust`. It extracts only the four explicitly named regular files after size and SHA-256 validation:

| Installed file | Bytes | Pinned SHA-256 |
| --- | ---: | --- |
| `riscv_emu_rust_wasm.js` | 15,694 | `b51a13f2be49a8286e96e5c6d51dd677950fbdf4675b7fa80ff356da76878d05` |
| `riscv_emu_rust_wasm_bg.wasm` | 158,147 | `e1804062b8a40bfe27af9a393bee291b0e8228435ebbc47a19d45a1faa8e3f65` |
| `fw_payload.elf` | 9,907,888 | `3ce5529fe302144732b22636d614965bea3ac09999cfdb42912e74c33c6396e6` |
| `rootfs.img` | 52,428,800 | `273067980c4d8ad3898d505c1786ef69ecf6b33b8373942089a8b0ffe84b3409` |

At runtime the worker rechecks the Wasm, firmware and rootfs sizes/hashes; the JavaScript loader is checked by the setup fetcher but is not independently rehashed on each import. The hashes pin the retrieved bytes; they do **not** establish signed upstream provenance, a safe artifact, or correspondence to a reproducible source build. The upstream emulator is MIT-licensed, and its Rust dependency manifest names `fnv` and `wasm-bindgen`; the precise dependencies and licenses in the prebuilt Wasm have not been independently reconstructed.

The upstream [guest build instructions](https://github.com/takahirox/riscv-rust/blob/8ee69d7a5dc7ef6d8b2bda96bf86d2923f2cf176/resources/linux/opensbi/Readme.md) describe Linux 5.4, OpenSBI 0.8 and static BusyBox 1.32.0. Linux and BusyBox carry GPL-family obligations; OpenSBI is BSD-2-Clause with additional notices; a statically linked C library may carry further obligations. Exact binary-to-source correspondence, all build inputs/configurations, notices and corresponding-source/relinking obligations have **not** been established. Do not publish these binary images as a release until this review is completed. `npm run build` copies `public/guest` if the files are present.

### `riscv-rust` MIT notice

```text
MIT License

Copyright (c) 2020 Takahiro

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## JavaScript toolchain

Direct packages and versions are pinned in `package.json` and the transitive graph/integrities in `package-lock.json`. The direct dependencies are xterm.js and its fit addon (MIT); development tools include TypeScript and Playwright (Apache-2.0), Biome (MIT OR Apache-2.0), Vite and Vitest (MIT), Node types (MIT), and WebGPU types (BSD-3-Clause). The installed dependency graph also contains BSD-3-Clause and ISC packages; inspect each package's own `LICENSE` and `npm query '*'` before redistribution. `npm audit` on 2026-10-31 reported zero known advisories for the resolved graph; this is a point-in-time automated check, not a security audit.

The independently researched `rv64.js` candidate is MIT-licensed, but is not incorporated as the production VM. Any future use requires pinned source, dependency inventory, license notices and actual compatibility/security testing.
