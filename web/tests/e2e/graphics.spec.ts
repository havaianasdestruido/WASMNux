import { test, expect } from '@playwright/test';
test('guest framebuffer is presented and non-empty', async ({ browser }) => {
  const context = await browser.newContext({ serviceWorkers: 'block' });
  const page = await context.newPage();
  await page.goto('/');
  await page.getByRole('button', { name: 'Boot', exact: true }).click();
  await expect(page.locator('#terminal')).toContainText('WASMNUX_PERSISTENT_ROOT_READY', { timeout: 280000 });
  await expect(page.locator('#graphics')).toContainText(/WebGPU|Canvas 2D/);
  await page.locator('#terminal').click();
  await page.keyboard.type('dd if=/dev/urandom of=/dev/fb0 bs=1024 count=256 2>/dev/null');
  await page.keyboard.press('Enter');
  await expect(page.locator('#display')).toHaveAttribute('data-frame-checksum', /[1-9]/, { timeout: 15000 });
});
