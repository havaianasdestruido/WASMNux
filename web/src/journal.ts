/** Synchronous storage adapter: implementations must flush before returning. */
export interface SyncFile {
  read(offset: number, length: number): Uint8Array;
  write(offset: number, bytes: Uint8Array): void;
  truncate(length: number): void;
  size(): number;
  flush(): void;
}
export function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}
const MAGIC = 0x574e584a;
export class JournaledDisk {
  constructor(
    private disk: SyncFile,
    private journal: SyncFile,
    readonly sizeBytes: number,
  ) {
    if (!Number.isSafeInteger(sizeBytes) || sizeBytes <= 0 || disk.size() !== sizeBytes)
      throw new Error('Disk size mismatch');
    this.recover();
  }
  private recover(): void {
    const n = this.journal.size();
    if (n === 0) return;
    if (n < 32 || n > 16 * 1024 * 1024 + 32) throw new Error('Corrupt journal length; restore a backup');
    const record = this.journal.read(0, n);
    const v = new DataView(record.buffer, record.byteOffset, record.byteLength);
    const offset = v.getFloat64(8, true),
      len = v.getUint32(16, true),
      crc = v.getUint32(20, true);
    const committed = v.getUint32(24, true) === MAGIC;
    if (
      v.getUint32(0, true) !== MAGIC ||
      !Number.isSafeInteger(offset) ||
      offset < 0 ||
      len !== n - 32 ||
      offset + len > this.sizeBytes
    )
      throw new Error('Corrupt journal header; restore a backup');
    if (committed && crc32(record.subarray(32)) !== crc)
      throw new Error('Journal checksum mismatch; restore a backup');
    if (committed) {
      this.disk.write(offset, record.subarray(32));
      this.disk.flush();
    }
    this.journal.truncate(0);
    this.journal.flush();
  }
  read(offset: number, length: number): Uint8Array {
    if (
      !Number.isSafeInteger(offset) ||
      !Number.isSafeInteger(length) ||
      offset < 0 ||
      length < 0 ||
      offset + length > this.sizeBytes
    )
      throw new Error('Disk read out of bounds');
    return this.disk.read(offset, length);
  }
  write(offset: number, bytes: Uint8Array): void {
    if (
      !Number.isSafeInteger(offset) ||
      offset < 0 ||
      bytes.length > 16 * 1024 * 1024 ||
      offset + bytes.length > this.sizeBytes
    )
      throw new Error('Disk write out of bounds');
    if (bytes.length === 0) return;
    const record = new Uint8Array(32 + bytes.length),
      v = new DataView(record.buffer);
    v.setUint32(0, MAGIC, true);
    v.setFloat64(8, offset, true);
    v.setUint32(16, bytes.length, true);
    v.setUint32(20, crc32(bytes), true);
    record.set(bytes, 32);
    this.journal.truncate(0);
    this.journal.write(0, record);
    v.setUint32(24, MAGIC, true);
    this.journal.write(24, record.subarray(24, 28));
    this.journal.flush();
    this.disk.write(offset, bytes);
    this.disk.flush();
    this.journal.truncate(0);
    this.journal.flush();
  }
  flush(): void {
    this.disk.flush();
  }
  close(): void {
    this.flush();
  }
}
