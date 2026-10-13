import { defineConfig } from '../../../apps/web/node_modules/@playwright/test/index.mjs';
export default defineConfig({
  outputDir: '../../../apps/web/test-results/integrated',
  testDir: '.', testMatch: 'integrated.spec.ts', workers: 1, timeout: 35_000,
  use: { browserName: 'chromium', permissions: ['microphone'],
    launchOptions: { args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] } },
});
