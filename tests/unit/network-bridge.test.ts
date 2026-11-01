import { describe, expect, it } from 'vitest';
import {
  executeBridgedHttpRequest,
  isBlockedPrivateOrLoopbackHost,
  resolveBridgeTarget,
  sanitizeRequestPath,
} from '../../src/net/network-bridge';

describe('Browser Network Bridge & SSRF Guardrails (Phase 9)', () => {
  it('blocks loopback, link-local, and RFC1918 private hosts while permitting explicit virtual gateway hosts', () => {
    expect(isBlockedPrivateOrLoopbackHost('apt.wasmnux.local')).toBe(false);
    expect(isBlockedPrivateOrLoopbackHost('gateway.wasmnux.local:80')).toBe(false);
    expect(isBlockedPrivateOrLoopbackHost('10.0.2.2:80')).toBe(false);

    expect(isBlockedPrivateOrLoopbackHost('localhost')).toBe(true);
    expect(isBlockedPrivateOrLoopbackHost('api.localhost:3000')).toBe(true);
    expect(isBlockedPrivateOrLoopbackHost('127.0.0.1:8080')).toBe(true);
    expect(isBlockedPrivateOrLoopbackHost('169.254.169.254')).toBe(true);
    expect(isBlockedPrivateOrLoopbackHost('192.168.1.1')).toBe(true);
    expect(isBlockedPrivateOrLoopbackHost('172.16.0.5')).toBe(true);
    expect(isBlockedPrivateOrLoopbackHost('10.0.0.1')).toBe(true);
    expect(isBlockedPrivateOrLoopbackHost('[::1]:8080')).toBe(true);
    expect(isBlockedPrivateOrLoopbackHost('router.local')).toBe(true);
  });

  it('rejects path traversal and maps apt.wasmnux.local to same-origin /debian-repo/*', () => {
    expect(sanitizeRequestPath('/debian-repo/../secret')).toBeNull();
    expect(sanitizeRequestPath('/bad\\path')).toBeNull();

    const localTarget = resolveBridgeTarget(
      { host: 'apt.wasmnux.local', path: '/debian-repo/dists/trixie/Release' },
      'https://preview.e2b.app/',
    );
    expect(localTarget).toEqual({
      kind: 'local-mirror',
      url: 'https://preview.e2b.app/debian-repo/dists/trixie/Release',
    });

    const extTarget = resolveBridgeTarget(
      { host: 'example.com', path: '/index.txt' },
      'https://preview.e2b.app',
    );
    expect(extTarget).toEqual({
      kind: 'external-https',
      url: 'https://example.com/index.txt',
    });
  });

  it('converts SPA text/html fallback on /debian-repo/* into 404 and enforces networkEnabled policy', async () => {
    const disabledResp = await executeBridgedHttpRequest(
      {
        requestId: 1n,
        method: 'GET',
        host: 'apt.wasmnux.local',
        path: '/debian-repo/dists/trixie/Release',
        headers: [],
        body: new Uint8Array(0),
      },
      { networkEnabled: false, origin: 'http://localhost:4173' },
    );
    expect(disabledResp.statusCode).toBe(503);

    const spaFallbackResp = await executeBridgedHttpRequest(
      {
        requestId: 2n,
        method: 'GET',
        host: 'apt.wasmnux.local',
        path: '/debian-repo/dists/trixie/InRelease',
        headers: [],
        body: new Uint8Array(0),
      },
      {
        networkEnabled: true,
        origin: 'http://localhost:4173',
        fetchImpl: () =>
          Promise.resolve(
            new Response('<!DOCTYPE html><html></html>', {
              status: 200,
              headers: { 'Content-Type': 'text/html; charset=utf-8' },
            }),
          ),
      },
    );
    expect(spaFallbackResp.statusCode).toBe(404);

    const ssrfBlocked = await executeBridgedHttpRequest(
      {
        requestId: 3n,
        method: 'GET',
        host: '127.0.0.1:9229',
        path: '/json',
        headers: [],
        body: new Uint8Array(0),
      },
      { networkEnabled: true, origin: 'http://localhost:4173' },
    );
    expect(ssrfBlocked.statusCode).toBe(403);
  });
});
