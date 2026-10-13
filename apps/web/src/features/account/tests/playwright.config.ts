import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: '*.spec.ts', workers: 1, timeout: 20_000,
  use: { baseURL: 'http://127.0.0.1:15231', browserName: 'chromium' },
  webServer: { command: 'pnpm exec vite --host 127.0.0.1 --port 15231 --strictPort', url: 'http://127.0.0.1:15231', reuseExistingServer: false },
});
