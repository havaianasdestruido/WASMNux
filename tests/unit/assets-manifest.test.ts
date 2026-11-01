/**
 * Guards the shipped guest assets against silent replacement or truncation.
 *
 * The provenance of each payload is documented in `docs/ASSETS.md`; this test
 * recomputes SHA-256 for every entry and fails if the manifest drifts from disk.
 */
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import manifest from '../../public/assets-manifest.json';

const ROOT = join(import.meta.dirname, '..', '..');

/** Assets whose provenance and integrity the project depends on. */
const REQUIRED_ASSETS = [
  'images/vmlinux-7.3.1-riscv64.Image.gz',
  'images/vanilla-initramfs.cpio.gz',
  'images/vanilla-disk.ext4.gz',
  'images/debian-rootfs.ext4.gz',
  'debian-repo/dists/trixie/Release',
  'debian-repo/dists/trixie/main/binary-riscv64/Packages',
  'debian-repo/pool/main/w/wasmnux-hello/wasmnux-hello_1.0.0_riscv64.deb',
] as const;

describe('guest asset manifest', () => {
  it('lists every required guest asset with a well-formed digest', () => {
    for (const relativePath of REQUIRED_ASSETS) {
      const entry = manifest.assets[relativePath as keyof typeof manifest.assets];
      expect(entry, `manifest missing ${relativePath}`).toBeDefined();
      expect(entry.sha256).toMatch(/^[0-9a-f]{64}$/u);
      expect(entry.bytes).toBeGreaterThan(0);
    }
    for (const relativePath of Object.keys(manifest.assets)) {
      expect(relativePath).not.toMatch(/^\/|\.\./u);
    }
  });

  it('matches the exact bytes shipped in public/', () => {
    for (const relativePath of Object.keys(manifest.assets)) {
      const bytes = readFileSync(join(ROOT, 'public', relativePath));
      const entry = manifest.assets[relativePath as keyof typeof manifest.assets];
      expect(bytes.length, `size drift for ${relativePath}`).toBe(entry.bytes);
      expect(createHash('sha256').update(bytes).digest('hex'), `hash drift for ${relativePath}`).toBe(
        entry.sha256,
      );
    }
  });
});
