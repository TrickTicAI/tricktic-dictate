import { expect, test, type Page } from '../../apps/web/node_modules/@playwright/test';
import type { Job, Recording, TranscriptionRequest } from '../../packages/contracts';
import type { ModeProfile } from '../../apps/web/src/features/modes/api';
const envelope = (data: unknown) => ({ api_version: '1', data });
async function fixture(page: Page, configured = true) {
  const recordings = new Map<string, Recording>();
  const jobs = new Map<string, Job>();
  const requests: TranscriptionRequest[] = [];
  let fail = true;
  let uncertainUpload = false;
  let uncertainSubmit = false;
  let uploads = 0;
  let activeMode: ModeProfile | undefined;
  const capturedModes: ModeProfile[] = [];
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/v1/session') return route.fulfill({ json: envelope({ token: 'a'.repeat(64), expires_in_seconds: 43200 }) });
    if (path.endsWith('/health')) return route.fulfill({ json: { status: 'ok' } });
    if (path.endsWith('/live-processing')) return route.fulfill({ json: envelope({ bound: false, attempt: null }) });
    if (path.endsWith('/processing')) return route.fulfill({ json: envelope([]) });
    if (path.endsWith('/providers')) return route.fulfill({ json: path.includes('/cleanup/') ? envelope([]) : { providers: [{ id: 'openai', name: 'OpenAI', configured, available: false, status: configured ? 'unverified' : 'not_configured' }] } });
    if (path.endsWith('/vocabulary')) return route.fulfill({ json: envelope({ hints: [], corrections: [], snippets: [] }) });
    if (path === '/api/v1/modes') return route.fulfill({ json: envelope({ revision: 1, active: activeMode?.id ?? 'regular', profiles: activeMode ? [activeMode] : [] }) });
    if (path.startsWith('/api/v1/recordings/') && path.endsWith('/mode')) {
      capturedModes.push(route.request().postDataJSON().data.profile);
      return route.fulfill({ json: envelope({ profile: capturedModes.at(-1) }) });
    }
    if (path === '/api/v1/recordings') {
      uploads++;
      const body = route.request().postDataBuffer()!.toString();
      expect(body.indexOf('name="metadata"')).toBeLessThan(body.indexOf('name="audio"'));
      const metadata = JSON.parse(body.split('name="metadata"\r\n\r\n')[1].split('\r\n--')[0]);
      expect(metadata.api_version).toBe('1');
      expect(Object.keys(metadata.data).sort()).toEqual(['id', 'created_at_ms', 'duration_ms', 'byte_length', 'mime_type', 'input_language', 'capture_state'].sort());
      const recording: Recording = { metadata: metadata.data, storage: 'self_hosted_server', audio: { state: 'saved' } };
      recordings.set(recording.metadata.id, recording);
      if (uncertainUpload) { uncertainUpload = false; return route.fulfill({ status: 409, json: { api_version: '1', error: { code: 'idempotency_conflict', message: 'Already retained', retryable: false } } }); }
      return route.fulfill({ status: 201, json: envelope(recording) });
    }
    if (path.startsWith('/api/v1/recordings/')) {
      const recording = recordings.get(path.split('/').at(-1)!);
      return route.fulfill({ status: recording ? 200 : 404, json: recording ? envelope(recording) : { api_version: '1', error: { code: 'recording_not_found', message: 'Not found', retryable: false } } });
    }
    if (path === '/api/v1/transcriptions') {
      const request = route.request().postDataJSON().data as TranscriptionRequest;
      requests.push(request);
      let job = [...jobs.values()].find(item => item.request.idempotency_key === request.idempotency_key);
      if (!job) {
        job = { id: crypto.randomUUID(), request, created_at_ms: Date.now(), updated_at_ms: Date.now(), status: fail ? { state: 'failed', error: { code: 'provider_failed', message: 'Fixture provider failed; saved audio can be retried.', retryable: true } } : { state: 'succeeded', result: { cleanup: { state: 'off' }, transcript: { raw_text: 'שלום Metrix — fixture transcript', input_language: request.options.input_language, output_language: request.options.output_language, selection: request.options.selection, execution_location: 'cloud' } } } };
        jobs.set(job.id, job);
      }
      if (uncertainSubmit) { uncertainSubmit = false; return route.abort(); }
      return route.fulfill({ status: 202, json: envelope({ ...job, status: { state: 'queued' } }) });
    }
    if (path.endsWith('/processed')) return route.fulfill({ json: envelope({ text: 'שלום Metrix — fixture transcript' }) });
    if (path.startsWith('/api/v1/jobs/')) return route.fulfill({ json: envelope(jobs.get(path.split('/').at(-1)!)) });
    return route.fulfill({ status: 404, body: 'Fixture route unavailable' });
  });
  return { requests, recordings, jobs, capturedModes, mode: (profile: ModeProfile) => { activeMode = profile; }, uploads: () => uploads, succeed: () => { fail = false; }, fail: () => { fail = true; }, uncertainUpload: () => { uncertainUpload = true; }, uncertainSubmit: () => { uncertainSubmit = true; } };
}
async function login(page: Page) {
  await page.getByLabel('Owner key').fill('b'.repeat(64));
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Sign out', exact: true })).toBeVisible();
}
/* An empty Home leads with its first-dictation button; afterwards the dock records. */
const recordButton = (page: Page) => page.getByRole('button', { name: /^(Record|Try a dictation here)$/ });
const reader = (page: Page) => page.getByRole('region', { name: 'Transcript', exact: true });
const transcribeAgain = (page: Page) => reader(page).getByRole('button', { name: 'Transcribe again', exact: true });
async function record(page: Page) {
  await recordButton(page).click();
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  await expect(page.getByLabel('Recording duration')).not.toHaveText('00:00');
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Record', exact: true })).toBeEnabled();
}
const transcript = (page: Page) => page.getByRole('group', { name: 'As spoken transcript' });
const failureNotice = /recording is saved — nothing is lost\./;
test('missing credentials retains playable audio across reload without upload', async ({ page }) => {
  const server = await fixture(page, false); await page.goto('/'); await login(page); await record(page);
  await expect(page.locator('.session-row')).toHaveCount(1);
  await page.reload(); await login(page); await expect(page.locator('.session-row')).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Transcribe', exact: true })).toBeDisabled();
  const audio = page.locator('audio');
  await expect.poll(() => audio.evaluate((element: HTMLAudioElement) => element.readyState)).toBeGreaterThan(0);
  await audio.evaluate((element: HTMLAudioElement) => element.play());
  await expect.poll(() => audio.evaluate((element: HTMLAudioElement) => element.currentTime)).toBeGreaterThan(0);
  const download = page.waitForEvent('download'); await page.getByRole('button', { name: 'Save audio', exact: true }).click();
  expect(await (await download).failure()).toBeNull(); expect(server.uploads()).toBe(0);
});
test('unverified cloud attempts, failure reload retry and previous originals survive', async ({ page }) => {
  const server = await fixture(page); await page.goto('/'); await login(page); await record(page);
  await expect(page.getByText(failureNotice)).toBeVisible();
  const id = server.requests[0].recording_id; expect(server.uploads()).toBe(1);
  await page.reload(); await login(page); await expect(page.getByText(failureNotice)).toBeVisible();
  server.succeed(); await transcribeAgain(page).click();
  await expect(transcript(page)).toHaveText('שלום Metrix — fixture transcript');
  expect(server.requests[1].recording_id).toBe(id); expect(server.requests[1].retry_of_job_id).toBe([...server.jobs.keys()][0]); expect(server.uploads()).toBe(1);
  await reader(page).getByRole('button', { name: 'Copy', exact: true }).click();
  await expect(reader(page).getByRole('button', { name: 'Copied', exact: true })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('שלום Metrix — fixture transcript');
  server.fail(); await transcribeAgain(page).click();
  await expect(page.getByText(failureNotice).first()).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Earlier attempts (2)' })).toBeVisible();
  await expect(page.locator('.attempt .transcript-text').first()).toHaveText('שלום Metrix — fixture transcript');
  await expect(page.locator('.session-row')).toHaveCount(1);
});
test('lost submit response reuses exact idempotency payload after reload and resolves upload conflict by ID', async ({ page }) => {
  const server = await fixture(page); server.succeed(); server.uncertainUpload(); server.uncertainSubmit();
  await page.goto('/'); await login(page); await record(page); await expect.poll(() => server.requests.length).toBe(1);
  await expect(transcribeAgain(page)).toBeEnabled(); await page.reload(); await login(page);
  await expect(page.getByText('Submission was not acknowledged.', { exact: false })).toBeVisible();
  await transcribeAgain(page).click();
  await expect(transcript(page)).toHaveText('שלום Metrix — fixture transcript');
  expect(server.requests[1]).toEqual(server.requests[0]); expect(server.uploads()).toBe(1); expect(server.jobs.size).toBe(1);
});
test('older session selection changes the sole playback URL and retry source', async ({ page }) => {
  const server = await fixture(page); server.succeed(); await page.goto('/'); await login(page); await record(page);
  await expect(transcript(page)).toHaveText('שלום Metrix — fixture transcript'); const first = server.requests[0].recording_id;
  await record(page); await expect(page.locator('.session-row')).toHaveCount(2); await expect.poll(() => server.requests.length).toBe(2);
  // The finished dictation takes over the reader.
  await expect(page.locator('.session-row').first()).toHaveClass(/current/);
  const latestUrl = await page.locator('audio').getAttribute('src');
  await page.locator('.session-row').last().click(); await expect(page.locator('audio')).toHaveCount(1);
  await expect(page.locator('audio')).not.toHaveAttribute('src', latestUrl!);
  await transcribeAgain(page).click(); await expect.poll(() => server.requests.length).toBe(3);
  expect(server.requests[2].recording_id).toBe(first); expect(server.uploads()).toBe(2);
});
test('unsupported translation stays unavailable while microphone capture remains usable', async ({ page }) => {
  const server = await fixture(page); server.succeed(); await page.goto('/'); await login(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const output = page.getByRole('combobox', { name: 'Output language' });
  await expect(output.locator('option[value="he"]')).toBeDisabled();
  await expect(output.locator('option[value="en"]')).toBeDisabled();
  await expect(output).toHaveValue('preserve');
  await page.keyboard.press('Escape'); await record(page);
  await expect(transcript(page)).toHaveText('שלום Metrix — fixture transcript');
  expect(server.requests[0].options.output_language).toEqual({ mode: 'preserve' });
});
test('microphone permission failure returns to idle and shows actionable error', async ({ page }) => {
  await fixture(page); await page.addInitScript(() => { navigator.mediaDevices.getUserMedia = async () => { throw new DOMException('Denied', 'NotAllowedError'); }; });
  await page.goto('/'); await login(page); await recordButton(page).click();
  await expect(page.getByRole('alert')).toContainText('Microphone access was denied');
  await expect(recordButton(page)).toBeEnabled();
});
test('API outage leaves recording enabled', async ({ page }) => {
  await fixture(page); await page.route('**/api/v1/health', route => route.fulfill({ status: 503, body: 'offline' }));
  await page.goto('/'); await login(page); await expect(page.getByText('Offline', { exact: true })).toBeVisible();
  await record(page); await expect(page.locator('.session-row')).toHaveCount(1);
});
test('interrupted server upload is replaced from saved browser audio on retry', async ({ page }) => {
  const server = await fixture(page); server.succeed(); let interrupted = true;
  await page.route('**/api/v1/recordings', async route => {
    if (!interrupted) return route.fallback();
    interrupted = false;
    const body = route.request().postDataBuffer()!.toString();
    const metadata = JSON.parse(body.split('name="metadata"\r\n\r\n')[1].split('\r\n--')[0]).data;
    server.recordings.set(metadata.id, { metadata, storage: 'self_hosted_server', audio: { state: 'uploading', received_bytes: 20 } });
    return route.abort();
  });
  await page.goto('/'); await login(page); await record(page);
  await expect(transcribeAgain(page)).toBeEnabled();
  await page.reload(); await login(page); await transcribeAgain(page).click();
  await expect(transcript(page)).toHaveText('שלום Metrix — fixture transcript');
  expect(server.recordings.size).toBe(1); expect(server.requests.length).toBe(1);
});
test('processed snippets and opt-in cleanup remain separate from original through cleanup failure', async ({ page }) => {
  const server = await fixture(page); server.succeed(); let cleanupCalls = 0;
  await page.route('**/api/v1/jobs/*/processed', route => route.fulfill({ json: envelope({ text: 'Writer.Name+Notes@example.com — exact fixture snippet' }) }));
  await page.route('**/api/v1/cleanup/providers', route => route.fulfill({ json: envelope([{ selection: { provider: 'openai', model: 'gpt-4.1-mini' }, configured: true, execution_location: 'cloud' }]) }));
  await page.route('**/api/v1/jobs/*/cleanup', route => {
    cleanupCalls++;
    const job = [...server.jobs.values()][0];
    if (job.status.state !== 'succeeded') throw Error('Expected successful fixture job');
    job.status.result.cleanup = { state: 'failed', error: { code: 'provider_failed', message: 'Fixture cleanup rejected; original unchanged.', retryable: true } };
    return route.fulfill({ status: 202, json: envelope(job) });
  });
  await page.goto('/'); await login(page); await record(page);
  await expect(transcript(page)).toHaveText('שלום Metrix — fixture transcript');
  await page.getByRole('group', { name: 'Transcript version' }).getByRole('button', { name: 'Corrected' }).click();
  await expect(page.getByRole('group', { name: 'Corrected transcript' })).toHaveText('Writer.Name+Notes@example.com — exact fixture snippet');
  await page.getByRole('group', { name: 'Transcript version' }).getByRole('button', { name: 'As spoken' }).click();
  expect(cleanupCalls).toBe(0);
  await reader(page).getByRole('button', { name: 'More actions' }).click();
  await page.getByRole('menuitem', { name: 'Clean up', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Fixture cleanup rejected');
  await expect(transcript(page)).toHaveText('שלום Metrix — fixture transcript');
  await page.reload(); await login(page); await expect(page.getByRole('alert')).toContainText('Fixture cleanup rejected');
  expect(cleanupCalls).toBe(1); await expect(page.locator('.session-row')).toHaveCount(1);
});

test('browser capture freezes primary and backup and restores that policy for a saved-audio retry', async ({ page }) => {
 const server = await fixture(page);
 const profile: ModeProfile = {id:'super',name:'Super',kind:'super',revision:1,instructions:'Keep both languages',text_model:{provider:'openai',model:'gpt-4.1-mini'},fallback_model:{provider:'gemini',model:'gemini-3.8-flash'},fallback_after_ms:1000,polish_timeout_ms:3000,context:{application:false,selected_text:false,clipboard:false}};
 server.mode(profile); await page.goto('/'); await login(page);
 await recordButton(page).click();
 await expect(page.getByRole('button',{name:'Stop',exact:true})).toBeVisible();
 await expect(page.getByLabel('Recording duration')).not.toHaveText('00:00');
 server.mode({...profile,revision:2,text_model:{provider:'openai',model:'gpt-5.6-luna'}});
 await page.getByRole('button',{name:'Stop',exact:true}).click();
 await expect(page.getByText(failureNotice)).toBeVisible();
 expect(server.capturedModes).toEqual([profile]);
 await page.reload(); await login(page); server.succeed();
 await transcribeAgain(page).click();
 await expect(transcript(page)).toHaveText('שלום Metrix — fixture transcript');
 expect(server.capturedModes).toEqual([profile,profile]);
 expect(server.requests).toHaveLength(2); expect(server.requests[0].recording_id).toBe(server.requests[1].recording_id);
});
