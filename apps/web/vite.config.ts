import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  build: {
    rollupOptions: {
      plugins: [{
        name: 'retry-settings-import',
        renderDynamicImport({ targetModuleId }) {
          if (!targetModuleId?.endsWith('/features/settings/SettingsModal.tsx')) return null;
          // Chromium remembers rejected module URLs for the document lifetime.
          // A new fragment retries the same packaged file, even on desktop hosts
          // that reject query parameters. Rollup still owns the hashed filename.
          return { left: 'import(', right: ' + "#" + performance.now())' };
        },
      }],
    },
  },
  server: {
    host: process.env.WRITER_WEB_HOST ?? '127.0.0.1',
    port: Number(process.env.WRITER_WEB_PORT ?? 5174),
    strictPort: true,
    proxy: {
      '/api': {
        target: `http://127.0.0.1:${process.env.WRITER_API_PORT ?? 8791}`,
      },
    },
  },
});
