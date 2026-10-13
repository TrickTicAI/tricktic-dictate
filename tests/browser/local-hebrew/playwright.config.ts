import { defineConfig } from '../../../apps/web/node_modules/@playwright/test';
import base from '../../../apps/web/playwright.config';
export default defineConfig({ ...base, testDir: '.', testMatch: '*.spec.ts',
  use: { ...base.use, baseURL: 'http://127.0.0.1:15186' },
  webServer: { ...base.webServer as object, command: 'pnpm exec vite --host 127.0.0.1 --port 15186 --strictPort', url: 'http://127.0.0.1:15186' },
});
