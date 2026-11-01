# Deploying WASMNux to GitHub Pages

WASMNux is a **completely static** application. The RISC-V emulator, the Linux `7.3.1` kernel,
the BusyBox/Debian root filesystems, and the local `trixie` APT repository are immutable files in
`public/`, and every guest disk write is persisted in the browser's OPFS/IndexedDB. GitHub Pages
therefore hosts the full virtual machine with no server, no build-time secrets, and no runtime API.

Published site: `https://<owner>.github.io/WASMNux/` (a project sub-path).

---

## 1. One-time repository setup

1. Open **Settings -> Pages**.
2. Under **Build and deployment -> Source**, choose **GitHub Actions**.
3. Re-run the **Deploy to GitHub Pages** workflow (Actions -> Deploy to GitHub Pages -> Run workflow),
   or push any commit to `main`.

This step is intentionally manual: creating a Pages site through the REST API requires repository
admin rights that the workflow's `GITHUB_TOKEN` does not have, so `actions/configure-pages` cannot
enable Pages for you (it fails with `HttpError: Resource not accessible by integration`).
3. (Optional) Add a custom domain; the app works from any path prefix or hostname without config
   changes because all asset URLs are resolved relatively at runtime.

> **Plan requirement:** GitHub Pages is available for public repositories on GitHub Free. Private
> repositories require a plan that includes Pages (Pro/Team/Enterprise). The repository itself does
> not need to be public for the workflow file to be valid — only Pages availability matters.

No repository secrets or tokens are required: the workflow uses the scoped `GITHUB_TOKEN` with
`pages: write` + `id-token: write`.

---

## 2. Deployment workflow

[`.github/workflows/pages.yml`](../.github/workflows/pages.yml) runs on:

- every push to `main`, and
- manual `workflow_dispatch` from the Actions tab (any branch).

Pipeline:

| Step | Command / action | Output |
|---|---|---|
| Checkout | `actions/checkout@v4` | repository sources |
| Node.js | `actions/setup-node@v4` (Node 22, npm cache) | reproducible dependency install |
| Rust toolchain | `rustup target add wasm32-unknown-unknown` | `wasm32` target |
| Dependencies | `npm ci` (`PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1`) | `node_modules/` |
| Build | `npm run build` (`build-wasm.sh` + `tsc --noEmit` + `vite build`) | `dist/` (~35 MiB) |
| Pages config | `actions/configure-pages@v5` (`enablement: true`) | Pages metadata |
| Artifact | `actions/upload-pages-artifact@v3` (`path: dist`) | Pages tarball |
| Deploy | `actions/deploy-pages@v4` | live site + `page_url` |

`dist/` contents:

```
dist/
├── .nojekyll                            # GitHub Pages should serve assets verbatim
├── index.html                           # relative module/CSS/icon/manifest references
├── assets/index-*.js|css, vm.worker-*.js
├── icon.svg, manifest.webmanifest, sw.js
├── wasm/wasmnux_wasm.wasm               # RV64GC virtualization engine (~305 KiB)
├── images/vmlinux-7.3.1-riscv64.Image.gz
├── images/vanilla-initramfs.cpio.gz
├── images/vanilla-disk.ext4.gz
├── images/debian-rootfs.ext4.gz         # ~28 MiB (< 100 MiB GitHub Pages per-file limit)
└── debian-repo/                         # signed-by nothing, SHA-256 verified Debian trixie riscv64 mirror
```

Total published size is well under both GitHub Pages limits (1 GiB site, 100 MiB per file).

---

## 3. Sub-path correctness

A GitHub Pages project site is served from `/<repository>/`, not `/`. Vite is configured with the
relative base `./` (`vite.config.ts`), and all runtime fetches go through
[`src/runtime/asset-url.ts`](../src/runtime/asset-url.ts):

| Consumer | Resolution |
|---|---|
| `index.html`, JS/CSS/Worker chunks | Vite emits `./assets/...`; no change needed |
| WASM runtime (`public/wasm/wasmnux_wasm.wasm`) | `resolveAssetUrl('wasm/wasmnux_wasm.wasm')` |
| Kernel / initramfs / ext4 base disks | `BootOrchestrator` resolves each profile path against the application base URL |
| Guest `VirtIO-Net` APT mirror | `appBaseUrl` is passed with `init_vm`; `VmEngineHost` derives `{ origin, basePath }` and the bridge requests `<origin><basePath>/debian-repo/...` |
| PWA Service Worker | `sw.js` derives shell URLs from `new URL('./', self.location.href)` and is registered inside the deployment root, so its scope is `/WASMNux/` |

This keeps **same-origin** requests intact (GitHub Pages sends no CORS headers for sub-resources),
so `apt-get update` and `apt-get install` keep working over the guest network bridge.

---

## 4. Local verification of the exact published layout

`npm run dev` / `npm run preview` serve the app from `/`, which does not exercise the sub-path
logic. To reproduce the GitHub Pages layout locally:

```bash
npm run build

rm -rf /tmp/pages && mkdir -p /tmp/pages
cp -a dist /tmp/pages/WASMNux

python3 -m http.server 8080 --directory /tmp/pages
# open http://127.0.0.1:8080/WASMNux/
```

Expected results (verified in headless Chromium):

- the shell reaches `main[data-ready="true"]` and `window.__WASMNUX_APP_SHELL__` is exposed;
- every runtime request stays under `/WASMNux/`:
  `wasm/wasmnux_wasm.wasm`, `images/vmlinux-7.3.1-riscv64.Image.gz`,
  `images/vanilla-initramfs.cpio.gz`, `images/vanilla-disk.ext4.gz`;
- **Vanilla Linux 7.3.1** cold-boots to `root@wasmnux-vanilla:~#` (BusyBox `uname -srm` works);
- **Debian 13.7** cold-boots, `apt-get update` fetches
  `/WASMNux/debian-repo/dists/trixie/{InRelease,Release,Release.gpg,Packages.xz}` and
  `apt-get install wasmnux-hello` installs and runs the package through the bridge
  (`InRelease`/`Release.gpg` return 404 by design — `apt` falls back to `Release`);
- `navigator.serviceWorker.getRegistrations()` reports scope `.../WASMNux/` and the cache is
  `wasmnux-static-v2`;
- no failed requests and no unexpected HTTP status codes.

Automated coverage for this behaviour lives in
[`tests/unit/static-hosting.test.ts`](../tests/unit/static-hosting.test.ts).

---

## 5. Headers, isolation, and known GitHub Pages limitations

GitHub Pages cannot set response headers, so:

- **`Cross-Origin-Opener-Policy` / `Cross-Origin-Embedder-Policy`** are absent and
  `crossOriginIsolated` is `false`. WASMNux does not require `SharedArrayBuffer`: the Worker RPC
  transport falls back to structured-clone/transferable buffers
  (`transportMode: 'worker'`), and the capability panel reports the reduced isolation.
- **No `Content-Encoding: gzip` control.** The `*.gz` guest images are decompressed in-app with
  `DecompressionStream('gzip')`, which auto-detects hosts that already gunzipped the response.
- **No SPA rewrite rules.** The app is a single `index.html` with no client-side router, so this is
  irrelevant. Missing APT metadata files return the real 404 (plus GitHub's HTML 404 page), which
  the network bridge normalizes so `apt` falls back correctly.
- **Service Worker updates.** `public/sw.js` is network-first for the HTML/JS/CSS shell and
  cache-first for immutable guest assets; bump `STATIC_CACHE` (`wasmnux-static-v2`) to invalidate
  cached guest images after replacing them.
- **Caching.** GitHub Pages serves static assets with a 10-minute `max-age` and ignores query
  strings for invalidation; content-hashed Vite chunks handle JS/CSS updates automatically.

---

## 6. Troubleshooting

| Symptom | Cause / fix |
|---|---|
| Workflow fails at **Configure GitHub Pages** with `Resource not accessible by integration` (HTTP 403 on `POST /repos/{owner}/{repo}/pages`) | Pages is not enabled yet. Enable **Settings -> Pages -> Source: GitHub Actions**, then re-run the workflow. The workflow token cannot create the Pages site itself. |
| **Settings -> Pages** shows that Pages is unavailable for this repository | GitHub Pages is not included for private repositories on GitHub Free; make the repository public or use a plan that includes Pages, then set the source to **GitHub Actions**. |
| Deployment succeeds but the site 404s | Pages source is set to "Deploy from a branch" instead of "GitHub Actions", or the deployment ran on a branch that is not allowed by the `github-pages` environment protection rules. |
| Blank page, console 404 on `assets/index-*.js` | A custom `base` was set in `vite.config.ts`. Keep `base: './'`; the runtime resolves sub-paths itself. |
| `apt-get update` fails with `404` on `/debian-repo/...` | The mirror is served relative to the deployment root; confirm `dist/debian-repo/` exists and that the page URL ends with `/WASMNux/`. |
| Terminal works but shows no WebGPU badge | GitHub Pages runs over HTTPS (secure context), so `navigator.gpu` depends on the browser; Canvas 2D rendering is automatic otherwise. |
