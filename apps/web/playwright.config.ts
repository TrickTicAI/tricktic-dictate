import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';
export default defineConfig({
  testDir: fileURLToPath(new URL('../../tests/browser', import.meta.url)), testMatch: '*.spec.ts', workers: 1, timeout: 45_000,
  use: { baseURL: 'http://127.0.0.1:15184', browserName: 'chromium', permissions: ['microphone', 'clipboard-read', 'clipboard-write'],
    launchOptions: { args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] } },
  webServer: { command: 'pnpm exec vite --host 127.0.0.1 --port 15184 --strictPort', cwd: fileURLToPath(new URL('.', import.meta.url)), url: 'http://127.0.0.1:15184', reuseExistingServer: false },
});
