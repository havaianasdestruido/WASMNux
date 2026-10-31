/**
 * Guest Linux profile definitions, asset paths, and streaming Gzip / sparse-chunk helpers.
 */

import { DISK_CHUNK_BYTES } from '../wasm/bindings';
import type { BaseChunkInput } from '../worker/protocol';

export type LinuxProfileId = 'vanilla' | 'debian';

export interface LinuxProfileDefinition {
  readonly id: LinuxProfileId;
  readonly name: string;
  readonly subtitle: string;
  readonly description: string;
  readonly kernelVersion: string;
  readonly userspaceSummary: string;
  readonly kernelUrl: string;
  readonly initrdUrl: string;
  readonly baseDiskUrl: string;
  readonly defaultRamMib: number;
  readonly diskMib: number;
  readonly bootargs: string;
}

export const LINUX_PROFILES: Readonly<Record<LinuxProfileId, LinuxProfileDefinition>> = {
  vanilla: {
    id: 'vanilla',
    name: 'Vanilla Linux',
    subtitle: 'Linux 7.3.1 · BusyBox 1.36.1 · ext4',
    description:
      'Minimal upstream Linux 7.3.1 (riscv64) booting a static musl BusyBox 1.36.1 initramfs with a persistent 64 MiB ext4 disk on /dev/vda.',
    kernelVersion: '7.3.1',
    userspaceSummary: 'BusyBox 1.36.1 (static riscv64-linux-musl)',
    kernelUrl: '/images/vmlinux-7.3.1-riscv64.Image.gz',
    initrdUrl: '/images/vanilla-initramfs.cpio.gz',
    baseDiskUrl: '/images/vanilla-disk.ext4.gz',
    defaultRamMib: 128,
    diskMib: 64,
    bootargs: 'console=ttyS0,115200 earlycon=sbi panic=-1',
  },
  debian: {
    id: 'debian',
    name: 'Debian 13 ("trixie")',
    subtitle: 'Debian 13.7 · glibc 2.41 · apt 3.0 / dpkg',
    description:
      'Official stable Debian 13.7 ("trixie") riscv64 userspace with real apt/dpkg package management, BusyBox utilities, and persistent ext4 rootfs.',
    kernelVersion: '7.3.1',
    userspaceSummary: 'Debian 13.7 "trixie" riscv64 (glibc 2.41, apt 3.0.3, dpkg 1.22.22)',
    kernelUrl: '/images/vmlinux-7.3.1-riscv64.Image.gz',
    initrdUrl: '/images/debian-initramfs.cpio.gz',
    baseDiskUrl: '/images/debian-rootfs.ext4.gz',
    defaultRamMib: 256,
    diskMib: 256,
    bootargs: 'console=ttyS0,115200 earlycon=sbi root=/dev/vda rw rootwait panic=-1',
  },
};

export function getLinuxProfile(id: LinuxProfileId): LinuxProfileDefinition {
  return LINUX_PROFILES[id];
}

/**
 * Decompress a Gzipped `Uint8Array` (or return unchanged if it does not begin with Gzip magic `1f 8b`).
 */
export async function decompressGzipIfNeeded(input: Uint8Array): Promise<Uint8Array> {
  if (input.byteLength < 2 || input[0] !== 0x1f || input[1] !== 0x8b) {
    return input;
  }
  const ds = new DecompressionStream('gzip');
  const writer = ds.writable.getWriter();
  const writePromise = writer.write(new Uint8Array(input)).then(() => writer.close());

  const reader = ds.readable.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const step = await reader.read();
    if (step.done) {
      break;
    }
    chunks.push(step.value);
    total += step.value.byteLength;
  }
  await writePromise;

  const out = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.byteLength;
  }
  return out;
}

/**
 * Slice a raw virtual disk image into non-zero `64 KiB` sparse base chunks.
 * All-zero chunks are omitted so multi-hundred-MiB ext4 filesystems transfer in milliseconds.
 */
export function extractNonZeroBaseChunks(rawDisk: Uint8Array): BaseChunkInput[] {
  const out: BaseChunkInput[] = [];
  const totalChunks = Math.ceil(rawDisk.byteLength / DISK_CHUNK_BYTES);
  for (let idx = 0; idx < totalChunks; idx += 1) {
    const start = idx * DISK_CHUNK_BYTES;
    const end = Math.min(start + DISK_CHUNK_BYTES, rawDisk.byteLength);
    const view = rawDisk.subarray(start, end);
    let hasNonZero = false;
    for (let i = 0; i < view.byteLength; i += 1) {
      if (view[i] !== 0) {
        hasNonZero = true;
        break;
      }
    }
    if (!hasNonZero) {
      continue;
    }
    const block = new Uint8Array(DISK_CHUNK_BYTES);
    block.set(view, 0);
    out.push({ chunkIndex: idx, data: block });
  }
  return out;
}
