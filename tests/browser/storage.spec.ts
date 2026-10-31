import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium, expect, test } from "@playwright/test";

test("real IndexedDB blocks, snapshots, validation and profile isolation", async ({
  page,
  context,
}) => {
  await page.goto("/tests/browser/storage-harness.html");
  const results = await page.evaluate(async () => {
    const d = window.diskTest;
    await d.open();
    await d.create("vanilla", 131072);
    await d.create("debian", 131072);
    const initiallyZero = (await d.read("vanilla", 0, 512)).every((byte) => byte === 0);
    await d.write("vanilla", 65024, Array(1024).fill(7));
    await d.write("debian", 0, Array(512).fill(9));
    const crossBlock = (await d.read("vanilla", 65024, 1024)).every((byte) => byte === 7);
    const snap = await d.snapshot("vanilla", "before change");
    await d.write("vanilla", 65024, Array(1024).fill(3));
    await d.restore("vanilla", snap.id);
    const restored = (await d.read("vanilla", 65024, 1024)).every((byte) => byte === 7);
    await d.reset("vanilla");
    const reset = (await d.read("vanilla", 65024, 1024)).every((byte) => byte === 0);
    const otherProfile = (await d.read("debian", 0, 512)).every((byte) => byte === 9);
    await d.restore("vanilla", snap.id);
    const snapshotSurvivesReset = (await d.read("vanilla", 65024, 1024)).every(
      (byte) => byte === 7,
    );
    const failures: string[] = [];
    for (const operation of [
      () => d.create("vanilla", 131072),
      () => d.create("debian", 513),
      () => d.read("vanilla", -512, 512),
      () => d.read("vanilla", 0, 513),
      () => d.write("vanilla", 131072, Array(512).fill(1)),
      () => d.restore("vanilla", "../../bad"),
      () => d.restore("debian", snap.id),
    ]) {
      try {
        await operation();
        failures.push("unexpected success");
      } catch (error) {
        failures.push(error instanceof Error ? error.message : "unknown error");
      }
    }
    return {
      initiallyZero,
      crossBlock,
      restored,
      reset,
      otherProfile,
      snapshotSurvivesReset,
      failures,
    };
  });
  expect(results).toMatchObject({
    initiallyZero: true,
    crossBlock: true,
    restored: true,
    reset: true,
    otherProfile: true,
    snapshotSurvivesReset: true,
  });
  expect(results.failures).toHaveLength(7);
  expect(results.failures).not.toContain("unexpected success");

  const second = await context.newPage();
  await second.goto("/tests/browser/storage-harness.html");
  await second.evaluate(() => window.diskTest.open());
  await Promise.all([
    page.evaluate(() => window.diskTest.write("debian", 0, Array(512).fill(11))),
    second.evaluate(() => window.diskTest.write("debian", 512, Array(512).fill(12))),
  ]);
  const both = await page.evaluate(() => window.diskTest.read("debian", 0, 1024));
  expect(both.slice(0, 512)).toEqual(Array(512).fill(11));
  expect(both.slice(512)).toEqual(Array(512).fill(12));
});

test("a corrupt snapshot aborts restoration without deleting committed blocks", async ({
  page,
}) => {
  await page.goto("/tests/browser/storage-harness.html");
  const result = await page.evaluate(async () => {
    const d = window.diskTest;
    await d.open();
    await d.create("vanilla", 131072);
    await d.write("vanilla", 0, Array(512).fill(1));
    const snapshot = await d.snapshot("vanilla", "to corrupt");
    await d.write("vanilla", 0, Array(512).fill(2));
    await d.corruptSnapshot("vanilla", snapshot.id);
    let errorMessage = "";
    try {
      await d.restore("vanilla", snapshot.id);
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : "unknown";
    }
    return { errorMessage, bytes: await d.read("vanilla", 0, 512), info: await d.info("vanilla") };
  });
  expect(result.errorMessage).toContain("corrupt");
  expect(result.bytes).toEqual(Array(512).fill(2));
  expect(result.info?.revision).toBe(2);
});

test("committed block bytes and snapshots survive a complete browser restart", async () => {
  const directory = await mkdtemp(join(tmpdir(), "wasmnux-browser-"));
  const options = {
    headless: true,
    args: ["--no-sandbox"],
    ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
      : {}),
  };
  try {
    const first = await chromium.launchPersistentContext(directory, options);
    try {
      const page = await first.newPage();
      await page.goto("http://127.0.0.1:4173/tests/browser/storage-harness.html");
      await page.evaluate(async () => {
        const d = window.diskTest;
        await d.open();
        await d.create("debian", 131072);
        await d.write("debian", 0, Array(512).fill(41));
        const snapshot = await d.snapshot("debian", "restart-check");
        await d.write("debian", 0, Array(512).fill(42));
        localStorage.setItem("test-snapshot", snapshot.id);
        d.close();
      });
    } finally {
      await first.close();
    }
    const second = await chromium.launchPersistentContext(directory, options);
    try {
      const page = await second.newPage();
      await page.goto("http://127.0.0.1:4173/tests/browser/storage-harness.html");
      const result = await page.evaluate(async () => {
        const d = window.diskTest;
        await d.open();
        const current = await d.read("debian", 0, 512);
        const id = localStorage.getItem("test-snapshot");
        if (!id) throw new Error("Missing snapshot id after browser restart");
        await d.restore("debian", id);
        return { current, restored: await d.read("debian", 0, 512) };
      });
      expect(result.current).toEqual(Array(512).fill(42));
      expect(result.restored).toEqual(Array(512).fill(41));
    } finally {
      await second.close();
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("rejects out-of-capacity and negative block keys without changing committed data", async ({
  page,
}) => {
  await page.goto("/tests/browser/storage-harness.html");
  const result = await page.evaluate(async () => {
    const d = window.diskTest;
    await d.open();
    await d.create("vanilla", 131072);
    await d.write("vanilla", 0, Array(512).fill(7));
    const failures: string[] = [];
    for (const index of [2, -1, "bad-index", [1]]) {
      await d.injectBlock("vanilla", index);
      try {
        await d.snapshot("vanilla", `invalid index ${index}`);
        failures.push("unexpected success");
      } catch (error) {
        failures.push(error instanceof Error ? error.message : "unknown");
      }
      await d.reset("vanilla");
    }
    await d.write("vanilla", 0, Array(512).fill(11));
    const snapshot = await d.snapshot("vanilla", "valid snapshot");
    const otherSnapshot = await d.snapshot("vanilla", "another valid snapshot");
    await d.write("vanilla", 0, Array(512).fill(12));
    for (const [id, index] of [
      [snapshot.id, 2],
      [otherSnapshot.id, [1]],
    ] as const) {
      await d.injectSnapshotBlock("vanilla", id, index);
      try {
        await d.restore("vanilla", id);
        failures.push("unexpected success");
      } catch (error) {
        failures.push(error instanceof Error ? error.message : "unknown");
      }
    }
    return { failures, bytes: await d.read("vanilla", 0, 512), info: await d.info("vanilla") };
  });
  expect(result.failures).toHaveLength(6);
  expect(result.failures).not.toContain("unexpected success");
  for (const failure of result.failures) expect(failure).toContain("corrupt");
  expect(result.bytes).toEqual(Array(512).fill(12));
  expect(result.info?.revision).toBe(7);
});

test("a real browser quota failure rolls back all blocks in a write", async ({ page, context }) => {
  await page.goto("/tests/browser/storage-harness.html");
  const client = await context.newCDPSession(page);
  // Chromium applies the override when the origin's bucket is first opened.
  await client.send("Storage.overrideQuotaForOrigin", {
    origin: "http://127.0.0.1:4173",
    quotaSize: 256 * 1024,
  });
  try {
    await page.evaluate(async () => {
      const d = window.diskTest;
      await d.open();
      await d.create("vanilla", 2 * 1024 * 1024);
      await d.write("vanilla", 0, Array(512).fill(29));
    });
    const result = await page.evaluate(async () => {
      const d = window.diskTest;
      let error = "";
      try {
        await d.writeRandom("vanilla", 0, 2 * 1024 * 1024);
      } catch (failure) {
        error = failure instanceof Error ? `${failure.name}: ${failure.message}` : "unknown";
      }
      return {
        error,
        first: await d.read("vanilla", 0, 512),
        second: await d.read("vanilla", 65536, 512),
        info: await d.info("vanilla"),
      };
    });
    expect(result.error).toMatch(/quota/i);
    expect(result.first).toEqual(Array(512).fill(29));
    expect(result.second).toEqual(Array(512).fill(0));
    expect(result.info?.revision).toBe(1);
  } finally {
    await client.send("Storage.overrideQuotaForOrigin", { origin: "http://127.0.0.1:4173" });
    await client.detach();
  }
});

test("upgrades the original IndexedDB schema without losing disk or snapshot blocks", async ({
  page,
}) => {
  await page.goto("/tests/browser/storage-harness.html");
  const result = await page.evaluate(async () => {
    const id = crypto.randomUUID();
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const opening = indexedDB.open("wasmnux-storage-test", 1);
      opening.onupgradeneeded = () => {
        const database = opening.result;
        database.createObjectStore("disks", { keyPath: "profile" });
        database.createObjectStore("blocks", { keyPath: ["profile", "index"] });
        database.createObjectStore("snapshots", { keyPath: ["profile", "id"] });
        database.createObjectStore("snapshotBlocks", { keyPath: ["profile", "id", "index"] });
      };
      opening.onsuccess = () => resolve(opening.result);
      opening.onerror = () => reject(opening.error);
    });
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(["disks", "blocks", "snapshots", "snapshotBlocks"], "readwrite");
      const current = new Uint8Array(65536);
      current.fill(31, 0, 512);
      const saved = new Uint8Array(65536);
      saved.fill(21, 0, 512);
      tx.objectStore("disks").put({ profile: "vanilla", capacity: 131072, revision: 1 });
      tx.objectStore("blocks").put({ profile: "vanilla", index: 0, data: current.buffer });
      tx.objectStore("snapshots").put({
        profile: "vanilla",
        id,
        name: "old schema",
        capacity: 131072,
        revision: 0,
        createdAt: Date.now(),
      });
      tx.objectStore("snapshotBlocks").put({
        profile: "vanilla",
        id,
        index: 0,
        data: saved.buffer,
      });
      tx.oncomplete = () => resolve();
      tx.onabort = () => reject(tx.error);
    });
    db.close();
    const d = window.diskTest;
    await d.open();
    const before = await d.read("vanilla", 0, 512);
    await d.restore("vanilla", id);
    return { before, after: await d.read("vanilla", 0, 512), info: await d.info("vanilla") };
  });
  expect(result.before).toEqual(Array(512).fill(31));
  expect(result.after).toEqual(Array(512).fill(21));
  expect(result.info?.revision).toBe(2);
});
