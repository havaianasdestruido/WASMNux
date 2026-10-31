import { expect, test } from "@playwright/test";

test("clearly labels unavailable profiles, volatile data and missing artifacts", async ({
  page,
}) => {
  await page.route("**/riscv_emu_rust_wasm_bg.wasm", (route) => route.abort());
  await page.goto("/");
  await expect(
    page.getByText("Temporary diagnostic, not a persistent Linux environment."),
  ).toBeVisible();
  await expect(page.getByText("Vanilla Linux", { exact: true })).toBeVisible();
  await expect(page.getByText("Debian", { exact: true })).toBeVisible();
  await expect(page.getByText("PENDING")).toHaveCount(2);
  await page.getByRole("button", { name: "Boot diagnostic" }).click();
  await expect(page.locator("#vm-status")).toContainText("Error", { timeout: 20_000 });
  await expect(page.getByRole("button", { name: "Boot diagnostic" })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Stop" })).toBeDisabled();
});

test("boots the actual diagnostic guest, accepts terminal input and discards on stop", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await page.getByRole("button", { name: "Boot diagnostic" }).click();
  await expect(page.locator("#vm-status")).toContainText("Running", { timeout: 30_000 });
  const rows = page.locator(".xterm-rows");
  await expect(rows).toContainText("Please press Enter to activate this console", {
    timeout: 90_000,
  });
  await page.locator(".xterm-helper-textarea").focus();
  await page.keyboard.press("Enter");
  await expect(rows).toContainText("/ #", { timeout: 20_000 });
  await page.keyboard.type("uname -s; printf 'UI_%s\\n' VERIFIED");
  await page.keyboard.press("Enter");
  await expect(rows).toContainText("UI_VERIFIED", { timeout: 30_000 });
  await expect(rows).toContainText("Linux");
  await page.getByRole("button", { name: "Stop", exact: true }).click();
  await expect(page.locator("#vm-status")).toContainText("Stopped");
  await expect(page.locator("#message")).toContainText("discarded");
  expect(errors).toEqual([]);
});

test("inspects the real block store without claiming it is attached to the guest", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: /Inspect prototype block store/ }).click();
  await expect(page.locator("#block-status")).toHaveText(
    "No profile disks provisioned. Not connected to a guest.",
  );
});

test("reports an unavailable block store rather than claiming persistence", async ({ page }) => {
  await page.addInitScript(() =>
    Object.defineProperty(globalThis, "indexedDB", { value: undefined }),
  );
  await page.goto("/");
  await page.getByRole("button", { name: /Inspect prototype block store/ }).click();
  await expect(page.locator("#block-status")).toHaveText(
    "Prototype block store unavailable or corrupt. Not connected to a guest.",
  );
});

test("display check renders a clearly labeled synthetic pattern with fallback", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: /Run display check/ }).click();
  await expect(page.locator("#display-result")).toHaveText("Canvas 2D fallback active");
  await expect(page.getByText("Synthetic test pattern; not guest output.")).toBeVisible();
  await expect(page.locator("#framebuffer-host canvas")).toHaveCount(1);
});

test("a stale display failure cannot dispose a newer renderer", async ({ page }) => {
  await page.addInitScript(() => {
    let rejectFirst: ((reason: Error) => void) | undefined;
    let requests = 0;
    let failNext2d = false;
    Object.defineProperty(navigator, "gpu", {
      configurable: true,
      value: {
        requestAdapter() {
          requests++;
          if (requests === 1) {
            return new Promise((_resolve, reject) => {
              rejectFirst = reject;
            });
          }
          return Promise.resolve(null);
        },
      },
    });
    const original = HTMLCanvasElement.prototype.getContext;
    const prototype = HTMLCanvasElement.prototype as unknown as {
      getContext: (this: HTMLCanvasElement, ...args: unknown[]) => unknown;
    };
    prototype.getContext = function (...args: unknown[]) {
      if (failNext2d && args[0] === "2d") {
        failNext2d = false;
        return null;
      }
      return Reflect.apply(original, this, args);
    };
    Object.assign(window, {
      rejectStaleDisplay() {
        failNext2d = true;
        rejectFirst?.(new Error("Delayed adapter failure"));
      },
    });
  });
  await page.goto("/");
  await page.getByRole("button", { name: /Run display check/ }).click();
  await expect(page.locator("#display-result")).toHaveText("Initializing renderer…");
  // A BFCache suspension invalidates the pending attempt and permits a new one.
  await page.evaluate(() =>
    window.dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true })),
  );
  await page.getByRole("button", { name: /Run display check/ }).click();
  await expect(page.locator("#display-result")).toHaveText("Canvas 2D fallback active");
  await page.evaluate(() =>
    (window as unknown as { rejectStaleDisplay: () => void }).rejectStaleDisplay(),
  );
  await expect(page.locator("#framebuffer-host canvas")).toHaveCount(1);
  await expect(page.locator("#display-result")).toHaveText("Canvas 2D fallback active");
});

test("reports an actual guest trap and permits a fresh diagnostic", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(page.locator(".warning")).toContainText(
    "Some BusyBox commands can stop the emulator",
  );
  await page.getByRole("button", { name: "Boot diagnostic" }).click();
  const rows = page.locator(".xterm-rows");
  await expect(rows).toContainText("Please press Enter to activate this console", {
    timeout: 90_000,
  });
  await page.locator(".xterm-helper-textarea").focus();
  await page.keyboard.press("Enter");
  await expect(rows).toContainText("/ #", { timeout: 20_000 });
  await page.keyboard.type("awk 'BEGIN {print 42}'");
  await page.keyboard.press("Enter");
  await expect(page.locator("#vm-status")).toContainText("Error", { timeout: 30_000 });
  await expect(page.locator("#message")).toContainText("Emulator trapped: unreachable");
  await expect(page.getByRole("button", { name: "Boot diagnostic" })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toBeDisabled();
  expect(errors).toEqual([]);
});

test("reports unavailable APIs and a denied persistence request", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "gpu", { configurable: true, value: undefined });
    Object.defineProperty(navigator, "storage", {
      configurable: true,
      value: { persisted: async () => false, persist: async () => false },
    });
  });
  await page.goto("/");
  await expect(page.locator("#cap-gpu")).toHaveText("Unavailable");
  await expect(page.locator("#cap-opfs")).toHaveText("Unavailable");
  await expect(page.locator("#storage-status")).toHaveText("Best effort · eviction possible");
  await page.getByRole("button", { name: /Request persistent storage/ }).click();
  await expect(page.locator("#storage-status")).toHaveText("Request denied · eviction possible");
});
