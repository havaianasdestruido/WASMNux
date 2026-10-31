import { test, expect } from '@playwright/test';
test('snapshot create/restore/delete controls operate on a real guest', async ({ browser }) => {
  const context = await browser.newContext({ serviceWorkers: 'block' });
  const page = await context.newPage();
  await page.goto('/');
  await page.getByRole('button', { name: 'Boot', exact: true }).click();
  await expect(page.locator('#terminal')).toContainText('WASMNUX_PERSISTENT_ROOT_READY', { timeout: 280000 });
  await page.getByRole('button', { name: 'Snapshot', exact: true }).click();
  await expect(page.locator('#snapshots option')).toHaveCount(1, { timeout: 30000 });
  await page.getByRole('button', { name: 'Restore', exact: true }).click();
  page.once('dialog', (d) => d.accept());
  await expect(page.locator('#status')).toContainText('Snapshot restored', { timeout: 30000 });
  await page.getByRole('button', { name: 'Delete snapshot' }).click();
  page.once('dialog', (d) => d.accept());
  await expect(page.locator('#snapshots option')).toHaveCount(0, { timeout: 15000 });
  await context.close();
});
