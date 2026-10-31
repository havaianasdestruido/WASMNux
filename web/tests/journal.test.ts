import { describe, expect, it } from 'vitest';
import { crc32, JournaledDisk, type SyncFile } from '../src/journal';
class File implements SyncFile {
  data = new Uint8Array(4096);
  fail = false;
  read(o: number, n: number) {
    return this.data.slice(o, o + n);
  }
  write(o: number, b: Uint8Array) {
    if (this.fail) throw new Error('quota');
    if (o + b.length > this.data.length) {
      const grown = new Uint8Array(o + b.length);
      grown.set(this.data);
      this.data = grown;
    }
    this.data.set(b, o);
  }
  truncate(n: number) {
    const d = new Uint8Array(n);
    d.set(this.data.subarray(0, n));
    this.data = d;
  }
  size() {
    return this.data.length;
  }
  flush() {}
}
function emptyJournal(): File {
  const f = new File();
  f.truncate(0);
  return f;
}
describe('journaled disk', () => {
  it('round-trips and bounds checks', () => {
    const d = new JournaledDisk(new File(), emptyJournal(), 4096);
    d.write(8, new Uint8Array([1, 2, 3]));
    expect([...d.read(8, 3)]).toEqual([1, 2, 3]);
    expect(() => d.read(4095, 2)).toThrow();
    expect(() => d.write(-1, new Uint8Array(1))).toThrow();
  });
  it('recovers a committed write after crash', () => {
    const disk = new File(),
      journal = emptyJournal();
    new JournaledDisk(disk, journal, 4096);
    journal.data = new Uint8Array(4096);
    const rec = new Uint8Array(35);
    const v = new DataView(rec.buffer);
    v.setUint32(0, 0x574e584a, true);
    v.setFloat64(8, 16, true);
    v.setUint32(16, 3, true);
    v.setUint32(20, crc32(new Uint8Array([7, 8, 9])), true);
    v.setUint32(24, 0x574e584a, true);
    rec.set([7, 8, 9], 32);
    journal.write(0, rec);
    journal.truncate(35);
    const recovered = new JournaledDisk(disk, journal, 4096);
    expect([...recovered.read(16, 3)]).toEqual([7, 8, 9]);
    expect(journal.size()).toBe(0);
  });
  it('rejects a corrupt committed journal without touching disk', () => {
    const disk = new File(),
      journal = emptyJournal();
    journal.data = new Uint8Array(35);
    const v = new DataView(journal.data.buffer);
    v.setUint32(0, 0x574e584a, true);
    v.setUint32(16, 3, true);
    v.setUint32(24, 0x574e584a, true);
    expect(() => new JournaledDisk(disk, journal, 4096)).toThrow('checksum');
    expect(disk.data.every((x) => x === 0)).toBe(true);
  });
  it('propagates quota failures and does not acknowledge them', () => {
    const disk = new File(),
      journal = emptyJournal();
    const d = new JournaledDisk(disk, journal, 4096);
    disk.fail = true;
    expect(() => d.write(0, new Uint8Array([1]))).toThrow('quota');
  });
});
