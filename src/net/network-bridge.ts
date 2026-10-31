/**
 * Browser-compatible HTTP network bridge for VirtIO-Net + `wasmnux-net`.
 *
 * Security & browser boundary model:
 * - Maps guest virtual gateway hosts (`apt.wasmnux.local`, `gateway.wasmnux.local`, `10.0.2.2`)
 *   exclusively to safe same-origin static asset paths (`/debian-repo/*`, `/images/*`, etc.).
 * - Blocks SSRF / host-loopback / RFC1918 / link-local targets (`localhost`, `127.*`, `169.254.*`,
 *   `192.168.*`, `172.16-31.*`, `10.*` other than `10.0.2.2`, `[::1]`) so an untrusted guest
 *   process cannot probe services running on the user's machine or LAN.
 * - Converts SPA HTML fallback responses (`text/html` on `/debian-repo/*`) into HTTP 404 so
 *   Debian `apt` cleanly falls back from missing `InRelease` to `Release`.
 * - Enforces runtime `networkEnabled` policy and surfaces explicit HTTP 502/503 diagnostics
 *   for offline or CORS-blocked external endpoints.
 */

import type { BridgeHttpRequestMsg, BridgeHttpResponseMsg } from '../wasm/bindings';

export const MAX_BRIDGED_RESPONSE_BYTES = 64 * 1024 * 1024;

export interface NetworkBridgeOptions {
  readonly networkEnabled: boolean;
  readonly origin?: string;
  readonly fetchImpl?: typeof fetch;
  readonly timeoutMs?: number;
}

export interface ResolvedBridgeTarget {
  readonly kind: 'local-mirror' | 'external-https' | 'blocked';
  readonly url: string;
  readonly reason?: string;
}

const LOCAL_MIRROR_HOSTS = new Set(['apt.wasmnux.local', 'gateway.wasmnux.local', '10.0.2.2']);

/**
 * Strip optional `:port` from an HTTP `Host` header while preserving IPv6 bracket notation.
 */
export function extractHostname(rawHost: string): string {
  const trimmed = rawHost.trim().toLowerCase();
  if (trimmed.startsWith('[')) {
    const closeIdx = trimmed.indexOf(']');
    return closeIdx >= 0 ? trimmed.slice(0, closeIdx + 1) : trimmed;
  }
  const colonIdx = trimmed.indexOf(':');
  return colonIdx >= 0 ? trimmed.slice(0, colonIdx) : trimmed;
}

/**
 * Determine whether a hostname resolves to a loopback, link-local, or private LAN address
 * that must not be reachable from guest processes (SSRF protection).
 */
export function isBlockedPrivateOrLoopbackHost(hostname: string): boolean {
  const h = extractHostname(hostname);
  if (LOCAL_MIRROR_HOSTS.has(h)) {
    return false;
  }
  if (
    h === '' ||
    h === 'localhost' ||
    h.endsWith('.localhost') ||
    h.endsWith('.local') ||
    h.endsWith('.internal') ||
    h === '[::1]' ||
    h === '[::]' ||
    h.startsWith('[fe80:') ||
    h.startsWith('[fc') ||
    h.startsWith('[fd')
  ) {
    return true;
  }

  const ipv4Match = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(h);
  if (ipv4Match !== null) {
    const a = Number(ipv4Match[1]);
    const b = Number(ipv4Match[2]);
    if (a === 0 || a === 10 || a === 127) {
      return true;
    }
    if (a === 169 && b === 254) {
      return true;
    }
    if (a === 172 && b >= 16 && b <= 31) {
      return true;
    }
    if (a === 192 && b === 168) {
      return true;
    }
    if (a === 100 && b >= 64 && b <= 127) {
      return true;
    }
    if (a >= 224) {
      return true;
    }
  }

  return false;
}

/**
 * Normalize and validate an HTTP request path to prevent path-traversal or scheme injection.
 */
export function sanitizeRequestPath(rawPath: string): string | null {
  if (rawPath.length === 0 || rawPath.length > 4096) {
    return null;
  }
  let pathOnly = rawPath;
  if (rawPath.startsWith('http://') || rawPath.startsWith('https://')) {
    try {
      const parsed = new URL(rawPath);
      pathOnly = `${parsed.pathname}${parsed.search}`;
    } catch {
      return null;
    }
  }
  if (!pathOnly.startsWith('/')) {
    pathOnly = `/${pathOnly}`;
  }
  if (pathOnly.includes('\0') || pathOnly.includes('\\') || pathOnly.includes('/..')) {
    return null;
  }
  return pathOnly;
}

/**
 * Resolve a guest HTTP request into a validated target URL or block reason.
 */
export function resolveBridgeTarget(
  req: Pick<BridgeHttpRequestMsg, 'host' | 'path'>,
  origin: string,
): ResolvedBridgeTarget {
  const safePath = sanitizeRequestPath(req.path);
  if (safePath === null) {
    return {
      kind: 'blocked',
      url: '',
      reason: 'Rejected malformed or traversal HTTP path',
    };
  }

  const hostOnly = extractHostname(req.host);
  if (LOCAL_MIRROR_HOSTS.has(hostOnly)) {
    const cleanOrigin = origin.endsWith('/') ? origin.slice(0, -1) : origin;
    return {
      kind: 'local-mirror',
      url: `${cleanOrigin}${safePath}`,
    };
  }

  if (isBlockedPrivateOrLoopbackHost(hostOnly)) {
    return {
      kind: 'blocked',
      url: '',
      reason: `Blocked private/loopback host '${hostOnly}' by WASMNux SSRF security policy`,
    };
  }

  if (!/^[a-z0-9.-]+$/.test(hostOnly)) {
    return {
      kind: 'blocked',
      url: '',
      reason: `Invalid external hostname '${hostOnly}'`,
    };
  }

  return {
    kind: 'external-https',
    url: `https://${hostOnly}${safePath}`,
  };
}

function makeTextErrorResponse(
  requestId: bigint,
  statusCode: number,
  statusText: string,
  message: string,
): BridgeHttpResponseMsg {
  return {
    requestId,
    statusCode,
    statusText,
    headers: [['Content-Type', 'text/plain; charset=utf-8']],
    body: new TextEncoder().encode(`${message}\n`),
  };
}

/**
 * Execute a bridged HTTP request from the guest VM via `fetch()` with SSRF, size, and SPA-fallback guards.
 */
export async function executeBridgedHttpRequest(
  req: BridgeHttpRequestMsg,
  options: NetworkBridgeOptions,
): Promise<BridgeHttpResponseMsg> {
  if (!options.networkEnabled) {
    return makeTextErrorResponse(
      req.requestId,
      503,
      'Service Unavailable',
      'WASMNux network bridge is currently disabled for this VM session.',
    );
  }

  const defaultOrigin =
    typeof globalThis.location === 'object' && typeof globalThis.location.origin === 'string'
      ? globalThis.location.origin
      : 'http://localhost';
  const origin = options.origin ?? defaultOrigin;
  const resolved = resolveBridgeTarget(req, origin);

  if (resolved.kind === 'blocked') {
    return makeTextErrorResponse(
      req.requestId,
      403,
      'Forbidden',
      resolved.reason ?? 'Blocked by WASMNux network policy',
    );
  }

  const fetchFn = options.fetchImpl ?? globalThis.fetch;
  const method = req.method.toUpperCase();
  if (method !== 'GET' && method !== 'HEAD' && method !== 'POST' && method !== 'PUT') {
    return makeTextErrorResponse(
      req.requestId,
      405,
      'Method Not Allowed',
      `Unsupported HTTP method '${method}' on browser network bridge`,
    );
  }

  const reqHeaders = new Headers();
  for (const [k, v] of req.headers) {
    const lk = k.toLowerCase();
    if (
      lk !== 'host' &&
      lk !== 'connection' &&
      lk !== 'content-length' &&
      !lk.startsWith('sec-') &&
      lk !== 'origin' &&
      lk !== 'referer'
    ) {
      reqHeaders.set(k, v);
    }
  }

  const controller = new AbortController();
  const timeoutMs = options.timeoutMs ?? 20_000;
  const timer = setTimeout(() => {
    controller.abort();
  }, timeoutMs);

  try {
    const init: RequestInit = {
      method,
      headers: reqHeaders,
      signal: controller.signal,
      credentials: resolved.kind === 'local-mirror' ? 'same-origin' : 'omit',
    };
    if (method !== 'GET' && method !== 'HEAD' && req.body.byteLength > 0) {
      init.body = req.body.slice();
    }

    const resp = await fetchFn(resolved.url, init);
    const contentType = (resp.headers.get('content-type') ?? '').toLowerCase();

    // Vite / SPA static servers return index.html (200 text/html) for extensionless paths
    // such as `/debian-repo/dists/trixie/InRelease`. Convert SPA HTML fallbacks on `/debian-repo/`
    // into clean HTTP 404 responses so `apt` falls back to `Release`.
    if (
      resolved.kind === 'local-mirror' &&
      req.path.startsWith('/debian-repo/') &&
      resp.status === 200 &&
      contentType.includes('text/html')
    ) {
      return makeTextErrorResponse(req.requestId, 404, 'Not Found', 'APT metadata file not found');
    }

    const declaredLength = Number(resp.headers.get('content-length') ?? '0');
    if (Number.isFinite(declaredLength) && declaredLength > MAX_BRIDGED_RESPONSE_BYTES) {
      return makeTextErrorResponse(
        req.requestId,
        413,
        'Payload Too Large',
        `Bridged HTTP response exceeds ${String(MAX_BRIDGED_RESPONSE_BYTES)} byte limit`,
      );
    }

    const bodyBuf = new Uint8Array(await resp.arrayBuffer());
    if (bodyBuf.byteLength > MAX_BRIDGED_RESPONSE_BYTES) {
      return makeTextErrorResponse(
        req.requestId,
        413,
        'Payload Too Large',
        `Bridged HTTP response exceeds ${String(MAX_BRIDGED_RESPONSE_BYTES)} byte limit`,
      );
    }

    const outHeaders: Array<readonly [string, string]> = [];
    resp.headers.forEach((value, key) => {
      const lk = key.toLowerCase();
      if (lk !== 'content-encoding' && lk !== 'transfer-encoding' && lk !== 'content-length') {
        outHeaders.push([key, value]);
      }
    });

    return {
      requestId: req.requestId,
      statusCode: resp.status,
      statusText: resp.statusText || (resp.ok ? 'OK' : 'Error'),
      headers: outHeaders,
      body: bodyBuf,
    };
  } catch (err: unknown) {
    const errText = err instanceof Error ? err.message : 'Browser fetch blocked or offline';
    const corsHint =
      resolved.kind === 'external-https'
        ? ' (External HTTPS endpoints must allow CORS in browser environments.)'
        : '';
    return makeTextErrorResponse(
      req.requestId,
      502,
      'Bad Gateway',
      `Bridge HTTP fetch failed (${resolved.url}): ${errText}${corsHint}`,
    );
  } finally {
    clearTimeout(timer);
  }
}
