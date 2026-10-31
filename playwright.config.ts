import { defineConfig } from '@playwright/test';
import chromium from '@sparticuz/chromium';
const executablePath = process.env.CHROMIUM_PATH ?? (await chromium.executablePath());
export default defineConfig({
  testDir: './web/tests/e2e',
  timeout: 320000,
  workers: 1,
  use: {
    baseURL: 'http://127.0.0.1:4173',
    headless: true,
    launchOptions: {
      executablePath,
      args: chromium.args.filter((a) => !a.startsWith('--disk-cache-size') && a !== '--single-process'),
    },
  },
  webServer: {
    command: 'npm run preview -- --port 4173',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: true,
  },
});
