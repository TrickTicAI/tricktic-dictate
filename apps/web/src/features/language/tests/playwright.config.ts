import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: '*.spec.ts', workers: 1,
  use: { baseURL: 'http://127.0.0.1:15187', browserName: 'chromium' },
  webServer: { command: 'pnpm exec vite --host 127.0.0.1 --port 15187 --strictPort', url: 'http://127.0.0.1:15187', reuseExistingServer: false },
});
