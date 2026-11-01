/*
 * WASMNux PWA Service Worker (`wasmnux-static-v2`).
 *
 * Strictly caches immutable/static application shell, WASM runtime, kernel/rootfs base images,
 * and local `<deployment-root>/debian-repo/` APT packages for offline boot & install capability.
 * Guest persistent virtual disks live exclusively in OPFS + IndexedDB and are NEVER stored in
 * or cleared by Cache Storage.
 *
 * All URLs are derived from the Service Worker scope so the same script works when the site is
 * hosted at the domain root (`/`) or under a sub-path such as a GitHub Pages project site
 * (`/WASMNux/`).
 */

const STATIC_CACHE = 'wasmnux-static-v2';

// App root derived from the worker script location, always with a trailing slash:
// `https://host/` -> `/`, `https://host/WASMNux/sw.js` -> `/WASMNux/`.
const APP_ROOT = `${new URL('./', self.location.href).pathname.replace(/\/+$/, '')}/`;

const CORE_SHELL_URLS = [
  APP_ROOT,
  `${APP_ROOT}manifest.webmanifest`,
  `${APP_ROOT}icon.svg`,
  `${APP_ROOT}wasm/wasmnux_wasm.wasm`,
];

const GUEST_ASSET_PREFIXES = ['wasm/', 'images/', 'debian-repo/'];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(STATIC_CACHE)
      .then((cache) => cache.addAll(CORE_SHELL_URLS))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((k) => k.startsWith('wasmnux-static-') && k !== STATIC_CACHE)
            .map((k) => caches.delete(k)),
        ),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const req = event.request;
  if (req.method !== 'GET') {
    return;
  }
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) {
    return;
  }

  const isImmutableGuestAsset = GUEST_ASSET_PREFIXES.some((prefix) =>
    url.pathname.startsWith(`${APP_ROOT}${prefix}`),
  );

  if (isImmutableGuestAsset) {
    event.respondWith(
      caches.open(STATIC_CACHE).then(async (cache) => {
        const cached = await cache.match(req);
        if (cached) {
          return cached;
        }
        const netResp = await fetch(req);
        if (netResp.ok && !(netResp.headers.get('content-type') || '').includes('text/html')) {
          void cache.put(req, netResp.clone());
        }
        return netResp;
      }),
    );
    return;
  }

  // Network-first with offline cache fallback for HTML/JS/CSS shell assets
  event.respondWith(
    fetch(req)
      .then((resp) => {
        if (resp.ok) {
          const copy = resp.clone();
          void caches.open(STATIC_CACHE).then((cache) => cache.put(req, copy));
        }
        return resp;
      })
      .catch(async () => {
        const cached = await caches.match(req);
        if (cached) {
          return cached;
        }
        throw new Error('Offline and asset not cached');
      }),
  );
});
