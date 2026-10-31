import { expect, it } from 'vitest';
import { validateDiskImage, DISK_BYTES } from '../src/storage-admin';
it('rejects wrong-size and non-ext images', () => {
  expect(() => validateDiskImage('vanilla', new Uint8Array(1024))).toThrow();
  const b = new Uint8Array(DISK_BYTES.vanilla);
  expect(() => validateDiskImage('vanilla', b)).toThrow('superblock');
  b[1080] = 0x53;
  b[1081] = 0xef;
  expect(() => validateDiskImage('vanilla', b)).not.toThrow();
  expect(() => validateDiskImage('debian', b)).toThrow();
});
