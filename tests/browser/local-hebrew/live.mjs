// Opt-in live saved-audio proof. No audio, references, or raw results belong in Git.
// Uses an already verified operator-owned GPU worker; starts only a new Writer API.
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import { createHash, randomUUID, randomBytes } from 'node:crypto';
import { readFile, writeFile, mkdir, rm } from 'node:fs/promises';
import { join, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../../', import.meta.url));
const require = createRequire(join(root, 'apps/web/package.json'));
const { chromium, expect } = require('@playwright/test');
const manifestPath = process.env.WRITER_EVAL_MANIFEST;
const output = process.env.WRITER_EVAL_OUTPUT;
const binary = process.env.WRITER_EVAL_API_BINARY;
if (![manifestPath, output, binary].every(value => value && isAbsolute(value))) throw Error('Set absolute WRITER_EVAL_MANIFEST, WRITER_EVAL_OUTPUT, WRITER_EVAL_API_BINARY');
if (output.startsWith(root) || manifestPath.startsWith(root)) throw Error('Keep evaluation inputs and outputs outside source');
if (process.env.WRITER_LOCAL_STT_MODEL !== 'ivrit-turbo') throw Error('This harness requires the verified Hebrew ivrit-turbo worker');
const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
if (!manifest.public_corpus_reuse_confirmed || !manifest.samples?.length || manifest.samples.some(s => s.input_language !== 'he' || !isAbsolute(s.audio_path))) throw Error('Supply public-corpus provenance and absolute Hebrew sample paths');
await mkdir(output, { recursive: false });
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const port = process.env.WRITER_EVAL_PORT || '18794';
const origin = `http://127.0.0.1:${port}`;
const ownerKey = randomBytes(32).toString('hex');
const keyPath = join(output, 'owner-key');
await writeFile(keyPath, ownerKey, { flag: 'wx', mode: 0o600 });
let headers;
const apiFetch = path => fetch(`${origin}${path}`, { headers });
async function login(page) {
  await page.getByLabel('Owner key').fill(ownerKey);
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Sign out', exact: true })).toBeVisible();
}
const api = spawn(binary, [], { cwd: root, stdio: 'ignore', env: { ...process.env,
  OPENAI_API_KEY: '', GEMINI_API_KEY: '', WRITER_API_HOST: '127.0.0.1', WRITER_API_PORT: port,
  WRITER_OWNER_KEY_FILE: keyPath, WRITER_HTTP_ORIGIN: origin,
  WRITER_DATA_DIR: join(output, 'server'), WRITER_WEB_DIST: join(root, 'apps/web/dist'),
} });
let browser;
try {
  await expect.poll(async () => {
    if (api.exitCode !== null) throw Error('Isolated API exited; check selected port');
    try { return (await fetch(`${origin}/api/v1/health`)).status === 403; } catch { return false; }
  }, { timeout: 20000 }).toBe(true);
  const session = await fetch(`${origin}/api/v1/session`, {
    method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Writer-Request': '1' },
    body: JSON.stringify({ owner_key: ownerKey }),
  });
  if (!session.ok) throw Error('Disposable owner sign-in failed');
  headers = { Authorization: `Bearer ${(await session.json()).data.token}`, 'X-Writer-Request': '1' };
  await expect.poll(async () => {
    const result = await (await apiFetch('/api/v1/providers')).json();
    return result.providers.some(p => p.id === 'local' && p.available && p.models.includes('ivrit-turbo'));
  }, { timeout: 35000 }).toBe(true);
  browser = await chromium.launch();
  const page = await browser.newPage();
  const submissions = []; let uploads = 0;
  page.on('request', request => {
    const path = new URL(request.url()).pathname;
    if (path === '/api/v1/recordings' && request.method() === 'POST') uploads++;
    if (path === '/api/v1/transcriptions') submissions.push(request.postDataJSON().data);
  });
  const settings = { ui_language: 'en', transcription: { selection: { provider: 'local', model: 'ivrit-turbo' }, input_language: 'he', output_language: { mode: 'preserve' }, vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' } } };
  await page.addInitScript(settings => localStorage.setItem('writer-preferences', JSON.stringify(settings)), settings);
  await page.goto(origin); await login(page);
  const results = []; const audit = [];
  for (const sample of manifest.samples) {
    const audio = await readFile(sample.audio_path); const id = randomUUID(); const audio_sha256 = hash(audio);
    // Import an already saved WAV into the actual browser store; no microphone simulation.
    await page.evaluate(async ({ id, bytes, duration }) => {
      const db = await new Promise((resolve, reject) => { const r = indexedDB.open('metrix-writer-recordings', 1); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });
      await new Promise((resolve, reject) => {
        const tx = db.transaction(['sessions', 'chunks'], 'readwrite');
        tx.objectStore('sessions').add({ id, created_at_ms: Date.now(), mime_type: 'audio/wav', byte_length: bytes.length, duration_ms: duration, input_language: 'he', capture_state: 'complete', chunk_count: 1 });
        tx.objectStore('chunks').add({ id, index: 0, blob: new Blob([new Uint8Array(bytes)], { type: 'audio/wav' }) });
        tx.oncomplete = resolve; tx.onabort = () => reject(tx.error);
      }); db.close();
    }, { id, bytes: [...audio], duration: sample.duration_ms });
    await page.reload(); await login(page);
    await expect(page.getByRole('button', { name: 'Transcribe saved audio', exact: true })).toBeEnabled();
    const count = results.length === 0 ? 2 : 1;
    for (let attempt = 0; attempt < count; attempt++) {
      const started = Date.now();
      await page.getByRole('button', { name: attempt ? 'Retry saved audio' : 'Transcribe saved audio', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Retry saved audio', exact: true })).toBeEnabled({ timeout: 180000 });
      const original = page.getByLabel('Original transcript').first();
      await expect(original).toBeVisible();
      const raw_text = await original.inputValue(); const ended = Date.now();
      const request = submissions.at(-1);
      if (request.recording_id !== id || request.options.input_language !== 'he' || request.options.output_language.mode !== 'preserve') throw Error('Browser request changed saved audio/options');
      const retained = Buffer.from(await (await apiFetch(`/api/v1/recordings/${id}/audio`)).arrayBuffer());
      if (hash(retained) !== audio_sha256 || hash(await readFile(sample.audio_path)) !== audio_sha256) throw Error('Audio changed');
      const result = { id: sample.id, input_language: 'he', raw_text, audio_sha256, consent_confirmed: true, stopped_at_ms: started, result_at_ms: ended, recording_duration_ms: sample.duration_ms };
      if (!attempt) results.push(result);
      audit.push({ sample: sample.id, attempt, recording_id: id, audio_sha256, elapsed_ms: ended - started, raw_sha256: hash(raw_text), bytes: retained.length, request });
      if (attempt && (!request.retry_of_job_id || audit.at(-2).recording_id !== id)) throw Error('Retry lost previous job/source');
    }
  }
  if (uploads !== manifest.samples.length) throw Error('Retry uploaded/rerecorded audio');
  const run = { kind: 'audio_run', model: 'local/ivrit-turbo', model_revision: manifest.model_sha256, run_at: new Date().toISOString(), settings: { ...settings.transcription, runtime: manifest.runtime, device: manifest.device, note: 'Saved clip submit-to-visible latency; model startup excluded. Public corpus reuse, not personal microphone recordings.' }, results };
  await writeFile(join(output, 'run.json'), JSON.stringify(run, null, 2));
  await writeFile(join(output, 'audit.json'), JSON.stringify({ browser: browser.version(), uploads, audit }, null, 2));
  console.log(JSON.stringify({ passed: true, output, samples: results.length, attempts: audit.length, uploads, elapsed_ms: audit.map(a => a.elapsed_ms), identicalSavedAudio: true }));
} finally { await browser?.close(); api.kill('SIGTERM'); await rm(keyPath, { force: true }); }
