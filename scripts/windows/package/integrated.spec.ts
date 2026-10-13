import { test, expect, type Page } from '../../../apps/web/node_modules/@playwright/test/index.mjs';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import type { Job, Settings, Recording, TranscriptionRequest } from '../../../packages/contracts';
const origin = 'https://writer.metrix.invalid';
const dist = new URL('../../../apps/web/dist/', import.meta.url);
const envelope = (data: unknown) => ({ api_version: '1', data });
async function fixture(page: Page) {
  let settings: Settings = { ui_language: 'en', transcription: { selection: { provider: 'openai', model: 'gpt-transcribe' }, input_language: 'en', output_language: { mode: 'preserve' }, vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' } } };
  const requests: TranscriptionRequest[] = [];
  const recordings = new Map<string, Recording>();
  const jobs = new Map<string, Job>();
  let fail = false, configured = true;
  await page.addInitScript(() => {
    const listeners = new Set<(event: MessageEvent) => void>();
    Object.assign(window, { chrome: { webview: { postMessage: () => {},
      addEventListener: (_type: string, listener: (event: MessageEvent) => void) => listeners.add(listener),
      removeEventListener: (_type: string, listener: (event: MessageEvent) => void) => listeners.delete(listener) } },
      nativeCommand: (action: string) => {
        const current = (window as any).writerDesktopSnapshot();
        const command = { version: 1, kind: 'command', epoch: current.epoch, command_id: crypto.randomUUID(), action, session_id: action === 'start' ? crypto.randomUUID() : current.session_id };
        for (const listener of listeners) listener(new MessageEvent('message', { data: command }));
      }, streams: 0 });
    const capture = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getUserMedia = constraints => { (window as any).streams++; return capture(constraints); };
  });
  await page.route(origin + '/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (!path.startsWith('/api/')) {
      const relative = path === '/' ? 'index.html' : path.slice(1);
      const file = new URL(relative, dist);
      if (!file.href.startsWith(dist.href)) return route.abort();
      const type = path.endsWith('.js') ? 'application/javascript' : path.endsWith('.css') ? 'text/css' : path.endsWith('.woff2') ? 'font/woff2' : 'text/html';
      return route.fulfill({ body: await readFile(fileURLToPath(file)), contentType: type });
    }
    if (path.endsWith('/health')) return route.fulfill({ json: { status: 'ok' } });
    if (path === '/api/v1/providers') return route.fulfill({ json: { providers: [{ id: 'openai', configured, available: false, status: 'unverified' }] } });
    if (path === '/api/v1/vocabulary') return route.fulfill({ json: envelope(settings.transcription.vocabulary) });
    if (path === '/api/v1/setup') return route.fulfill({ json: envelope({ mode: 'desktop', first_run: false, settings, models: [], runtime: { worker: { state: 'idle' } }, cloud: [{ provider: 'openai', models: ['gpt-transcribe'], credential: configured ? 'protected_store' : 'absent', verified: false }] }) });
    if (path === '/api/v1/settings') {
      if (route.request().method() === 'PUT') settings = route.request().postDataJSON().data;
      return route.fulfill({ json: envelope(settings) });
    }
    if (path === '/api/v1/recordings') {
      const body = route.request().postDataBuffer()!.toString();
      const metadata = JSON.parse(body.split('name="metadata"\r\n\r\n')[1].split('\r\n--')[0]).data;
      const recording: Recording = { metadata, storage: 'self_hosted_server', audio: { state: 'saved' } };
      recordings.set(metadata.id, recording); return route.fulfill({ json: envelope(recording) });
    }
    if (path.startsWith('/api/v1/recordings/')) {
      const recording = recordings.get(path.split('/').at(-1)!);
      return route.fulfill({ status: recording ? 200 : 404, json: recording ? envelope(recording) : { api_version: '1', error: { code: 'recording_not_found', retryable: false } } });
    }
    if (path === '/api/v1/transcriptions') {
      const request = route.request().postDataJSON().data as TranscriptionRequest; requests.push(request);
      const job: Job = { id: crypto.randomUUID(), request, created_at_ms: Date.now(), updated_at_ms: Date.now(), status: fail ? { state: 'failed', error: { code: 'provider_failed', message: 'Fixture failure', retryable: true } } : { state: 'succeeded', result: { cleanup: { state: 'off' }, transcript: { raw_text: 'שלום — integrated fixture', input_language: request.options.input_language, output_language: request.options.output_language, selection: request.options.selection, execution_location: 'cloud' } } } };
      jobs.set(job.id, job); return route.fulfill({ json: envelope(job) });
    }
    if (path.endsWith('/processed')) return route.fulfill({ json: envelope({ text: 'שלום — integrated fixture' }) });
    if (path.startsWith('/api/v1/jobs/')) return route.fulfill({ json: envelope(jobs.get(path.split('/').at(-1)!)) });
    if (path === '/api/v1/cleanup/providers') return route.fulfill({ json: envelope([]) });
    return route.fulfill({ status: 404 });
  });
  return { requests, recordings, fail: () => { fail = true; }, unavailable: () => { configured = false; } };
}
async function ready(page: Page) {
  await page.goto(origin);
  await expect(page.getByText('API connected', { exact: true })).toBeVisible();
  await expect(page.getByText('Stop to save audio and transcribe with the selected model.')).toBeVisible();
}
test('mounted app shares native/UI capture, persisted settings, raw history and retry across reload', async ({ page }) => {
  const api = await fixture(page); await ready(page);
  await page.evaluate(() => (window as any).nativeCommand('start'));
  await expect(page.getByRole('button', { name: 'Stop recording', exact: true })).toBeEnabled();
  await expect(page.getByLabel('Recording duration')).not.toHaveText('00:00');
  await page.getByRole('button', { name: 'Stop recording', exact: true }).click();
  await expect(page.getByLabel('Original transcript')).toHaveValue('שלום — integrated fixture');
  expect(api.requests).toHaveLength(1);
  expect(await page.evaluate(() => (window as any).streams)).toBe(1);
  await page.getByRole('combobox', { name: 'Speech language', exact: true }).selectOption('mixed');
  await page.getByRole('button', { name: 'Use OpenAI', exact: true }).click();
  await expect(page.getByText('Saved. Writer reuses this choice on the next launch.')).toBeVisible();
  await page.reload();
  await expect(page.getByRole('combobox', { name: 'Speech language', exact: true })).toHaveValue('mixed');
  await expect(page.getByLabel('Original transcript')).toHaveValue('שלום — integrated fixture');
  api.fail();
  await page.getByRole('button', { name: 'Retry saved audio', exact: true }).click();
  await expect(page.getByText('Transcription failed. Audio is saved for retry.')).toBeVisible();
  await page.getByText('Previous attempts (1)', { exact: true }).click();
  await expect(page.getByLabel('Original transcript')).toHaveValue('שלום — integrated fixture');
  expect(api.requests).toHaveLength(2);
  expect(api.requests[1].options.input_language).toBe('mixed');
  expect(api.recordings.size).toBe(1);
  expect(await page.evaluate(() => (window as any).streams)).toBe(0);
});
test('missing model keeps capture usable and native cancel suppresses submission', async ({ page }) => {
  const api = await fixture(page); api.unavailable(); await page.goto(origin);
  await expect(page.getByRole('button', { name: 'Start recording', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Start recording', exact: true }).click();
  await expect(page.getByLabel('Recording duration')).not.toHaveText('00:00');
  await page.evaluate(() => (window as any).nativeCommand('cancel'));
  await expect(page.locator('.recording-row')).toHaveCount(1);
  expect(api.requests).toHaveLength(0);
  await page.reload();
  await expect(page.locator('.recording-row')).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Transcribe saved audio', exact: true })).toBeDisabled();
});
