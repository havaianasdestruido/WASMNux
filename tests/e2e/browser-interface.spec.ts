import { expect, test } from '@playwright/test';

test('drives full Browser Interface: interactive xterm.js typing, pause/resume, snapshots, invalid disk import banner, diagnostics, profile switch, persistent UI settings, and PWA SW', async ({
  page,
}) => {
  test.setTimeout(120_000);
  const pageErrors: Error[] = [];
  page.on('pageerror', (err) => pageErrors.push(err));

  await page.goto('/');
  await expect(page.locator('main[data-ready="true"]')).toBeVisible();

  // Verify PWA manifest link & Service Worker readiness
  const pwaCheck = await page.evaluate(async () => {
    const manifestHref = document.querySelector('link[rel="manifest"]')?.getAttribute('href');
    const swSupported = 'serviceWorker' in navigator;
    let swRegistered = false;
    if (swSupported) {
      const reg = await navigator.serviceWorker.ready;
      swRegistered = Boolean(reg.active);
    }
    return { manifestHref, swSupported, swRegistered };
  });
  expect(pwaCheck.manifestHref).toContain('manifest.webmanifest');
  expect(pwaCheck.swRegistered).toBe(true);

  // Customize terminal font size (14 -> 16) and theme ('amber')
  await page.getByTestId('btn-font-inc').click();
  await page.getByTestId('btn-font-inc').click();
  await page.getByTestId('select-term-theme').selectOption('amber');

  // Boot Vanilla Linux 7.3.1 via the UI Boot button
  await page.getByTestId('btn-boot').click();
  await expect(page.getByTestId('badge-vm-state')).toContainText('State: RUNNING', {
    timeout: 30_000,
  });

  // Switch to Boot Diagnostics tab and wait until kernel & userspace ready milestones appear
  await page.getByTestId('tab-diagnostics').click();
  await expect(page.getByTestId('milestone-list')).toContainText('Linux Kernel Banner', {
    timeout: 30_000,
  });
  await expect(page.getByTestId('milestone-list')).toContainText('Userspace Init & Shell Ready', {
    timeout: 30_000,
  });

  // Type real interactive command directly into xterm.js helper textarea -> guest NS16550A UART!
  const xtermInput = page.locator('.xterm-helper-textarea');
  await xtermInput.focus();
  await page.keyboard.type('echo "XTERM_REAL_GUEST_$((20+22))" && uname -r\n');

  await expect(page.getByTestId('kernel-log-view')).toContainText('XTERM_REAL_GUEST_42', {
    timeout: 15_000,
  });

  // Test Ctrl+C (SIGINT) in guest shell
  await xtermInput.focus();
  await page.keyboard.press('Control+c');

  // Test Pause -> Resume controls
  await page.getByTestId('btn-pause-resume').click();
  await expect(page.getByTestId('badge-vm-state')).toContainText('State: PAUSED');
  await page.getByTestId('btn-pause-resume').click();
  await expect(page.getByTestId('badge-vm-state')).toContainText('State: RUNNING');

  // Save a named snapshot from the Snapshots tab
  await page.getByTestId('tab-snapshots').click();
  await page.getByTestId('input-snapshot-name').fill('ui-checkpoint');
  await page.getByTestId('btn-save-snapshot').click();
  await expect(page.getByTestId('snapshot-list')).toContainText('ui-checkpoint', {
    timeout: 15_000,
  });

  // Attempt importing a corrupted/invalid .wasmnux-disk file -> verify error banner surfaces cleanly
  await page.getByTestId('input-import-disk').setInputFiles({
    name: 'corrupt.wasmnux-disk',
    mimeType: 'application/octet-stream',
    buffer: Buffer.from('NOT_A_VALID_WASMNUX_BUNDLE_PAYLOAD_12345'),
  });
  await expect(page.getByTestId('error-banner')).toBeVisible();
  await expect(page.getByTestId('error-banner-text')).toContainText('Invalid disk import rejected');
  await page.getByTestId('btn-dismiss-error').click();
  await expect(page.getByTestId('error-banner')).toBeHidden();

  // Switch profile to Debian 13.7 ("trixie"), then reload page and verify persisted UI preferences!
  await page.getByTestId('profile-debian').click();
  await expect(page.getByTestId('profile-debian')).toHaveAttribute('aria-pressed', 'true');

  await page.reload();
  await expect(page.locator('main[data-ready="true"]')).toBeVisible();
  await expect(page.getByTestId('profile-debian')).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByTestId('select-term-theme')).toHaveValue('amber');

  expect(pageErrors).toHaveLength(0);
});
