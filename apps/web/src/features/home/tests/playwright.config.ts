import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: '*.spec.ts', workers: 1, timeout: 20_000,
  use: { baseURL: 'http://127.0.0.1:15193', browserName: 'chromium' },
  webServer: { command: 'pnpm exec vite --host 127.0.0.1 --port 15193 --strictPort', url: 'http://127.0.0.1:15193', reuseExistingServer: false },
});
