import { expect, test } from "@playwright/test";

interface Hooks {
  storeKind: string;
  wasmStatus(): string;
  wasmReason(): string | undefined;
  profileAssetKind(profile: string): string;
  writeDiskMarker(profile: string, text: string): Promise<void>;
  readDiskMarker(profile: string): Promise<string>;
}

async function hooks(page: import("@playwright/test").Page): Promise<Hooks> {
  await page.waitForFunction(
    () => (window as unknown as Record<string, unknown>)["__wasmnux"] !== undefined,
    undefined,
    { timeout: 15_000 },
  );
  return page.evaluate(() => {
    const w = window as unknown as Record<string, unknown>;
    return w["__wasmnux"] as Hooks;
  });
}

/** Waits until guest-asset probing settles for a profile. */
async function assetKind(
  page: import("@playwright/test").Page,
  profile: string,
): Promise<string> {
  const h = await hooks(page);
  let kind = h.profileAssetKind(profile);
  await expect
    .poll(
      async () => {
        const current = await page.evaluate(
          (p) =>
            ((window as unknown as Record<string, unknown>)["__wasmnux"] as Hooks).profileAssetKind(p),
          profile,
        );
        kind = current;
        return current;
      },
      { timeout: 15_000 },
    )
    .not.toBe("unknown");
  return kind;
}

test.describe("WASMNux browser app", () => {
  test("loads, detects capabilities and instantiates the WASM core", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("h1")).toHaveText("WASMNux");
    await expect(page.locator("#capabilities")).toContainText("WebAssembly");
    await expect(page.locator("#capabilities")).toContainText("OPFS");
    // The Rust core must actually instantiate (the CI e2e job places the
    // wasm-bindgen artifact under public/wasm before running this suite).
    {
      const h = await hooks(page);
      await expect
        .poll(async () => h.wasmStatus(), { timeout: 20_000 })
        .not.toBe("loading");
      if (h.wasmStatus() !== "ready") {
        throw new Error(`WASM core unavailable in e2e: ${h.wasmReason() ?? "(no reason)"}`);
      }
    }
    const h = await hooks(page);
    expect(["opfs", "indexeddb"]).toContain(h.storeKind);
  });

  test("refuses to fake a boot when guest assets are absent", async ({ page }) => {
    await page.goto("/");
    test.skip((await assetKind(page, "vanilla")) === "ready", "assets present; covered by the gated boot test");
    await expect(page.locator("#btn-boot")).toBeDisabled();
    await expect(page.locator("#boot-reasons")).toContainText("guest/scripts");
    await expect(page.locator("#asset-detail")).toContainText("manifest.json");
  });

  test("disk data survives a full page reload (real OPFS/IndexedDB persistence)", async ({ page }) => {
    await page.goto("/");
    const marker = `e2e-persist-${Date.now()}`;
    await page.evaluate(async (m) => {
      const h = (window as unknown as Record<string, unknown>)["__wasmnux"] as Hooks;
      await h.writeDiskMarker("vanilla", m);
    }, marker);
    await page.reload({ waitUntil: "networkidle" });
    const after = await page.evaluate(async () => {
      const h = (window as unknown as Record<string, unknown>)["__wasmnux"] as Hooks;
      return h.readDiskMarker("vanilla");
    });
    expect(after).toBe(marker);
  });

  test("snapshots restore and profiles stay isolated across reload + reset", async ({ page }) => {
    await page.goto("/");
    await page.evaluate(async () => {
      const h = (window as unknown as Record<string, unknown>)["__wasmnux"] as Hooks;
      await h.writeDiskMarker("vanilla", "vanilla-v1");
      await h.writeDiskMarker("Debian", "Debian-v1");
    });
    await page.fill("#snapshot-name", "e2e-snap");
    await page.click("#btn-snapshot");
    await expect(page.locator("#snapshot-list option")).toHaveCount(1);

    await page.evaluate(async () => {
      const h = (window as unknown as Record<string, unknown>)["__wasmnux"] as Hooks;
      await h.writeDiskMarker("vanilla", "vanilla-v2-messy");
    });
    await page.click("#btn-restore");
    const restored = await page.evaluate(async () => {
      const h = (window as unknown as Record<string, unknown>)["__wasmnux"] as Hooks;
      return Promise.all([h.readDiskMarker("vanilla"), h.readDiskMarker("Debian")]);
    });
    expect(restored).toEqual(["vanilla-v1", "Debian-v1"]);

    // Reset only Vanilla; Debian is untouched, and both survive a reload.
    page.on("dialog", (dialog) => void dialog.accept());
    await page.click("#btn-reset-disk");
    await page.reload({ waitUntil: "networkidle" });
    const afterReset = await page.evaluate(async () => {
      const h = (window as unknown as Record<string, unknown>)["__wasmnux"] as Hooks;
      return Promise.all([h.readDiskMarker("vanilla"), h.readDiskMarker("Debian")]);
    });
    expect(afterReset[0]).toBe("");
    expect(afterReset[1]).toBe("Debian-v1");
  });

  test("renderer self-test visibly changes the framebuffer canvas", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("#renderer-badge")).toContainText("renderer:");
    const canvas = page.locator("#framebuffer");
    const before = await canvas.screenshot();
    await page.click("#btn-test-pattern");
    await expect(page.locator("#framebuffer-status")).toContainText("NOT guest output");
    const after = await canvas.screenshot();
    expect(after.equals(before)).toBe(false);
    expect(after.length).toBeGreaterThan(2_000);
  });
});

/**
 * The full guest-boot workflow (Mission §8I). It runs only when real guest
 * assets are present under public/guest (E2E_GUEST=1 in the guest CI job);
 * elsewhere it skips loudly. It is never weakened to pass without them.
 */
test.describe("guest boot (gated on real guest assets)", () => {
  test.skip(process.env["E2E_GUEST"] !== "1", "set E2E_GUEST=1 with built guest assets present");

  test("Vanilla Linux boots to a shell, persists a file across reboot", async ({ page }) => {
    test.setTimeout(600_000);
    await page.goto("/");
    await expect(page.locator("#btn-boot")).toBeEnabled();
    await page.click("#btn-boot");
    const terminal = page.locator("#terminal");
    await expect(terminal).toContainText("Linux version", { timeout: 300_000 });
    await expect(terminal).toContainText("WASMNux Vanilla Linux", { timeout: 300_000 });

    await terminal.click();
    await page.keyboard.type("echo e2e-was-here > /persist/e2e.txt && sync && cat /persist/e2e.txt\n");
    await expect(terminal).toContainText("e2e-was-here", { timeout: 30_000 });

    await page.keyboard.type("uname -a\n");
    await expect(terminal).toContainText("riscv64", { timeout: 30_000 });

    await page.keyboard.type("poweroff\n");
    await expect(page.locator("#vm-state")).toHaveText("halted", { timeout: 120_000 });

    await page.click("#btn-boot");
    await expect(terminal).toContainText("WASMNux Vanilla Linux", { timeout: 300_000 });
    await terminal.click();
    await page.keyboard.type("cat /persist/e2e.txt\n");
    await expect(terminal).toContainText("e2e-was-here", { timeout: 30_000 });
  });
});
