// Real isolated API/storage/worker; synthetic audio and ONLY provider discovery is a fixture.
// The API has no keys, so the expected result is a real provider-unavailable error.
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import { randomBytes, createHash } from 'node:crypto';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../..', import.meta.url));
const require = createRequire(join(root, 'apps/web/package.json'));
const { chromium, expect } = require('@playwright/test');
const data = await mkdtemp(join(tmpdir(), 'writer-api-browser-'));
const ownerKey = randomBytes(32).toString('hex');
const keyPath = join(data, 'owner-key');
await writeFile(keyPath, ownerKey, { mode: 0o600 });
const env = { PATH: process.env.PATH, HOME: data, WRITER_API_HOST: '127.0.0.1', WRITER_API_PORT: '18792',
  WRITER_OWNER_KEY_FILE: keyPath, WRITER_HTTP_ORIGIN: 'http://127.0.0.1:18792',
  WRITER_DATA_DIR: data, WRITER_STATE_DIR: join(data, 'state'), WRITER_MODELS_DIR: join(data, 'models'),
  WRITER_VOCABULARY_PATH: join(data, 'vocabulary.json'), WRITER_WEB_DIST: join(root, 'apps/web/dist') };
let headers;
const apiFetch = (path) => fetch(`http://127.0.0.1:18792${path}`, { headers });
async function login(page) {
  await page.getByLabel('Owner key').fill(ownerKey);
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Sign out', exact: true })).toBeVisible();
}
const api = spawn(join(root, 'target/debug/metrix-writer-api'), [], { cwd: root, env, stdio: 'ignore' });
let browser;
try {
  for (let i = 0; i < 100; i++) {
    if (api.exitCode !== null) throw Error('Isolated API exited; check port 18792');
    try { if ((await fetch('http://127.0.0.1:18792/api/v1/health')).status === 403) break; } catch {}
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  const session = await fetch('http://127.0.0.1:18792/api/v1/session', {
    method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Writer-Request': '1' },
    body: JSON.stringify({ owner_key: ownerKey }),
  });
  if (!session.ok) throw Error('Disposable owner sign-in failed');
  headers = { Authorization: `Bearer ${(await session.json()).data.token}`, 'X-Writer-Request': '1' };
  const providers = await (await apiFetch('/api/v1/providers')).json();
  if (providers.providers.some(provider => provider.configured)) throw Error('Smoke must run with no configured adapters');
  browser = await chromium.launch({ args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] });
  const context = await browser.newContext({ permissions: ['microphone'], viewport: { width: 1280, height: 1000 } });
  const page = await context.newPage();
  await page.route('**/api/v1/providers', route => route.fulfill({ json: { providers: [{ id: 'openai', name: 'Fixture discovery only', configured: true, available: false, status: 'unverified' }] } }));
  const submissions = [];
  page.on('request', request => { if (new URL(request.url()).pathname === '/api/v1/transcriptions') submissions.push(request.postDataJSON().data); });
  await page.goto('http://127.0.0.1:18792'); await login(page);
  await page.getByRole('button', { name: 'Record', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  await expect(page.getByLabel('Recording duration')).not.toHaveText('00:00');
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(page.getByText('The saved audio is untouched and can be retried.')).toBeVisible();
  const id = submissions[0].recording_id;
  const retained = await (await apiFetch(`/api/v1/recordings/${id}`)).json();
  if (retained.data.audio.state !== 'saved') throw Error('Server did not save audio');
  await page.reload(); await login(page); await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect.poll(() => submissions.length).toBe(2);
  await expect(page.getByText('The saved audio is untouched and can be retried.').first()).toBeVisible();
  if (submissions[1].recording_id !== id || !submissions[1].retry_of_job_id) throw Error('Retry lost source/previous job');
  const source = await page.locator('audio').getAttribute('src');
  const browserBytes = await page.evaluate(async source => [...new Uint8Array(await (await fetch(source)).arrayBuffer())], source);
  const serverBytes = Buffer.from(await (await apiFetch(`/api/v1/recordings/${id}/audio`)).arrayBuffer());
  const hash = value => createHash('sha256').update(value).digest('hex');
  const same = hash(Buffer.from(browserBytes)) === hash(serverBytes);
  if (!same) throw Error('Server audio differs from browser copy');
  await page.screenshot({ path: join(data, 'failed-retry.png'), fullPage: true });
  const result = { passed: true, data, recordingId: id, bytes: retained.data.metadata.byte_length, attempts: submissions.length, sameAudio: same, syntheticMicrophone: true, realApi: true, discoveryFixture: true, liveTranscription: false };
  await writeFile(join(data, 'result.json'), JSON.stringify(result, null, 2)); console.log(JSON.stringify(result));
} finally { await browser?.close(); api.kill('SIGTERM'); await rm(keyPath, { force: true }); }
