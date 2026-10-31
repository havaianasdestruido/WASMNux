/**
 * IEEE 802.3 CRC32 checksum, identifier sanitization, and `.wasmnux-disk` binary bundle codec.
 * Wire-compatible with `crates/wasmnux-storage`.
 */

import { DISK_CHUNK_BYTES } from '../wasm/bindings';

const CRC32_TABLE: Uint32Array = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i += 1) {
    let crc = i;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc & 1) !== 0 ? (crc >>> 1) ^ 0xedb8_8320 : crc >>> 1;
    }
    table[i] = crc >>> 0;
  }
  return table;
})();

export function crc32(bytes: Uint8Array): number {
  let crc = 0xffff_ffff;
  for (let i = 0; i < bytes.byteLength; i += 1) {
    const b = bytes[i] ?? 0;
    const lookup = CRC32_TABLE[(crc ^ b) & 0xff] ?? 0;
    crc = (crc >>> 8) ^ lookup;
  }
  return ~crc >>> 0;
}

export class StorageValidationError extends Error {
  public readonly code:
    | 'invalid_id'
    | 'corrupt_bundle'
    | 'checksum_mismatch'
    | 'profile_mismatch'
    | 'quota_exceeded'
    | 'oversized_bundle'
    | 'io_interrupted';

  public constructor(
    code:
      | 'invalid_id'
      | 'corrupt_bundle'
      | 'checksum_mismatch'
      | 'profile_mismatch'
      | 'quota_exceeded'
      | 'oversized_bundle'
      | 'io_interrupted',
    message: string,
  ) {
    super(message);
    this.name = 'StorageValidationError';
    this.code = code;
  }
}

export function validateStorageIdentifier(id: string): void {
  if (id.length === 0 || id.length > 63 || !/^[a-z0-9][a-z0-9_-]*$/.test(id)) {
    throw new StorageValidationError(
      'invalid_id',
      `Invalid storage/profile identifier '${id}' (must match ^[a-z0-9][a-z0-9_-]{0,62}$)`,
    );
  }
}

export const DISK_BUNDLE_MAGIC = 'WASMNUXD';
export const DISK_BUNDLE_VERSION = 1;
export const MAX_IMPORT_BUNDLE_BYTES = 512 * 1024 * 1024; // 512 MiB safety cap

export interface ParsedDiskBundle {
  readonly profileId: string;
  readonly capacityBytes: number;
  readonly chunks: ReadonlyArray<{
    readonly chunkIndex: number;
    readonly crc32: number;
    readonly data: Uint8Array;
  }>;
}

const utf8Encoder = new TextEncoder();
const utf8Decoder = new TextDecoder('utf-8', { fatal: true });

export function encodeDiskOverlayBundle(
  profileId: string,
  capacityBytes: number,
  chunks: ReadonlyArray<{ readonly chunkIndex: number; readonly data: Uint8Array }>,
): Uint8Array {
  validateStorageIdentifier(profileId);
  const idBytes = utf8Encoder.encode(profileId);
  const magicBytes = utf8Encoder.encode(DISK_BUNDLE_MAGIC);
  const sorted = [...chunks].sort((a, b) => a.chunkIndex - b.chunkIndex);

  const payloadLen =
    8 + 4 + 2 + idBytes.byteLength + 8 + 4 + sorted.length * (8 + DISK_CHUNK_BYTES);
  const out = new Uint8Array(payloadLen + 4);
  const dv = new DataView(out.buffer);

  out.set(magicBytes, 0);
  let cur = 8;
  dv.setUint32(cur, DISK_BUNDLE_VERSION, true);
  cur += 4;
  dv.setUint16(cur, idBytes.byteLength, true);
  cur += 2;
  out.set(idBytes, cur);
  cur += idBytes.byteLength;
  dv.setBigUint64(cur, BigInt(capacityBytes), true);
  cur += 8;
  dv.setUint32(cur, sorted.length, true);
  cur += 4;

  for (const item of sorted) {
    if (item.data.byteLength !== DISK_CHUNK_BYTES) {
      throw new StorageValidationError('corrupt_bundle', 'Invalid chunk byte length during export');
    }
    const chunkCrc = crc32(item.data);
    dv.setUint32(cur, item.chunkIndex >>> 0, true);
    cur += 4;
    dv.setUint32(cur, chunkCrc, true);
    cur += 4;
    out.set(item.data, cur);
    cur += DISK_CHUNK_BYTES;
  }

  const trailerCrc = crc32(out.subarray(0, payloadLen));
  dv.setUint32(payloadLen, trailerCrc, true);
  return out;
}

export function decodeAndValidateDiskOverlayBundle(
  bundle: Uint8Array,
  expectedProfileId?: string,
): ParsedDiskBundle {
  if (bundle.byteLength > MAX_IMPORT_BUNDLE_BYTES) {
    throw new StorageValidationError(
      'oversized_bundle',
      `Disk bundle (${String(bundle.byteLength)} bytes) exceeds ${String(MAX_IMPORT_BUNDLE_BYTES)} ceiling`,
    );
  }
  if (bundle.byteLength < 31) {
    throw new StorageValidationError('corrupt_bundle', 'Disk bundle shorter than minimum header');
  }
  const magic = new TextDecoder().decode(bundle.subarray(0, 8));
  if (magic !== DISK_BUNDLE_MAGIC) {
    throw new StorageValidationError('corrupt_bundle', 'Invalid .wasmnux-disk magic header');
  }

  const dv = new DataView(bundle.buffer, bundle.byteOffset, bundle.byteLength);
  const payloadEnd = bundle.byteLength - 4;
  const expectedBundleCrc = dv.getUint32(payloadEnd, true);
  const actualBundleCrc = crc32(bundle.subarray(0, payloadEnd));
  if (actualBundleCrc !== expectedBundleCrc) {
    throw new StorageValidationError(
      'checksum_mismatch',
      `Bundle CRC32 mismatch (expected 0x${expectedBundleCrc.toString(16)}, got 0x${actualBundleCrc.toString(16)})`,
    );
  }

  const version = dv.getUint32(8, true);
  if (version !== DISK_BUNDLE_VERSION) {
    throw new StorageValidationError(
      'corrupt_bundle',
      `Unsupported disk bundle schema version ${String(version)}`,
    );
  }

  const idLen = dv.getUint16(12, true);
  let cur = 14;
  if (cur + idLen + 12 > payloadEnd) {
    throw new StorageValidationError('corrupt_bundle', 'Truncated bundle profile header');
  }

  let profileId: string;
  try {
    profileId = utf8Decoder.decode(bundle.subarray(cur, cur + idLen));
  } catch {
    throw new StorageValidationError('corrupt_bundle', 'Non-UTF8 profile identifier in bundle');
  }
  validateStorageIdentifier(profileId);
  cur += idLen;

  if (expectedProfileId !== undefined && profileId !== expectedProfileId) {
    throw new StorageValidationError(
      'profile_mismatch',
      `Bundle profile '${profileId}' does not match target profile '${expectedProfileId}'`,
    );
  }

  const capacityBytes = Number(dv.getBigUint64(cur, true));
  cur += 8;
  if (
    capacityBytes <= 0 ||
    capacityBytes > 2 * 1024 * 1024 * 1024 ||
    capacityBytes % DISK_CHUNK_BYTES !== 0
  ) {
    throw new StorageValidationError('corrupt_bundle', 'Invalid disk capacity in bundle header');
  }
  const totalChunks = capacityBytes / DISK_CHUNK_BYTES;

  const chunkCount = dv.getUint32(cur, true);
  cur += 4;
  const expectedRemaining = chunkCount * (8 + DISK_CHUNK_BYTES);
  if (cur + expectedRemaining !== payloadEnd) {
    throw new StorageValidationError('corrupt_bundle', 'Bundle payload length mismatch');
  }

  const chunks: Array<{ chunkIndex: number; crc32: number; data: Uint8Array }> = [];
  for (let i = 0; i < chunkCount; i += 1) {
    const chunkIndex = dv.getUint32(cur, true);
    cur += 4;
    const expectedChunkCrc = dv.getUint32(cur, true);
    cur += 4;
    if (chunkIndex >= totalChunks) {
      throw new StorageValidationError('corrupt_bundle', 'Chunk index out of disk capacity bounds');
    }
    const data = bundle.slice(cur, cur + DISK_CHUNK_BYTES);
    cur += DISK_CHUNK_BYTES;
    const actualChunkCrc = crc32(data);
    if (actualChunkCrc !== expectedChunkCrc) {
      throw new StorageValidationError('checksum_mismatch', 'Corrupted chunk CRC32 inside bundle');
    }
    chunks.push({ chunkIndex, crc32: actualChunkCrc, data });
  }

  return { profileId, capacityBytes, chunks };
}
