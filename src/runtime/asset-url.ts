/**
 * Base-aware URL helpers for statically hosted builds.
 *
 * WASMNux can be served either from the domain root (`http://localhost:5173/`,
 * `https://example.com/`) or from a nested sub-path such as the GitHub Pages project site
 * `https://<owner>.github.io/WASMNux/`. `vite.config.ts` uses the relative base `./`, so the
 * emitted HTML/CSS/JS chunks resolve correctly under any prefix, but runtime `fetch()` calls
 * (WASM runtime, kernel, initramfs, ext4 base disks, `/debian-repo/` APT mirror) must be
 * resolved against the *application* base URL rather than the origin root.
 *
 * Everything here is pure and DOM-optional so it can run on the main thread, inside the
 * dedicated Web Worker, and under Node during Vitest runs.
 */

const ABSOLUTE_URL_RE = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i;

function currentDocumentHref(): string {
  if (typeof document === 'object' && document.baseURI.length > 0) {
    return document.baseURI;
  }
  if (typeof globalThis.location === 'object' && globalThis.location.href.length > 0) {
    return globalThis.location.href;
  }
  return 'http://localhost/';
}

/**
 * Resolve the application base directory (always an absolute URL ending in `/`).
 *
 * `base` defaults to the Vite-injected `import.meta.env.BASE_URL` (`./` for this project),
 * which is resolved against the current document (or worker) URL.
 */
export function resolveAppBaseUrl(base: string = import.meta.env.BASE_URL): string {
  const effectiveBase = typeof base === 'string' && base.length > 0 ? base : './';
  const resolved = new URL(effectiveBase, currentDocumentHref());
  return resolved.pathname.endsWith('/') ? resolved.href : `${resolved.href}/`;
}

/**
 * Resolve a published static asset path (for example `/images/vmlinux.gz`) against the
 * application base directory. Absolute URLs (`https:`, `data:`, `blob:`, protocol-relative
 * `//cdn/...`) are returned untouched.
 */
export function resolveAssetUrl(path: string, appBaseUrl: string = resolveAppBaseUrl()): string {
  if (typeof path !== 'string' || path.length === 0) {
    return appBaseUrl;
  }
  if (ABSOLUTE_URL_RE.test(path)) {
    return path;
  }
  const base = appBaseUrl.endsWith('/') ? appBaseUrl : `${appBaseUrl}/`;
  return new URL(path.replace(/^\/+/, ''), base).href;
}

/**
 * Normalize a base pathname for the in-WASM HTTP bridge: always begins with `/` and never
 * ends with `/` (`''` means "served from the origin root").
 */
export function normalizeBasePath(pathname: string): string {
  if (typeof pathname !== 'string') {
    return '';
  }
  const trimmed = pathname.trim().replace(/^\/+/, '').replace(/\/+$/, '');
  return trimmed.length === 0 ? '' : `/${trimmed}`;
}

export interface AppBaseUrlParts {
  readonly origin: string;
  readonly basePath: string;
}

/**
 * Split an absolute application base URL into the origin and the normalized sub-path that the
 * guest `VirtIO-Net` HTTP bridge must prefix onto same-origin mirror requests.
 */
export function splitAppBaseUrl(appBaseUrl: string): AppBaseUrlParts | null {
  try {
    const parsed = new URL(appBaseUrl);
    return {
      origin: parsed.origin,
      basePath: normalizeBasePath(parsed.pathname),
    };
  } catch {
    return null;
  }
}
