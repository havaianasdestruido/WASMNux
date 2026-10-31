import { chromium, expect, test } from "@playwright/test";

const initial = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
const resized = [5, 20, 70, 255, 200, 90, 10, 255];

test("Canvas 2D fallback renders, resizes, validates and cleans up", async ({ page }) => {
  await page.goto("/tests/browser/framebuffer-harness.html");
  expect(await page.evaluate(() => window.frameTest.start(true))).toBe("canvas2d");
  await page.evaluate((bytes) => window.frameTest.draw(2, 2, bytes), initial);
  expect(await page.evaluate(() => window.frameTest.pixels())).toEqual(initial);
  await page.evaluate((bytes) => window.frameTest.draw(1, 2, bytes), resized);
  expect(await page.evaluate(() => window.frameTest.pixels())).toEqual(resized);
  const error = await page.evaluate(async () => {
    try {
      await window.frameTest.draw(2, 2, [0]);
      return "unexpected success";
    } catch (error) {
      return error instanceof Error ? error.message : "unknown";
    }
  });
  expect(error).toContain("Invalid RGBA");
  await page.evaluate(() => window.frameTest.dispose());
  expect(await page.evaluate(() => window.frameTest.canvasCount())).toBe(0);
});

test("real WebGPU shader renders texture updates, resizes and recovers from device loss", async () => {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
      : {}),
    args: [
      "--no-sandbox",
      "--enable-unsafe-webgpu",
      "--use-angle=swiftshader",
      "--enable-unsafe-swiftshader",
    ],
  });
  try {
    const page = await browser.newPage();
    await page.goto("http://127.0.0.1:4173/tests/browser/framebuffer-harness.html");
    expect(await page.evaluate(() => window.frameTest.start(false, true))).toBe("webgpu");
    await page.evaluate((bytes) => window.frameTest.draw(2, 2, bytes), initial);
    expect(await page.evaluate(() => window.frameTest.pixels())).toEqual(initial);
    await page.evaluate((bytes) => window.frameTest.draw(1, 2, bytes), resized);
    expect(await page.evaluate(() => window.frameTest.pixels())).toEqual(resized);
    await page.evaluate(() => window.frameTest.lose());
    await expect.poll(() => page.evaluate(() => window.frameTest.kind())).toBe("canvas2d");
    expect(await page.evaluate(() => window.frameTest.change())).toContain("device lost");
    expect(await page.evaluate(() => window.frameTest.pixels())).toEqual(resized);
    await page.evaluate(() => window.frameTest.dispose());
    expect(await page.evaluate(() => window.frameTest.canvasCount())).toBe(0);
  } finally {
    await browser.close();
  }
});
