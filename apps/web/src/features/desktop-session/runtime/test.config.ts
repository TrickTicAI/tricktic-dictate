import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../../../../../', import.meta.url));
export default defineConfig({
  projects: [
    { name: 'session', testDir: `${root}tests/browser/desktop-session` },
    { name: 'modes', testDir: fileURLToPath(new URL('./tests/', import.meta.url)) },
  ], testMatch: '*.spec.ts', workers: 1, timeout: 35_000,
  // Native-origin fixtures relay their transformed modules through port 15184.
  use: { baseURL: 'http://127.0.0.1:15184', browserName: 'chromium', permissions: ['microphone'],
    launchOptions: { args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', '--autoplay-policy=no-user-gesture-required'] } },
  webServer: { command: 'pnpm exec vite --host 127.0.0.1 --port 15184 --strictPort', cwd: `${root}apps/web`, url: 'http://127.0.0.1:15184', reuseExistingServer: false },
});
