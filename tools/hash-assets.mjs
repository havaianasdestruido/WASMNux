#!/usr/bin/env node
/**
 * Regenerates `public/assets-manifest.json` from the bytes actually on disk.
 *
 * The manifest exists so that guest payloads (kernel, initramfs, disk images and the
 * bundled `.deb` repository) can be integrity-checked, and so that a silently replaced
 * or truncated asset is detectable. `tests/unit/assets-manifest.test.ts` fails if the
 * manifest drifts from the shipped files.
 *
 * The WASM runtime is deliberately not listed: it is rebuilt from source by
 * `scripts/build-wasm.sh`, so it is not an immutable input.
 *
 * Usage: node tools/hash-assets.mjs [--check]
 *   --check exits non-zero when the manifest does not match the files on disk.
 */
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const ROOT = join(import.meta.dirname, '..');
const MANIFEST_PATH = join(ROOT, 'public', 'assets-manifest.json');

/** Immutable guest assets, relative to `public/`. */
export const TRACKED_ASSETS = [
  'images/vmlinux-7.3.1-riscv64.Image.gz',
  'images/vanilla-initramfs.cpio.gz',
  'images/vanilla-disk.ext4.gz',
  'images/debian-rootfs.ext4.gz',
  'debian-repo/dists/trixie/Release',
  'debian-repo/dists/trixie/main/binary-riscv64/Packages',
  'debian-repo/dists/trixie/main/binary-riscv64/Packages.xz',
  'debian-repo/pool/main/w/wasmnux-fbtools/wasmnux-fbtools_1.0.0_riscv64.deb',
  'debian-repo/pool/main/w/wasmnux-hello/wasmnux-hello_1.0.0_riscv64.deb',
  'debian-repo/pool/main/w/wasmnux-notes/wasmnux-notes_1.0.0_all.deb',
  'debian-repo/pool/main/w/wasmnux-sysbench/wasmnux-sysbench_1.0.0_riscv64.deb',
];

export function digestAssets() {
  const assets = {};
  for (const relativePath of TRACKED_ASSETS) {
    const bytes = readFileSync(join(ROOT, 'public', relativePath));
    assets[relativePath] = {
      sha256: createHash('sha256').update(bytes).digest('hex'),
      bytes: bytes.length,
    };
  }
  return assets;
}

const assets = digestAssets();
const manifest = {
  manifestVersion: 1,
  description:
    'SHA-256 and byte length of every immutable guest asset shipped in public/. Rebuilt artifacts (the WASM runtime) are intentionally excluded because they are produced from source by scripts/build-wasm.sh.',
  generatedBy: 'node tools/hash-assets.mjs (see docs/ASSETS.md)',
  assets,
};

if (process.argv.includes('--check')) {
  const current = JSON.parse(readFileSync(MANIFEST_PATH, 'utf8'));
  if (JSON.stringify(current.assets) !== JSON.stringify(assets)) {
    console.error('assets-manifest.json is stale; run: node tools/hash-assets.mjs');
    process.exit(1);
  }
  console.log('assets-manifest.json matches the shipped assets');
} else {
  writeFileSync(MANIFEST_PATH, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`wrote ${MANIFEST_PATH} (${String(TRACKED_ASSETS.length)} assets)`);
}
