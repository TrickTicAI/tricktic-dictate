import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../../../../../', import.meta.url));
export default defineConfig({ root: `${root}tests/browser/desktop-session`, build: { target: 'es2022', outDir: `${root}.tmp/desktop-session-web`, emptyOutDir: true } });
