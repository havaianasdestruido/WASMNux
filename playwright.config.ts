import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { brotliDecompressSync } from 'node:zlib';
import { defineConfig } from '@playwright/test';
import chromiumPkg from '@sparticuz/chromium';

function ensureSparticuzRuntimeLibs(): { ldLibraryPath: string; vkIcdPath: string } {
  const binDir = join(process.cwd(), 'node_modules', '@sparticuz', 'chromium', 'bin');
  const alDir = '/tmp/al2023';
  const swiftDir = '/tmp/swiftshader';

  if (!existsSync(join(alDir, 'lib', 'libnspr4.so'))) {
    mkdirSync(alDir, { recursive: true });
    const tar = brotliDecompressSync(readFileSync(join(binDir, 'al2023.tar.br')));
    writeFileSync('/tmp/al2023.tar', tar);
    execFileSync('tar', ['-xf', '/tmp/al2023.tar', '-C', alDir]);
    unlinkSync('/tmp/al2023.tar');
  }

  if (!existsSync(join(swiftDir, 'libvk_swiftshader.so'))) {
    mkdirSync(swiftDir, { recursive: true });
    const tar = brotliDecompressSync(readFileSync(join(binDir, 'swiftshader.tar.br')));
    writeFileSync('/tmp/swiftshader.tar', tar);
    execFileSync('tar', ['-xf', '/tmp/swiftshader.tar', '-C', swiftDir]);
    unlinkSync('/tmp/swiftshader.tar');
  }

  const icdPath = join(swiftDir, 'vk_swiftshader_icd.json');
  writeFileSync(
    icdPath,
    JSON.stringify({
      file_format_version: '1.0.0',
      ICD: {
        library_path: join(swiftDir, 'libvk_swiftshader.so'),
        api_version: '1.0.5',
      },
    }),
  );

  const existing = process.env['LD_LIBRARY_PATH'];
  const combined = [join(alDir, 'lib'), swiftDir, existing].filter(Boolean).join(':');
  return { ldLibraryPath: combined, vkIcdPath: icdPath };
}

const resolvedExecutablePath = await chromiumPkg.executablePath();
const runtimeLibs = ensureSparticuzRuntimeLibs();

export default defineConfig({
  testDir: './tests/e2e',
  timeout: 180000,
  expect: {
    timeout: 30000,
  },
  fullyParallel: false,
  workers: 1,
  reporter: [['list']],
  use: {
    baseURL: 'http://127.0.0.1:4173',
    headless: true,
    launchOptions: {
      executablePath: resolvedExecutablePath,
      env: {
        ...(process.env as Record<string, string>),
        LD_LIBRARY_PATH: runtimeLibs.ldLibraryPath,
        VK_ICD_FILENAMES: runtimeLibs.vkIcdPath,
      },
      args: [
        '--no-sandbox',
        '--disable-setuid-sandbox',
        '--disable-dev-shm-usage',
        '--ignore-gpu-blocklist',
        '--enable-unsafe-swiftshader',
        '--enable-unsafe-webgpu',
        '--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan,WebGPU',
        '--use-angle=vulkan',
        '--use-vulkan=swiftshader',
        '--use-webgpu-adapter=swiftshader',
      ],
    },
  },
  webServer: {
    command: 'npm run preview',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: true,
    timeout: 60000,
  },
});
