import { expect, test } from '@playwright/test';
import fixtures from '../../../packages/contracts/examples.json';

test.beforeEach(async ({ page }) => {
  await page.route('http://tauri.localhost/**', async route => {
    if (route.request().url().endsWith('/fast-test')) return route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Native fast path</title>' });
    const response = await route.fetch({ url: route.request().url().replace('http://tauri.localhost', 'http://127.0.0.1:15184') });
    await route.fulfill({ response });
  });
  await page.goto('http://tauri.localhost/fast-test');
  await page.evaluate(() => {
    const w = window as any;
    w.calls = []; w.listeners = {};
    w.__TAURI__ = { core: { invoke: async (command: string, args: any) => { w.calls.push({ command, args }); } },
      event: { listen: async (name: string, cb: any) => { w.listeners[name] = cb; return () => { delete w.listeners[name]; }; } } };
  });
});

test('native preview scheduler starts immediately and receives text without JavaScript polling', async ({ page }) => {
  await page.evaluate(async options => {
    const w = window as any;
    const path = '/src/features/desktop-session/runtime/preview.ts';
    const { LocalPreview } = await import(path);
    w.texts = []; w.preview = new LocalPreview((text: string) => w.texts.push(text));
    options.selection.provider = 'local'; options.selection.model = 'ivrit-turbo';
    w.preview.start('current', options);
  }, structuredClone(fixtures.transcription_request.data.options));
  await expect.poll(() => page.evaluate(() => (window as any).calls.length)).toBe(1);
  const result = await page.evaluate(() => {
    const w = window as any;
    w.listeners['writer-preview']({ payload: { id: 'old', text: 'stale' } });
    w.listeners['writer-preview']({ payload: { id: 'current', text: 'provisional' } });
    const late = w.listeners['writer-preview']; w.preview.stop();
    late({ payload: { id: 'current', text: 'late' } });
    return { texts: w.texts, calls: w.calls, listening: !!w.listeners['writer-preview'] };
  });
  expect(result.texts).toEqual(['', 'provisional']);
  expect(result.calls[0].command).toBe('capture_preview_start');
  expect(result.calls[0].args.options.cleanup).toEqual({ mode: 'off' });
  expect(result.listening).toBe(false);
});

test('durable native success is delivered while the browser history mirror is still writing', async ({ page }) => {
  await page.evaluate(async fixture => {
    const w = window as any;
    const path = '/src/features/desktop-session/runtime/jobs.ts'; const { SessionJobs } = await import(path);
    const completed = structuredClone(fixture.job.data);
    const queued = { ...completed, status: { state: 'queued' } };
    w.delivered = []; w.writeFinished = false;
    w.jobs = new SessionJobs((_id: string, job: any) => w.delivered.push(job?.status.state), {
      load: async () => [{ id: queued.request.recording_id, request: queued.request, job: queued }],
      save: async () => { await new Promise(resolve => { w.finishWrite = resolve; }); w.writeFinished = true; },
      upload: async () => {}, submit: async () => queued, get: async () => queued, pollMs: 60000,
    });
    w.visible = [];
    w.jobs.subscribe(() => w.visible.push(w.jobs.list()[0].job.status.state));
    await w.jobs.initialize(); w.jobs.receiveNative(completed);
  }, fixtures);
  await expect.poll(() => page.evaluate(() => (window as any).delivered)).toEqual(['succeeded']);
  expect(await page.evaluate(() => (window as any).writeFinished)).toBe(false);
  expect(await page.evaluate(() => (window as any).visible)).toEqual(['succeeded']);
  await page.evaluate(() => (window as any).finishWrite());
  await expect.poll(() => page.evaluate(() => (window as any).writeFinished)).toBe(true);
  await page.evaluate(() => (window as any).jobs.dispose());
});

test('native mode completion supplies final text before exactly one guarded delivery', async ({ page }) => {
  await page.evaluate(async fixture => {
    const w = window as any;
    w.__TAURI__.core.invoke = async (command: string, args: any) => {
      w.calls.push({ command, args });
      if (command === 'prepare_delivery') return new Promise(resolve => { w.completeMode = resolve; });
      if (command === 'deliver_transcript') return { copied: true, pasted: true, message: '' };
      throw new Error('Unexpected command');
    };
    window.fetch = async () => { throw new Error('Native completion must not depend on HTTP polling'); };
    const path = '/src/features/desktop-session/runtime/delivery.ts'; const { deliverTranscript } = await import(path);
    w.ready = false;
    // Asked once the final text exists, immediately before insertion.
    w.first = deliverTranscript(fixture.job.data, () => { w.ready = true; return true; });
    w.second = deliverTranscript(fixture.job.data);
  }, fixtures);
  await expect.poll(() => page.evaluate(() => (window as any).calls.length)).toBe(1);
  expect(await page.evaluate(() => (window as any).ready)).toBe(false);
  const result = await page.evaluate(async () => {
    const w = window as any; w.completeMode('final mode output'); await Promise.all([w.first, w.second]);
    return { calls: w.calls, ready: w.ready };
  });
  expect(result.ready).toBe(true);
  expect(result.calls.map((c: any) => c.command)).toEqual(['prepare_delivery', 'deliver_transcript']);
  expect(result.calls[1].args.text).toBe('final mode output');
});

test('first failed request write recovers before a retry submits the original key once', async ({ page }) => {
  await page.clock.install();
  const result = await page.evaluate(async fixture => {
    const path = '/src/features/desktop-session/runtime/jobs.ts';
    const { SessionJobs } = await import(path);
    const request = fixture.job.data.request;
    const submitted: string[] = [];
    let writes = 0;
    const jobs = new SessionJobs(() => {}, {
      load: async () => [],
      save: async () => { if (++writes === 1) throw new Error('Temporary write failure'); },
      upload: async () => {},
      submit: async (next: typeof request) => { submitted.push(next.idempotency_key); return { ...fixture.job.data, request: next }; },
      pollMs: 1000,
    });
    await jobs.initialize();
    const run = (key: string) => jobs.run({ id: request.recording_id }, request.options, async () => new Blob(), key);
    await run('original');
    (window as any).recovery = { jobs, run, submitted, id: request.recording_id };
    return { busy: jobs.busy, count: jobs.list().length, submitted };
  }, fixtures);
  expect(result).toEqual({ busy: true, count: 0, submitted: [] });
  await page.clock.fastForward(1000);
  expect(await page.evaluate(async () => {
    const { jobs, run, submitted, id } = (window as any).recovery;
    const recovered = { key: jobs.get(id)?.request.idempotency_key, busy: jobs.busy, submissions: submitted.length };
    await run('explicit-retry');
    await run('original');
    jobs.dispose();
    return { recovered, submitted };
  })).toEqual({ recovered: { key: 'original', busy: false, submissions: 0 }, submitted: ['original'] });
});

test('merged history keeps the local request and appends native-only rows in native order', async ({ page }) => {
  const rows = await page.evaluate(async request => {
    const storagePath = '/src/features/transcripts/storage.ts';
    const jobsPath = '/src/features/desktop-session/runtime/jobs.ts';
    const { saveAttempt } = await import(storagePath);
    const { SessionJobs } = await import(jobsPath);
    const row = (id: string, key: string) => ({ id, request: { ...request, recording_id: id, idempotency_key: key } });
    await saveAttempt(row('overlap', 'uncertain-local'));
    (window as any).__TAURI__.core.invoke = async () => [
      row('z', 'native-z'), row('overlap', 'old-native'), row('a', 'native-a'),
    ];
    const jobs = new SessionJobs(() => {});
    await jobs.initialize();
    jobs.dispose();
    return jobs.list().map((a: { request: typeof request }) => a.request.idempotency_key);
  }, fixtures.transcription_request.data);
  expect(rows).toEqual(['uncertain-local', 'native-z', 'native-a']);
});
