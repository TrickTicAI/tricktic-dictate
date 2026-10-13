// Isolated real server with synthetic credentials and no provider configuration.
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../../../../../', import.meta.url));
const directory = await mkdtemp(join(tmpdir(), 'writer-auth-browser-'));
const key = join(directory, 'owner-key');
await writeFile(key, 'a'.repeat(64), { mode: 0o600 });
const foreign = createServer((_request, response) => {
  response.setHeader('Content-Type', 'text/html');
  response.end('<!doctype html><title>Foreign origin</title>');
});
await new Promise((resolve, reject) => { foreign.once('error', reject); foreign.listen(15205, '127.0.0.1', resolve); });
const api = spawn(join(root, 'target/debug/metrix-writer-api'), [], {
  cwd: root, stdio: ['ignore', 'ignore', 'inherit'],
  env: { PATH: process.env.PATH, HOME: directory, WRITER_DATA_DIR: join(directory, 'data'),
    WRITER_STATE_DIR: join(directory, 'state'), WRITER_MODELS_DIR: join(directory, 'models'),
    WRITER_VOCABULARY_PATH: join(directory, 'vocabulary.json'),
    WRITER_API_HOST: '127.0.0.1', WRITER_API_PORT: '15204',
    WRITER_HTTP_ORIGIN: 'http://127.0.0.1:15204', WRITER_OWNER_KEY_FILE: key,
    WRITER_WEB_DIST: join(root, 'apps/web/dist') },
});
api.on('exit', async code => { foreign.close(); await rm(directory, { recursive: true, force: true }); process.exit(code ?? 0); });
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => api.kill('SIGINT'));
