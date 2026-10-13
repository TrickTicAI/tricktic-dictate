import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: '*.spec.ts', workers: 1, timeout: 30_000,
  use: { baseURL: 'http://127.0.0.1:15204', browserName: 'chromium',
    permissions: ['microphone'], launchOptions: { args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] } },
  webServer: { command: 'node server.mjs', url: 'http://127.0.0.1:15204', reuseExistingServer: false },
});
