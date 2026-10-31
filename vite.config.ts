import { defineConfig } from 'vite';
export default defineConfig({
  root: 'web',
  base: './',
  server: { host: '0.0.0.0', allowedHosts: true },
  build: { target: 'es2022', outDir: '../dist', emptyOutDir: true },
});
