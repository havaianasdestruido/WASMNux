import { test, expect, chromium as pw } from '@playwright/test';
import sparticuz from '@sparticuz/chromium';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
test('Debian boots and apt/dpkg package state survives reload', async () => {
  const context = await pw.launchPersistentContext(mkdtempSync(join(tmpdir(), 'wnx-debian-')), {
    executablePath: process.env.CHROMIUM_PATH ?? (await sparticuz.executablePath()),
    args: sparticuz.args.filter((a) => !a.startsWith('--disk-cache-size') && a !== '--single-process'),
    serviceWorkers: 'block',
  });
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/');
  await page.selectOption('#profile', 'debian');
  await page.getByRole('button', { name: 'Boot', exact: true }).click();
  await expect(page.locator('#terminal')).toContainText('WASMNUX_DEBIAN_READY', { timeout: 280000 });
  await page.locator('#terminal').click();
  await page.keyboard.type(
    'apt-get install -y /opt/packages/wasmnux-demo_1.0_riscv64.deb > /tmp/apt.log 2>&1; echo APT_RC=$?; dpkg-query -W wasmnux-demo; sync',
  );
  await page.keyboard.press('Enter');
  await expect(page.locator('#terminal')).toContainText('APT_RC=0', { timeout: 60000 });
  await expect(page.locator('#terminal')).toContainText('wasmnux-demo\t1.0', { timeout: 10000 });
  await page.getByRole('button', { name: 'Shut down' }).click();
  await expect(page.locator('#status')).toContainText(/Guest powered off|Stopped/, { timeout: 15000 });
  await page.reload();
  await page.selectOption('#profile', 'debian');
  await page.getByRole('button', { name: 'Boot', exact: true }).click();
  await expect(page.locator('#terminal')).toContainText('WASMNUX_DEBIAN_READY', { timeout: 280000 });
  await page.locator('#terminal').click();
  await page.keyboard.type('dpkg-query -W wasmnux-demo; wasm nux-demo 2>/dev/null || wasmnux-demo');
  await page.keyboard.press('Enter');
  await expect(page.locator('#terminal')).toContainText('WASMNUX_DEMO_PACKAGE_OK', { timeout: 15000 });
  await page.keyboard.type(
    'apt-get remove -y wasmnux-demo > /tmp/rm.log 2>&1; echo REMOVE_RC=$?; dpkg-query -W wasmnux-demo 2>&1; sync',
  );
  await page.keyboard.press('Enter');
  await expect(page.locator('#terminal')).toContainText('REMOVE_RC=0', { timeout: 60000 });
  await expect(page.locator('#terminal')).toContainText('no packages found matching wasmnux-demo', {
    timeout: 10000,
  });
  expect(errors).toEqual([]);
  await context.close();
});
