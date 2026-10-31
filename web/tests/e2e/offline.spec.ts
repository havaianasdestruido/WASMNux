import { test, expect, chromium as pw } from '@playwright/test';
import sparticuz from '@sparticuz/chromium';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
test('cached app boots offline after first online boot', async () => {
  const context = await pw.launchPersistentContext(mkdtempSync(join(tmpdir(), 'wnx-offline-')), {
    executablePath: process.env.CHROMIUM_PATH ?? (await sparticuz.executablePath()),
    args: sparticuz.args.filter((a) => !a.startsWith('--disk-cache-size') && a !== '--single-process'),
  });
  const page = await context.newPage();
  await page.goto('/');
  await page.evaluate(async () => {
    await navigator.serviceWorker.ready;
  });
  await page.getByRole('button', { name: 'Boot', exact: true }).click();
  await expect(page.locator('#terminal')).toContainText('WASMNUX_PERSISTENT_ROOT_READY', { timeout: 280000 });
  await page.getByRole('button', { name: 'Shut down' }).click();
  await expect(page.locator('#status')).toContainText(/Guest powered off|Stopped/, { timeout: 15000 });
  await context.setOffline(true);
  await page.reload();
  await page.getByRole('button', { name: 'Boot', exact: true }).click();
  await expect(page.locator('#terminal')).toContainText('WASMNUX_PERSISTENT_ROOT_READY', { timeout: 280000 });
  await context.close();
});
