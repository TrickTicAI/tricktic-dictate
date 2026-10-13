import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
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
