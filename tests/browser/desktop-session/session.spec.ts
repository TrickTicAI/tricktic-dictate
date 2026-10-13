import { test, expect, type Page } from '@playwright/test';
import fixtures from '../../../packages/contracts/examples.json';

async function setup(page: Page, configured = true) {
  await page.route('**/owner-test', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Isolated session owner fixture</title>' }));
  await page.goto('/owner-test');
  await page.evaluate(async ({ fixtures, configured }) => {
    const ownerPath = '/src/features/desktop-session/runtime/owner.ts';
    const storePath = '/src/features/transcripts/storage.ts';
    const { SessionOwner } = await import(ownerPath);
    const storage = await import(storePath);
    const w = window as any;
    w.cancelRequests = []; w.events = []; w.requests = []; w.uploads = []; w.server = {}; w.submitMode = 'success'; w.pollState = 'queued';
    w.settings = { language: 'en', options: configured ? structuredClone(fixtures.transcription_request.data.options) : null };
    if (w.settings.options) w.settings.options.input_language = 'en';
    w.owner = await SessionOwner.create({ settings: () => w.settings, jobs: {
      load: storage.attempts, save: storage.saveAttempt,
      upload: async (metadata: any, audio: () => Promise<Blob>) => {
        const blob = await audio();
        if (blob.size !== metadata.byte_length || !blob.size) throw Error('uncommitted audio');
        w.uploads.push({ id: metadata.id, size: blob.size, capture: metadata.capture_state });
      },
      submit: async (request: any) => {
        w.requests.push(structuredClone(request));
        const job = w.server[request.idempotency_key] ?? { ...structuredClone(fixtures.job.data), id: crypto.randomUUID(), request: structuredClone(request) };
        if (w.submitMode === 'queued' || w.submitMode === 'deferred') job.status = { state: 'queued' };
        w.server[request.idempotency_key] = job;
        if (w.submitMode === 'lost') { w.submitMode = 'success'; throw Error('lost accepted response'); }
        if (w.submitMode === 'deferred') await new Promise(resolve => { w.releaseSubmit = resolve; });
        return structuredClone(job);
      },
      cancelRetries: async (id: string) => {
        w.cancelRequests.push(id);
        const job: any = Object.values(w.server).find((job: any) => job.id === id);
        if (!job) throw Error('offline');
        job.recovery = { attempts_started: 1, failures: [], next_retry_at_ms: null, waiting_for_capacity: false, canceled: true };
        return structuredClone(job);
      },
      get: async (id: string) => {
        const job: any = Object.values(w.server).find((job: any) => job.id === id);
        if (!job) throw Error('offline');
        if (w.pollState === 'success') job.status = structuredClone(fixtures.job.data.status);
        else if (w.pollState === 'transcribing') job.status = { state: 'transcribing' };
        return structuredClone(job);
      }, pollMs: 50,
    } });
    w.owner.subscribe((event: any) => w.events.push({ ...event, at: performance.now() }));
    w.command = (action: string, id?: string, commandId?: string) => ({ version: 1, kind: 'command', epoch: w.owner.snapshot.epoch,
      command_id: commandId ?? crypto.randomUUID(), action, session_id: action === 'snapshot' ? null : id ?? w.owner.snapshot.session_id });
    w.send = (action: string, id?: string, commandId?: string) => w.owner.dispatch(w.command(action, id, commandId));
  }, { fixtures, configured });
}
const snapshot = (page: Page) => page.evaluate(() => (window as any).owner.snapshot);
async function start(page: Page) {
  const id = await page.evaluate(() => crypto.randomUUID());
  expect(await page.evaluate(id => (window as any).send('start', id), id)).toMatchObject({ outcome: 'accepted' });
  await expect.poll(async () => (await snapshot(page)).state).toBe('capturing');
  return id;
}
async function chunk(page: Page, id: string) {
  await expect.poll(() => page.evaluate(async id => (await (window as any).owner.capture.store.get(id))?.byte_length ?? 0, id)).toBeGreaterThan(0);
}

test('one stream and one submission after committed final audio; duplicate and competing commands', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => {
    const w = window as any, original = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    w.streamCount = 0; navigator.mediaDevices.getUserMedia = async constraints => { w.streamCount++; return original(constraints); };
    w.first = w.command('start', crypto.randomUUID());
  });
  expect(await page.evaluate(() => (window as any).owner.dispatch((window as any).first))).toMatchObject({ outcome: 'accepted' });
  expect(await page.evaluate(() => (window as any).owner.dispatch((window as any).first))).toMatchObject({ outcome: 'duplicate' });
  expect(await page.evaluate(() => (window as any).send('start', crypto.randomUUID()))).toMatchObject({ outcome: 'rejected', error_code: 'busy' });
  await expect.poll(async () => (await snapshot(page)).state).toBe('capturing');
  const id = (await snapshot(page)).session_id;
  await chunk(page, id);
  await page.evaluate(() => { void (window as any).send('stop'); return (window as any).send('stop'); });
  await expect.poll(async () => (await snapshot(page)).state).toBe('success');
  const evidence = await page.evaluate(async () => {
    const w = window as any;
    return { streams: w.streamCount, requests: w.requests, uploads: w.uploads, metadata: await w.owner.capture.store.get(w.owner.snapshot.session_id), events: w.events };
  });
  expect(evidence.events.some((e: any) => e.state === 'saved')).toBe(false);
  expect(evidence.streams).toBe(1); expect(evidence.requests).toHaveLength(1);
  expect(evidence.uploads[0]).toMatchObject({ size: evidence.metadata.byte_length, capture: 'complete' });
  expect(evidence.events.filter((e: any) => e.kind === 'snapshot').map((e: any) => e.state)).toEqual(expect.arrayContaining(['requesting', 'capturing', 'saving', 'success']));
});

test('permission cancellation releases a late stream and cannot affect a newer session or settings', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => {
    const w = window as any, original = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getUserMedia = async constraints => {
      const stream = await original(constraints); w.lateStream = stream;
      navigator.mediaDevices.getUserMedia = original;
      return new Promise<MediaStream>(resolve => { w.permission = () => resolve(stream); });
    };
    void w.send('start', crypto.randomUUID());
  });
  await expect.poll(() => page.evaluate(() => !!(window as any).permission)).toBe(true);
  expect((await snapshot(page)).state).toBe('requesting');
  await page.evaluate(() => (window as any).send('stop'));
  expect(await snapshot(page)).toMatchObject({ state: 'canceled', audio_saved: false });
  const id = await start(page);
  await page.evaluate(() => { (window as any).permission(); (window as any).settings.options.selection.model = 'changed-after-start'; });
  await chunk(page, id);
  expect(await page.evaluate(() => (window as any).lateStream.getTracks().every((track: MediaStreamTrack) => track.readyState === 'ended'))).toBe(true);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('success');
  expect(await page.evaluate(() => (window as any).requests[0].options.selection.model)).not.toBe('changed-after-start');
});

test('cancel drains capture, submits nothing and discards the audio', async ({ page }) => {
  await setup(page);
  const id = await start(page); await chunk(page, id);
  await page.evaluate(() => (window as any).send('cancel'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('canceled');
  expect((await snapshot(page)).audio_saved).toBe(false);
  expect(await page.evaluate(() => (window as any).requests.length)).toBe(0);
  // The drain still finishes before the delete, so nothing is torn down
  // mid-write; the recording is then gone rather than left in history.
  await expect.poll(() => page.evaluate(async id => (await (window as any).owner.capture.store.get(id)) ?? null, id)).toBeNull();
});

test('cancel during a delayed final write waits for commit and suppresses submission', async ({ page }) => {
  await setup(page); const id = await start(page); await chunk(page, id);
  await page.evaluate(() => {
    const w = window as any, store = w.owner.capture.store, append = store.append.bind(store);
    store.append = async (...args: any[]) => { await new Promise(resolve => { w.releaseWrite = resolve; }); return append(...args); };
    void w.send('stop');
  });
  await expect.poll(() => page.evaluate(() => !!(window as any).releaseWrite)).toBe(true);
  expect((await snapshot(page)).state).toBe('saving');
  await page.evaluate(() => { void (window as any).send('cancel'); (window as any).releaseWrite(); });
  await expect.poll(async () => (await snapshot(page)).state).toBe('canceled');
  expect(await page.evaluate(() => (window as any).requests.length)).toBe(0);
});

test('uncertain submission reuses the exact key/options; terminal retry preserves raw history and rejects an evicted duplicate', async ({ page }) => {
  await setup(page); await page.evaluate(() => { (window as any).submitMode = 'lost'; });
  const id = await start(page); await chunk(page, id);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('error');
  const first = await page.evaluate(() => (window as any).requests[0]);
  await page.evaluate(() => { (window as any).settings.options.selection.model = 'new-setting'; return (window as any).send('retry'); });
  await expect.poll(async () => (await snapshot(page)).state).toBe('success');
  expect(await page.evaluate(() => (window as any).requests[1])).toEqual(first);
  const retry = await page.evaluate(() => { const w = window as any; w.retryCommand = w.command('retry'); return w.retryCommand; });
  await page.evaluate(command => (window as any).owner.dispatch(command), retry);
  await expect.poll(() => page.evaluate(() => (window as any).requests.length)).toBe(3);
  await expect.poll(async () => (await snapshot(page)).state).toBe('success');
  const attempt = await page.evaluate(id => (window as any).owner.jobs.get(id), id);
  expect(attempt.history).toHaveLength(1);
  expect(attempt.history[0].status.result.transcript.raw_text).toBe(fixtures.job.data.status.result.transcript.raw_text);
  expect(attempt.request.retry_of_job_id).toBe(attempt.history[0].id);
  await page.evaluate(async () => { for (let i = 0; i < 260; i++) await (window as any).send('snapshot'); });
  expect(await page.evaluate(command => (window as any).owner.dispatch(command), retry)).toMatchObject({ outcome: 'duplicate' });
  expect(await page.evaluate(() => (window as any).requests.length)).toBe(3);
});

test('known running jobs are observed, and late completion after cancel only updates retained history', async ({ page }) => {
  await setup(page); await page.evaluate(() => { (window as any).submitMode = 'queued'; });
  const old = await start(page); await chunk(page, old);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('queued');
  expect(await page.evaluate(() => (window as any).send('retry'))).toMatchObject({ outcome: 'rejected', error_code: 'busy' });
  await page.evaluate(() => { (window as any).pollState = 'transcribing'; });
  await expect.poll(async () => (await snapshot(page)).state).toBe('transcribing');
  await page.evaluate(() => (window as any).send('cancel'));
  const next = await start(page);
  await page.evaluate(() => { (window as any).pollState = 'success'; });
  await expect.poll(() => page.evaluate(id => (window as any).owner.jobs.get(id)?.job?.status.state, old)).toBe('succeeded');
  expect(await snapshot(page)).toMatchObject({ state: 'capturing', session_id: next });
  expect(await page.evaluate(() => (window as any).requests.length)).toBe(1);
  await page.evaluate(() => (window as any).owner.dispose());
});

test('capture write failure and device disconnect release resources and retain the prefix', async ({ page }) => {
  await setup(page); const id = await start(page); await chunk(page, id);
  await page.evaluate(() => { (window as any).owner.capture.store.append = async () => { throw new DOMException('fixture', 'QuotaExceededError'); }; });
  await expect.poll(async () => (await snapshot(page)).state).toBe('error');
  expect(await snapshot(page)).toMatchObject({ audio_saved: true });
  expect(await page.evaluate(() => (window as any).owner.capture.snapshot.state)).toBe('idle');
  expect(await page.evaluate(() => (window as any).requests.length)).toBe(0);
  await setup(page); const next = await start(page); await chunk(page, next);
  await page.evaluate(() => (window as any).owner.capture.activeStream.getAudioTracks()[0].dispatchEvent(new Event('ended')));
  await expect.poll(async () => (await snapshot(page)).state).toBe('error');
  expect((await snapshot(page)).audio_saved).toBe(true);
});

test('real audio-thread telemetry distinguishes amplitude and silence and stops after capture', async ({ page }) => {
  await setup(page, false);
  await page.evaluate(() => {
    const w = window as any; const context = new AudioContext(); const oscillator = context.createOscillator();
    const gain = context.createGain(); gain.gain.value = 0.4;
    const output = context.createMediaStreamDestination(); oscillator.connect(gain); gain.connect(output); oscillator.start();
    w.audioFixture = { context, oscillator, gain, stream: output.stream };
    navigator.mediaDevices.getUserMedia = async () => { await context.resume(); return output.stream; };
  });
  await start(page);
  await expect.poll(() => page.evaluate(() => (window as any).events.filter((e: any) => e.kind === 'level' && e.rms > 0.2).length)).toBeGreaterThan(3);
  await page.evaluate(() => { (window as any).audioFixture.gain.gain.value = 0; });
  await expect.poll(() => page.evaluate(() => (window as any).events.filter((e: any) => e.kind === 'level').slice(-1)[0]?.peak)).toBe(0);
  const levels = await page.evaluate(() => (window as any).events.filter((e: any) => e.kind === 'level'));
  for (let i = 1; i < levels.length; i++) expect(levels[i].at - levels[i - 1].at).toBeGreaterThanOrEqual(48);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('saved');
  const count = await page.evaluate(() => (window as any).events.filter((e: any) => e.kind === 'level').length);
  await page.waitForTimeout(180);
  expect(await page.evaluate(() => (window as any).events.filter((e: any) => e.kind === 'level').length)).toBe(count);
  expect(await page.evaluate(() => (window as any).audioFixture.stream.getTracks().every((t: MediaStreamTrack) => t.readyState === 'ended'))).toBe(true);
});

test('new owner restores interrupted retained audio and rejects old epochs and reused start IDs', async ({ page }) => {
  await setup(page, false); const id = await start(page); await chunk(page, id);
  const old = await snapshot(page);
  await setup(page, false); // document recreation closes streams; no dispose/final chunk
  expect(await snapshot(page)).toMatchObject({ session_id: id, state: 'saved', audio_saved: true });
  const metadata = await page.evaluate(async id => (await (window as any).owner.capture.list()).find((s: any) => s.id === id), id);
  expect(metadata.capture_state).toBe('interrupted');
  expect(await page.evaluate(old => (window as any).owner.dispatch({ version: 1, kind: 'command', epoch: old.epoch, action: 'stop', session_id: old.session_id, command_id: crypto.randomUUID() }), old)).toMatchObject({ outcome: 'rejected', error_code: 'stale_epoch' });
  await page.evaluate(id => (window as any).send('start', id), id);
  await expect.poll(async () => (await snapshot(page)).error_code).toBe('session_exists');
  expect(await page.evaluate(async id => (await (window as any).owner.capture.exportRecording(id)).size, id)).toBeGreaterThan(0);
});

test('owner creation joins, permission denial is honest, and canceled start receipts survive recreation', async ({ page }) => {
  await setup(page, false);
  expect(await page.evaluate(async () => {
    const path = '/src/features/desktop-session/runtime/owner.ts'; const { SessionOwner } = await import(path);
    return await SessionOwner.create({ settings: () => ({ language: 'he', options: null }) }) === (window as any).owner;
  })).toBe(true);
  await page.evaluate(() => { navigator.mediaDevices.getUserMedia = async () => { throw new DOMException('fixture denial', 'NotAllowedError'); }; });
  const id = await page.evaluate(() => crypto.randomUUID());
  await page.evaluate(id => (window as any).send('start', id), id);
  await expect.poll(async () => (await snapshot(page)).state).toBe('error');
  expect((await snapshot(page)).audio_saved).toBe(false);
  expect(await page.evaluate(() => (window as any).owner.capture.snapshot.error)).toContain('denied');
  await setup(page, false);
  await page.evaluate(id => (window as any).send('start', id), id);
  await expect.poll(async () => (await snapshot(page)).error_code).toBe('session_exists');
});

test('cancel while submission acknowledgement is pending allows a new session without losing the old result', async ({ page }) => {
  await setup(page); await page.evaluate(() => { (window as any).submitMode = 'deferred'; });
  const id = await start(page); await chunk(page, id);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(() => page.evaluate(() => !!(window as any).releaseSubmit)).toBe(true);
  await page.evaluate(() => (window as any).send('cancel'));
  expect((await snapshot(page)).state).toBe('canceled');
  const next = await start(page);
  await page.evaluate(() => { (window as any).pollState = 'success'; (window as any).releaseSubmit(); });
  await expect.poll(() => page.evaluate(id => (window as any).owner.jobs.get(id)?.job?.status.state, id)).toBe('succeeded');
  expect(await snapshot(page)).toMatchObject({ state: 'capturing', session_id: next });
  await page.evaluate(() => (window as any).owner.dispose());
});

test('uncertain request survives reload and keeps its original options', async ({ page }) => {
  await setup(page); await page.evaluate(() => { (window as any).submitMode = 'lost'; });
  const id = await start(page); await chunk(page, id);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('error');
  const request = await page.evaluate(() => (window as any).requests[0]);
  await setup(page);
  await page.evaluate(() => { (window as any).settings.options.selection.model = 'changed-after-reload'; return (window as any).send('retry'); });
  await expect.poll(async () => (await snapshot(page)).state).toBe('success');
  expect(await page.evaluate(() => (window as any).requests[0])).toEqual(request);
});

test('failure to persist a completed response retries the write without repeating submission', async ({ page }) => {
  await setup(page); const id = await start(page); await chunk(page, id);
  await page.evaluate(() => {
    const w = window as any, save = w.owner.jobs.dependencies.save; let once = true;
    w.owner.jobs.dependencies.save = async (attempt: any) => {
      if (once && attempt.job) { once = false; throw Error('fixture storage unavailable'); }
      return save(attempt);
    };
  });
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('success');
  expect(await page.evaluate(() => (window as any).requests.length)).toBe(1);
  expect(await page.evaluate(() => (window as any).events.some((e: any) => e.state === 'error'))).toBe(true);
});

test('stale server identity cannot become a completion and the legacy origin cannot install the control bridge', async ({ page }) => {
  await setup(page); const id = await start(page); await chunk(page, id);
  await page.evaluate(() => {
    const w = window as any, submit = w.owner.jobs.dependencies.submit;
    w.owner.jobs.dependencies.submit = async (request: any) => {
      const job = await submit(request); job.request.recording_id = crypto.randomUUID(); return job;
    };
  });
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('error');
  expect(await page.evaluate(id => (window as any).owner.jobs.get(id)?.job, id)).toBeUndefined();
  expect(await page.evaluate(async () => {
    const path = '/src/features/desktop-session/runtime/bridge.ts';
    try { (await import(path)).installOwnerBridge((window as any).owner); return ''; } catch (e) { return (e as Error).message; }
  })).toBe('untrusted_origin');
});

test('automatic delivery runs once for live completion and never for a history retry', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => {
    const w=window as any; w.deliveries=[];
    w.owner.dependencies.deliver=async (job:any) => {w.deliveries.push(job.id);return {copied:true,pasted:true,message:'Pasted'};};
  });
  const id=await start(page); await chunk(page,id);
  await page.evaluate(()=> (window as any).send('stop'));
  await expect.poll(async()=> (await snapshot(page)).state).toBe('success');
  await expect.poll(()=>page.evaluate(()=>(window as any).deliveries.length)).toBe(1);
  await page.evaluate(async()=>{const w=window as any; const job=w.owner.jobs.get(w.owner.snapshot.session_id).job; await w.owner.jobs.updateJob(job);});
  expect(await page.evaluate(()=>(window as any).deliveries.length)).toBe(1);
  await page.evaluate(()=>(window as any).send('retry'));
  await expect.poll(()=>page.evaluate(()=>(window as any).requests.length)).toBe(2);
  expect(await page.evaluate(()=>(window as any).deliveries.length)).toBe(1);
});

test('preview cancellation ignores late provisional output and never runs cloud inference', async ({ page }) => {
  await page.route('**/preview-test', route => route.fulfill({contentType:'text/html',body:'<!doctype html><title>Preview lifecycle test</title>'}));
  await page.goto('/preview-test');
  await page.evaluate(async()=> {
    const path='/src/features/desktop-session/runtime/preview.ts'; const {LocalPreview}=await import(path);
    const w=window as any; w.previews=[];w.previewCalls=0;
    w.previewRunner=new LocalPreview((text:string)=>w.previews.push(text), async (_id:string,_opts:any, signal:AbortSignal)=>{w.previewCalls++;w.previewSignal=signal; return new Promise(r=>{w.releasePreview=r;});},()=>true);
  });
  const options={...fixtures.transcription_request.data.options,selection:{provider:'local',model:'whisper-tiny'}};
  await page.evaluate(options=>(window as any).previewRunner.start(crypto.randomUUID(),options),options);
  await expect.poll(()=>page.evaluate(()=>(window as any).previewCalls)).toBe(1);
  await page.evaluate(()=>{const w=window as any;w.previewRunner.stop();w.releasePreview('late text');});
  expect(await page.evaluate(()=>(window as any).previewSignal.aborted)).toBe(true);
  expect(await page.evaluate(()=>(window as any).previews)).toEqual(['']);
  await page.evaluate(options=>(window as any).previewRunner.start(crypto.randomUUID(),options),fixtures.transcription_request.data.options);
  await page.waitForTimeout(3200);
  expect(await page.evaluate(()=>(window as any).previewCalls)).toBe(1);
});

test('completed native job events survive an in-flight write without waiting for a polling timer', async ({ page }) => {
  await setup(page);
  await page.evaluate(()=>{const w=window as any;w.submitMode='queued';});
  const id=await start(page);await chunk(page,id);
  await page.evaluate(()=>(window as any).send('stop'));
  await expect.poll(async()=> (await snapshot(page)).state).toBe('queued');
  await page.evaluate(async fixture=> {
    const w=window as any;const jobs=w.owner.jobs;
    clearTimeout(jobs.timer); jobs.dependencies.pollMs=60000;
    const id = w.owner.snapshot.session_id;
    await jobs.locks.get(id);
    const completed={...jobs.get(id).job,status:fixture.job.data.status};
    const lock = new Promise<void>(resolve => { w.releaseWrite = resolve; }).finally(() => jobs.locks.delete(id));
    jobs.locks.set(id, lock);
    jobs.receiveNative(completed);
  },fixtures);
  expect((await snapshot(page)).state).toBe('queued');
  await page.evaluate(() => (window as any).releaseWrite());
  await expect.poll(async()=> (await snapshot(page)).state).toBe('success');
});

test('native capture events start previews when timers are suspended without overlapping inference', async ({ page }) => {
  await page.route('**/preview-test', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Preview event test</title>' }));
  await page.goto('/preview-test');
  await page.clock.install({ time: new Date('2026-09-08T12:00:00Z') });
  await page.clock.pauseAt(new Date('2026-09-08T12:00:00Z'));
  const options = { ...fixtures.transcription_request.data.options, selection: { provider: 'local', model: 'whisper-tiny' } };
  await page.evaluate(async options => {
    const w = window as any;
    w.__TAURI__ = { event: { listen: async (_name: string, cb: any) => { w.tick = cb; return () => { w.unlistened = true; }; } } };
    const path = '/src/features/desktop-session/runtime/preview.ts'; const { LocalPreview } = await import(path);
    w.previewCalls = 0;
    w.previewRunner = new LocalPreview(() => {}, async () => { w.previewCalls++; return new Promise(() => {}); }, () => true);
    w.previewRunner.start('active', options);
  }, options);
  await page.clock.setSystemTime(new Date('2026-09-08T12:00:03Z'));
  await page.evaluate(() => {
    const w = window as any;
    w.tick({ payload: { id: 'old', active: true } });
    if (w.previewCalls !== 0) throw new Error('Wrong session triggered inference');
    w.tick({ payload: { id: 'active', active: true } });
    w.tick({ payload: { id: 'active', active: true } });
  });
  expect(await page.evaluate(() => (window as any).previewCalls)).toBe(1);
  await page.evaluate(() => (window as any).previewRunner.stop());
  expect(await page.evaluate(() => (window as any).unlistened)).toBe(true);
});

test('native events also consume finished preview results while status timers are suspended', async ({ page }) => {
  await page.route('**/preview-test', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Preview status test</title>' }));
  await page.route('**/native/preview-audio/active', route => route.fulfill({ body: 'synthetic fixture', contentType: 'audio/wav' }));
  await page.route('**/api/v1/transcriptions/preview', route => route.fulfill({ status: 202, json: { id: 'task' } }));
  await page.route('**/api/v1/transcriptions/preview/task', route => route.fulfill({ json: { provisional: true, text: 'fixture preview' } }));
  await page.goto('/preview-test');
  await page.clock.install({ time: new Date('2026-09-08T12:00:00Z') });
  await page.clock.pauseAt(new Date('2026-09-08T12:00:00Z'));
  const options = { ...fixtures.transcription_request.data.options, selection: { provider: 'local', model: 'whisper-tiny' } };
  await page.evaluate(async options => {
    const w = window as any; w.ticks = new Set();
    w.__TAURI__ = { event: { listen: async (_name: string, cb: any) => { w.ticks.add(cb); return () => w.ticks.delete(cb); } } };
    const path = '/src/features/desktop-session/runtime/preview.ts'; const { LocalPreview } = await import(path);
    w.previewRunner = new LocalPreview((text: string) => { w.previewText = text; }, undefined, () => true);
    w.previewRunner.start('active', options);
  }, options);
  await page.clock.setSystemTime(new Date('2026-09-08T12:00:03Z'));
  const tick = () => page.evaluate(() => { for (const cb of (window as any).ticks) cb({ payload: { id: 'active', active: true } }); });
  await tick();
  await expect.poll(() => page.evaluate(() => (window as any).ticks.size)).toBe(2);
  await page.clock.setSystemTime(new Date('2026-09-08T12:00:04Z'));
  await tick();
  await expect.poll(() => page.evaluate(() => (window as any).previewText)).toBe('fixture preview');
  await page.evaluate(() => (window as any).previewRunner.stop());
  expect(await page.evaluate(() => (window as any).ticks.size)).toBe(0);
});

for (const readiness of ['during capture', 'after saving'] as const) {
  test(`first recording transcribes when startup settings arrive ${readiness}`, async ({ page }) => {
    await setup(page, false);
    await page.evaluate(() => { (window as any).settings.loading = true; });
    const id = await start(page);
    await chunk(page, id);
    if (readiness === 'after saving') {
      await page.evaluate(() => (window as any).send('stop'));
      await expect.poll(async () => (await snapshot(page)).state).toBe('saving');
      expect(await page.evaluate(() => (window as any).requests.length)).toBe(0);
    }
    await page.evaluate(options => {
      const w = window as any;
      w.settings = { language: 'he', options: { ...options, input_language: 'he' }, loading: false };
      w.owner.settingsLoaded();
      // Later UI changes cannot alter the first completed initialization snapshot.
      w.settings = { language: 'en', options: { ...options, input_language: 'en' } };
      w.owner.settingsLoaded();
    }, fixtures.transcription_request.data.options);
    if (readiness === 'during capture') await page.evaluate(() => (window as any).send('stop'));
    await expect.poll(async () => (await snapshot(page)).state).toBe('success');
    const requests = await page.evaluate(() => (window as any).requests);
    expect(requests).toHaveLength(1);
    expect(requests[0]).toMatchObject({ recording_id: id, options: { input_language: 'he' } });
  });
}

test('initially unconfigured and discarded recordings do not auto-submit on later setup', async ({ page }) => {
  await setup(page, false);
  const saved = await start(page); await chunk(page, saved);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('saved');
  await page.evaluate(() => { (window as any).settings.loading = true; });
  const canceled = await start(page); await chunk(page, canceled);
  await page.evaluate(() => (window as any).send('cancel'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('canceled');
  await page.evaluate(options => {
    const w = window as any; w.settings = { language: 'en', options, loading: false }; w.owner.settingsLoaded();
  }, fixtures.transcription_request.data.options);
  expect(await page.evaluate(() => (window as any).requests)).toHaveLength(0);
  expect(await page.evaluate(async id => (await (window as any).owner.capture.store.get(id))?.byte_length, saved)).toBeGreaterThan(0);
});

test('startup settings arriving after a newer recording preserve both saved sessions', async ({ page }) => {
  await setup(page, false);
  await page.evaluate(() => { (window as any).settings.loading = true; });
  const first = await start(page); await chunk(page, first);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('saving');
  const second = await start(page); await chunk(page, second);
  await page.evaluate(options => {
    const w = window as any; w.settings = { language: 'en', options, loading: false }; w.owner.settingsLoaded();
  }, fixtures.transcription_request.data.options);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('success');
  await expect.poll(() => page.evaluate(() => (window as any).requests.length)).toBe(2);
  expect(await page.evaluate(() => (window as any).requests.map((r: any) => r.recording_id))).toEqual([first, second]);
});

test('automatic recognition retries show progress and deliver the same live job once', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => {
    const w = window as any;
    w.submitMode = 'queued'; w.deliveries = [];
    w.owner.dependencies.deliver = async (job: any) => { w.deliveries.push(job.id); return { copied: true, pasted: true, message: 'Pasted' }; };
  });
  const id = await start(page); await chunk(page, id);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('queued');
  await page.evaluate(() => {
    const w = window as any, job: any = Object.values(w.server)[0];
    job.recovery = { attempts_started: 1, failures: [], canceled: false, waiting_for_capacity: false, next_retry_at_ms: Date.now() + 2000 };
  });
  await expect.poll(async () => (await snapshot(page)).progress).toBe('retrying_1');
  expect(await page.evaluate(() => (window as any).send('retry'))).toMatchObject({ outcome: 'rejected', error_code: 'busy' });
  await page.evaluate(() => {
    const w = window as any, job: any = Object.values(w.server)[0];
    job.recovery.attempts_started = 2; job.recovery.next_retry_at_ms = null; w.pollState = 'transcribing';
  });
  await expect.poll(async () => (await snapshot(page)).state).toBe('transcribing');
  await expect.poll(async () => (await snapshot(page)).progress).toBe('retrying_1');
  await page.evaluate(() => { (window as any).pollState = 'success'; });
  await expect.poll(async () => (await snapshot(page)).state).toBe('success');
  expect(await page.evaluate(() => (window as any).requests.length)).toBe(1);
  expect(await page.evaluate(() => (window as any).deliveries.length)).toBe(1);
  const job = await page.evaluate(id => (window as any).owner.jobs.get(id).job, id);
  await page.evaluate(async job => (window as any).owner.jobs.updateJob(job), job);
  expect(await page.evaluate(() => (window as any).deliveries.length)).toBe(1);
  expect((await snapshot(page)).progress).toBeUndefined();
});

test('an explicit retry of retained canceled audio can start a new attempt', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => {
    const w = window as any;
    w.submitMode = 'queued';
    w.owner.capture.deleteRecording = async () => { throw Error('Fixture keeps canceled audio'); };
  });
  const id = await start(page); await chunk(page, id);
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(async () => (await snapshot(page)).state).toBe('queued');
  await page.evaluate(() => (window as any).send('cancel'));
  await expect.poll(() => page.evaluate(() => (window as any).cancelRequests.length)).toBe(1);
  await page.evaluate(() => {
    const job: any = Object.values((window as any).server)[0];
    job.status = { state: 'failed', error: { code: 'interrupted', message: 'Canceled fixture', retryable: false } };
  });
  await expect.poll(() => page.evaluate(id => (window as any).owner.jobs.get(id)?.job?.status.state, id)).toBe('failed');
  expect(await page.evaluate(() => (window as any).send('retry'))).toMatchObject({ outcome: 'accepted' });
  await expect.poll(() => page.evaluate(() => (window as any).requests.length)).toBe(2);
  await expect.poll(async () => (await snapshot(page)).state).toBe('queued');
  const requests = await page.evaluate(() => (window as any).requests);
  expect(requests[1].recording_id).toBe(id);
  expect(requests[1].idempotency_key).not.toBe(requests[0].idempotency_key);
});
