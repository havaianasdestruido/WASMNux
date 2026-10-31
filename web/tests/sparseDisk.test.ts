import { describe, expect, it } from "vitest";

import { MemoryChunkStore } from "../src/storage/chunkStore.js";
import { CHUNK_SIZE, DiskCorruptionError, DiskError, SparseDisk } from "../src/storage/sparseDisk.js";

const MiB = 1024 * 1024;

function fill(value: number, length: number): Uint8Array {
  return new Uint8Array(length).fill(value);
}

describe("SparseDisk", () => {
  it("creates, writes across a chunk boundary, flushes and reopens", async () => {
    const store = new MemoryChunkStore();
    const disk = await SparseDisk.open(store, "t1", { capacityBytes: 4 * MiB });
    const payload = new Uint8Array(1000).map((_, i) => i % 251);
    await disk.write(CHUNK_SIZE - 500, payload);
    await disk.flush();

    const reopened = await SparseDisk.open(store, "t1");
    expect(reopened.capacityBytes).toBe(4 * MiB);
    expect(await reopened.read(CHUNK_SIZE - 500, 1000)).toEqual(payload);
    // Holes read as zeroes.
    expect(await reopened.read(3 * CHUNK_SIZE, 64)).toEqual(fill(0, 64));
  });

  it("does not expose unflushed writes to a fresh open (crash consistency)", async () => {
    const store = new MemoryChunkStore();
    const disk = await SparseDisk.open(store, "t2", { capacityBytes: 2 * MiB });
    await disk.write(0, fill(7, 512));
    await disk.flush();
    await disk.write(0, fill(9, 512)); // never flushed: tab "crashes" here

    const reopened = await SparseDisk.open(store, "t2");
    expect(await reopened.read(0, 512)).toEqual(fill(7, 512));
  });

  it("discardPending reverts the live view to the flushed state", async () => {
    const store = new MemoryChunkStore();
    const disk = await SparseDisk.open(store, "t3", { capacityBytes: 2 * MiB });
    await disk.write(512, fill(1, 512));
    await disk.flush();
    await disk.write(512, fill(2, 512));
    disk.discardPending();
    expect(await disk.read(512, 512)).toEqual(fill(1, 512));
    expect(disk.dirty).toBe(false);
  });

  it("snapshots are copy-on-write and restorable; profiles stay isolated", async () => {
    const store = new MemoryChunkStore();
    const vanilla = await SparseDisk.open(store, "profiles-vanilla", { capacityBytes: 2 * MiB });
    const Debian = await SparseDisk.open(store, "profiles-Debian", { capacityBytes: 2 * MiB });

    await vanilla.write(0, fill(0xaa, 4096));
    await vanilla.flush();
    await vanilla.snapshot("before-mess");
    await vanilla.write(0, fill(0xbb, 4096));
    await vanilla.flush();
    expect(await vanilla.read(0, 16)).toEqual(fill(0xbb, 16));

    await vanilla.restoreSnapshot("before-mess");
    expect(await vanilla.read(0, 16)).toEqual(fill(0xaa, 16));
    expect(await vanilla.listSnapshots()).toEqual(["before-mess"]);

    // The other profile was never touched.
    expect(await Debian.read(0, 4096)).toEqual(fill(0, 4096));
    expect(await Debian.listSnapshots()).toEqual([]);
    await expect(vanilla.snapshot("before-mess")).rejects.toBeInstanceOf(DiskError);
    await expect(vanilla.snapshot("../evil")).rejects.toBeInstanceOf(DiskError);
  });

  it("export/import round-trips a whole image and dedups unchanged chunks", async () => {
    const store = new MemoryChunkStore();
    const disk = await SparseDisk.open(store, "t4", { capacityBytes: 1 * MiB });
    const image = new Uint8Array(1 * MiB);
    image.set(fill(0x5a, CHUNK_SIZE), 0);
    image.set(fill(0x33, 8192), 2 * CHUNK_SIZE);
    await disk.importImage(image);
    expect(await disk.exportImage()).toEqual(image);

    const keysBefore = store.size;
    await disk.importImage(image); // identical: only a new manifest is written
    expect(store.size).toBe(keysBefore);
    await expect(disk.importImage(new Uint8Array(1234))).rejects.toBeInstanceOf(DiskError);
  });

  it("rejects out-of-range IO and non-sector capacities", async () => {
    const store = new MemoryChunkStore();
    await expect(SparseDisk.open(store, "bad", { capacityBytes: 1000 })).rejects.toBeInstanceOf(
      DiskError,
    );
    const disk = await SparseDisk.open(store, "t5", { capacityBytes: 512 * 4 });
    await expect(disk.write(512 * 4 - 100, fill(1, 200))).rejects.toBeInstanceOf(DiskError);
    await expect(disk.read(-1, 10)).rejects.toBeInstanceOf(DiskError);
    await expect(SparseDisk.open(store, "never-created")).rejects.toBeInstanceOf(DiskError);
  });

  it("detects chunk corruption by CRC instead of returning bad data", async () => {
    const store = new MemoryChunkStore();
    const disk = await SparseDisk.open(store, "t6", { capacityBytes: 2 * MiB });
    await disk.write(0, fill(0x11, 1024));
    await disk.flush();

    // Tamper with the stored chunk object behind the disk's back.
    const keys = await store.list("disks/t6/chunks/");
    expect(keys.length).toBe(1);
    const key = keys[0] as string;
    const original = await store.get(key);
    const tampered = new Uint8Array(original as Uint8Array);
    tampered[0] = 0xee;
    store.rawSet(key, tampered);

    const reopened = await SparseDisk.open(store, "t6");
    await expect(reopened.read(0, 1024)).rejects.toBeInstanceOf(DiskCorruptionError);
  });

  it("writing zeroes deallocates the chunk (sparse stays sparse)", async () => {
    const store = new MemoryChunkStore();
    const disk = await SparseDisk.open(store, "t7", { capacityBytes: 4 * MiB });
    await disk.write(0, fill(0xff, CHUNK_SIZE));
    await disk.flush();
    expect(disk.usage().allocatedChunks).toBe(1);
    await disk.write(0, fill(0, CHUNK_SIZE));
    await disk.flush();
    expect(disk.usage().allocatedChunks).toBe(0);
  });

  it("destroy removes manifest and chunks", async () => {
    const store = new MemoryChunkStore();
    const disk = await SparseDisk.open(store, "t8", { capacityBytes: 2 * MiB });
    await disk.write(0, fill(3, 2048));
    await disk.flush();
    await disk.destroy();
    expect(await store.list("disks/t8/")).toEqual([]);
    await expect(SparseDisk.open(store, "t8")).rejects.toBeInstanceOf(DiskError);
  });

  it("serialises overlapping operations (regression: reset racing reads tore state)", async () => {
    const store = new MemoryChunkStore();
    const disk = await SparseDisk.open(store, "t-serial", { capacityBytes: 2 * MiB });
    const payload = new TextEncoder().encode("concurrent-marker");
    await disk.write(0, payload);
    await disk.flush();

    // Fire a destroy and several reads/writes without awaiting in between.
    // With serialisation the calls run one-at-a-time in issue order, so the
    // first read sees the marker, later reads see an empty disk, and no
    // operation ever throws a corruption/parse error.
    const results = await Promise.allSettled([
      disk.read(0, payload.length),
      disk.destroy(),
      disk.read(0, payload.length),
      disk.write(0, payload),
      disk.flush(),
      disk.read(0, payload.length),
    ]);
    for (const r of results) {
      expect(r.status).toBe("fulfilled");
    }
    const first = results[0];
    const afterDestroy = results[2];
    const last = results[5];
    if (first.status === "fulfilled") {
      expect(new TextDecoder().decode(first.value)).toBe("concurrent-marker");
    }
    if (afterDestroy.status === "fulfilled") {
      expect(afterDestroy.value.every((b) => b === 0)).toBe(true);
    }
    if (last.status === "fulfilled") {
      expect(new TextDecoder().decode(last.value)).toBe("concurrent-marker");
    }
  });
});
