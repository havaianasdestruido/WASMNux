import { describe, expect, it } from 'vitest';
import { executeBridgedHttpRequest, resolveBridgeTarget } from '../../src/net/network-bridge';
import {
  normalizeBasePath,
  resolveAppBaseUrl,
  resolveAssetUrl,
  splitAppBaseUrl,
} from '../../src/runtime/asset-url';
import { BootOrchestrator } from '../../src/runtime/boot-orchestrator';
import { MemoryStorageAdapter, PersistentStorageEngine } from '../../src/storage/storage-engine';

const PAGES_ORIGIN = 'https://havaianasdestruido.github.io';
const PAGES_BASE = `${PAGES_ORIGIN}/WASMNux/`;

describe('Static hosting & GitHub Pages sub-path deployment', () => {
  it('resolves published assets under the domain root and under a project sub-path', () => {
    expect(
      resolveAssetUrl('/images/vmlinux-7.3.1-riscv64.Image.gz', 'http://127.0.0.1:4173/'),
    ).toBe('http://127.0.0.1:4173/images/vmlinux-7.3.1-riscv64.Image.gz');
    expect(resolveAssetUrl('/images/vmlinux-7.3.1-riscv64.Image.gz', PAGES_BASE)).toBe(
      `${PAGES_BASE}images/vmlinux-7.3.1-riscv64.Image.gz`,
    );
    expect(resolveAssetUrl('wasm/wasmnux_wasm.wasm', PAGES_BASE)).toBe(
      `${PAGES_BASE}wasm/wasmnux_wasm.wasm`,
    );
    expect(resolveAssetUrl('debian-repo/dists/trixie/Release', 'https://host/a/b/')).toBe(
      'https://host/a/b/debian-repo/dists/trixie/Release',
    );
  });

  it('keeps absolute asset URLs untouched and falls back to the document base URL', () => {
    expect(resolveAssetUrl('https://cdn.example.com/x.wasm', PAGES_BASE)).toBe(
      'https://cdn.example.com/x.wasm',
    );
    expect(resolveAssetUrl('//cdn.example.com/x.wasm', PAGES_BASE)).toBe(
      '//cdn.example.com/x.wasm',
    );
    expect(resolveAssetUrl('data:application/wasm;base64,AAAA', PAGES_BASE)).toBe(
      'data:application/wasm;base64,AAAA',
    );
    // Under Vitest there is no `document`, so the base resolves against the synthetic origin.
    expect(resolveAppBaseUrl('./')).toBe('http://localhost/');
  });

  it('normalizes base paths and splits deployment base URLs', () => {
    expect(normalizeBasePath('/WASMNux/')).toBe('/WASMNux');
    expect(normalizeBasePath('WASMNux')).toBe('/WASMNux');
    expect(normalizeBasePath('//WASMNux///')).toBe('/WASMNux');
    expect(normalizeBasePath('/')).toBe('');
    expect(normalizeBasePath('')).toBe('');
    expect(splitAppBaseUrl(PAGES_BASE)).toEqual({
      origin: PAGES_ORIGIN,
      basePath: '/WASMNux',
    });
    expect(splitAppBaseUrl('http://127.0.0.1:4173/')).toEqual({
      origin: 'http://127.0.0.1:4173',
      basePath: '',
    });
    expect(splitAppBaseUrl('not a url')).toBeNull();
  });

  it('routes guest APT mirror requests to <origin><basePath>/debian-repo/*', async () => {
    expect(
      resolveBridgeTarget(
        { host: 'apt.wasmnux.local', path: '/debian-repo/dists/trixie/Release' },
        PAGES_ORIGIN,
        '/WASMNux',
      ),
    ).toEqual({
      kind: 'local-mirror',
      url: `${PAGES_BASE}debian-repo/dists/trixie/Release`,
    });

    const fetched: string[] = [];
    const response = await executeBridgedHttpRequest(
      {
        requestId: 7n,
        method: 'GET',
        host: 'apt.wasmnux.local',
        path: '/debian-repo/pool/main/w/wasmnux-hello/wasmnux-hello_1.0.0_riscv64.deb',
        headers: [],
        body: new Uint8Array(0),
      },
      {
        networkEnabled: true,
        origin: PAGES_ORIGIN,
        basePath: '/WASMNux',
        fetchImpl: (input: RequestInfo | URL) => {
          fetched.push(
            typeof input === 'string' ? input : input instanceof URL ? input.href : input.url,
          );
          return Promise.resolve(
            new Response('deb-payload', {
              status: 200,
              headers: { 'Content-Type': 'application/vnd.debian.binary-package' },
            }),
          );
        },
      },
    );

    expect(fetched).toEqual([
      `${PAGES_BASE}debian-repo/pool/main/w/wasmnux-hello/wasmnux-hello_1.0.0_riscv64.deb`,
    ]);
    expect(response.statusCode).toBe(200);
    expect(new TextDecoder().decode(response.body)).toBe('deb-payload');
  });

  it('resolves guest kernel assets against the configured application base URL', async () => {
    const requested: string[] = [];
    const orchestrator = new BootOrchestrator({
      storage: new PersistentStorageEngine(new MemoryStorageAdapter()),
      appBaseUrl: PAGES_BASE,
      assetFetcher: (url: string) => {
        requested.push(url);
        return Promise.reject(new Error('asset-fetch-stop'));
      },
    });

    await expect(orchestrator.bootProfile('vanilla')).rejects.toThrow('asset-fetch-stop');
    expect(requested).toEqual([`${PAGES_BASE}images/vmlinux-7.3.1-riscv64.Image.gz`]);
  });
});
