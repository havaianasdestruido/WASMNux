import { expect, test } from "@playwright/test";
import { trapProgramCommand } from "./trap-fixture";

test("reports an unavailable guest artifact instead of pretending to boot", async ({ page }) => {
  await page.route("**/fw_payload.elf", (route) => route.abort());
  await page.goto("/tests/browser/worker-harness.html");
  await page.getByRole("button", { name: "Start" }).click();
  await expect(page.locator("#status")).toContainText("error:", { timeout: 20_000 });
  await expect(page.locator("#output")).not.toContainText("Linux version");
});

test("boots a historical real Linux guest in Rust/Wasm and executes a real command", async ({
  page,
}) => {
  await page.goto("/tests/browser/worker-harness.html");
  await page.getByRole("button", { name: "Start" }).click();
  await expect(page.locator("#status")).toHaveText("running", { timeout: 30_000 });
  const output = page.locator("#output");
  await expect(output).toContainText("Please press Enter to activate this console", {
    timeout: 90_000,
  });
  await expect(output).toContainText("Linux version 5.4");
  await expect(output).toContainText("VFS: Mounted root");
  await page.getByLabel("Serial input", { exact: true }).fill("\\n");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(output).toContainText("/ #", { timeout: 20_000 });
  await page
    .getByLabel("Serial input", { exact: true })
    .fill("uname -s; printf 'WASMNUX_%s\\n' PROBE_OK\\n");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(output).toContainText("WASMNUX_PROBE_OK", { timeout: 30_000 });
  await expect(output).toContainText("Linux\r\nWASMNUX_PROBE_OK");
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(page.locator("#status")).toHaveText("stopped");
});

test("reports a real emulator trap even when cleanup fails", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/tests/browser/worker-harness.html");
  await page.getByRole("button", { name: "Start" }).click();
  const output = page.locator("#output");
  await expect(output).toContainText("Please press Enter to activate this console", {
    timeout: 90_000,
  });
  await page.getByLabel("Serial input", { exact: true }).fill("\\n");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(output).toContainText("/ #", { timeout: 20_000 });
  // A real executable with a deliberately invalid instruction exercises the
  // unimplemented decoder path without relying on a formerly broken applet.
  await page.getByLabel("Raw serial input").fill(`${trapProgramCommand()}\n`);
  await page.getByRole("button", { name: "Send raw" }).click();
  await expect(page.locator("#status")).toHaveText(
    "error: Emulator trapped: unreachable at guest PC 0x10078. Diagnostic guest cannot continue.",
    { timeout: 30_000 },
  );
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(page.locator("#status")).toHaveText("stopped");
  expect(errors).toEqual([]);
});

test("pauses serial output until the consumer acknowledges it", async ({ page }) => {
  await page.goto("/tests/browser/worker-harness.html");
  await page.getByLabel("Acknowledge output").uncheck();
  await page.getByRole("button", { name: "Start" }).click();
  await expect(page.locator("#status")).toHaveText("running", { timeout: 30_000 });
  const output = page.locator("#output");
  await expect.poll(async () => (await output.textContent())?.length ?? 0).toBeGreaterThan(0);
  const first = await output.textContent();
  await page.waitForTimeout(600);
  expect(await output.textContent()).toBe(first);
  await page.getByLabel("Acknowledge output").check();
  await page.getByRole("button", { name: "Release output" }).click();
  await expect
    .poll(async () => (await output.textContent())?.length ?? 0)
    .toBeGreaterThan(first?.length ?? 0);
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(page.locator("#status")).toHaveText("stopped");
});
