/* WASMNux service worker: caches the *application shell* only.
 *
 * Guest disks are NOT here — they live in OPFS/IndexedDB via the app's
 * ChunkStore. Guest OS assets (/guest/) and the WASM binary (/wasm/) are
 * large and version-critical, so they are network-first and never served
 * stale.
 */
const SHELL_CACHE = "wasmnux-shell-v1";
const SHELL = ["/", "/index.html", "/manifest.webmanifest", "/icon.svg"];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(SHELL_CACHE)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((k) => k !== SHELL_CACHE).map((k) => caches.delete(k))),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== "GET" || url.origin !== self.location.origin) {
    return;
  }
  const isGuestOrWasm = url.pathname.startsWith("/guest/") || url.pathname.startsWith("/wasm/");
  if (isGuestOrWasm) {
    event.respondWith(fetch(event.request));
    return;
  }
  // App shell + hashed build assets: cache-first with background refresh.
  event.respondWith(
    caches.match(event.request).then((cached) => {
      const network = fetch(event.request)
        .then((response) => {
          if (response.ok) {
            const copy = response.clone();
            void caches.open(SHELL_CACHE).then((cache) => cache.put(event.request, copy));
          }
          return response;
        })
        .catch(() => cached);
      return cached ?? network;
    }),
  );
});
