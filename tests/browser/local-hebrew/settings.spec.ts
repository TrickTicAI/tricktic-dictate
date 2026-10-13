import { stubOwnerSession, loginFixture } from '../../../apps/web/src/features/auth/tests/fixture';
import { expect, test } from '../../../apps/web/node_modules/@playwright/test';
import type { Job, Recording, TranscriptionRequest } from '../../../packages/contracts';

// Authored outputs and synthetic microphone: these tests prove routing, not accuracy.
test('saved audio crosses local/cloud and English/Hebrew/mixed selections without upload or rerecording', async ({ page }) => {
  const requests: TranscriptionRequest[] = []; const jobs = new Map<string, Job>(); const recordings = new Map<string, Recording>(); let uploads = 0;
  const envelope = (data: unknown) => ({ api_version: '1', data });
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith('/health')) return route.fulfill({ json: { status: 'ok' } });
    if (path.endsWith('/live-processing')) return route.fulfill({ json: envelope({ bound: false, attempt: null }) });
    if (path.endsWith('/processing')) return route.fulfill({ json: envelope([]) });
    if (path === '/api/v1/providers') return route.fulfill({ json: { providers: [
      { id: 'local', configured: true, available: true, models: ['ivrit-turbo'] },
      { id: 'openai', configured: true, available: false },
      { id: 'gemini', configured: true, available: false },
    ] } });
    if (path.endsWith('/vocabulary')) return route.fulfill({ json: envelope({ hints: [], corrections: [], snippets: [] }) });
    if (path.endsWith('/cleanup/providers')) return route.fulfill({ json: envelope([]) });
    if (path === '/api/v1/recordings') {
      uploads++;
      const metadata = JSON.parse(route.request().postDataBuffer()!.toString().split('name="metadata"\r\n\r\n')[1].split('\r\n--')[0]).data;
      const recording: Recording = { metadata, storage: 'self_hosted_server', audio: { state: 'saved' } }; recordings.set(metadata.id, recording);
      return route.fulfill({ json: envelope(recording) });
    }
    if (path.startsWith('/api/v1/recordings/')) {
      const recording = recordings.get(path.split('/').at(-1)!);
      return route.fulfill({ status: recording ? 200 : 404, json: envelope(recording) });
    }
    if (path === '/api/v1/transcriptions') {
      const request = route.request().postDataJSON().data as TranscriptionRequest; requests.push(request);
      const raw_text = request.options.input_language === 'en' ? 'English fixture text' : request.options.input_language === 'he' ? 'טקסט בדיקה' : 'בדיקה with English';
      const job: Job = { id: crypto.randomUUID(), request, created_at_ms: Date.now(), updated_at_ms: Date.now(), status: { state: 'succeeded', result: { cleanup: { state: 'off' }, transcript: { ...request.options, raw_text, execution_location: request.options.selection.provider === 'local' ? 'self_hosted_server' : 'cloud' } } } };
      jobs.set(job.id, job); return route.fulfill({ status: 202, json: envelope(job) });
    }
    if (path.endsWith('/processed')) return route.fulfill({ json: envelope({ text: 'Authored processed fixture' }) });
    if (path.startsWith('/api/v1/jobs/')) return route.fulfill({ json: envelope(jobs.get(path.split('/').at(-1)!)) });
    return route.fulfill({ status: 404 });
  });
  await stubOwnerSession(page);
  const speech = (name: string) => page.getByRole('group', { name: 'Speech language' }).getByRole('button', { name, exact: true });
  const labels: Record<string, string> = { en: 'English', he: 'עברית · Hebrew', mixed: 'עברית + English' };
  const transcript = page.getByRole('group', { name: 'As spoken transcript' });
  const retry = page.getByRole('region', { name: 'Transcript', exact: true }).getByRole('button', { name: 'Transcribe again', exact: true });

  await page.goto('/?settings=models'); await loginFixture(page);
  await speech(labels.he).click();
  await page.getByLabel('Transcription model').selectOption('local/ivrit-turbo');
  await expect(page.getByText('Runs on your self-hosted server, not in this browser.')).toBeVisible();
  await page.keyboard.press('Escape');
  // An empty history offers its first dictation in place of the dock.
  await page.getByRole('button', { name: 'Try a dictation here' }).click();
  await expect(page.getByLabel('Recording duration')).not.toHaveText('00:00');
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(transcript).toHaveText('טקסט בדיקה');
  const id = requests[0].recording_id;
  // ivrit-turbo is Hebrew-only, so English and mixed must block the retry.
  for (const language of ['en', 'mixed']) {
    await page.goto(`/?settings=models`); await loginFixture(page);
    await speech(labels[language]).click();
    await page.keyboard.press('Escape');
    await expect(retry).toBeDisabled();
  }
  for (const [language, model, text] of [['en', 'openai/gpt-transcribe', 'English fixture text'], ['mixed', 'gemini/gemini-3.8-flash', 'בדיקה with English']]) {
    await page.goto('/?settings=models'); await loginFixture(page);
    await speech(labels[language]).click();
    await page.getByLabel('Transcription model').selectOption(model);
    await page.keyboard.press('Escape');
    await retry.click();
    await expect(transcript).toHaveText(text);
  }
  expect(requests).toHaveLength(3); expect(uploads).toBe(1);
  expect(requests.map(r => r.options.selection.provider)).toEqual(['local', 'openai', 'gemini']);
  expect(requests.map(r => r.options.input_language)).toEqual(['he', 'en', 'mixed']);
  for (const request of requests) { expect(request.recording_id).toBe(id); expect(request.options.output_language).toEqual({ mode: 'preserve' }); }
  await page.reload(); await loginFixture(page); await expect(page.locator('.session-row')).toHaveCount(1);
  await expect(transcript).toHaveText('בדיקה with English');
});

for (const configured of [false, true]) test(`local unavailable setup is actionable (configured=${configured})`, async ({ page }) => {
  await stubOwnerSession(page);
  await page.route('**/api/v1/vocabulary', route => route.fulfill({ json: { api_version: '1', data: { hints: [], corrections: [], snippets: [] } } }));
  await page.route('**/api/v1/health', route => route.fulfill({ json: { status: 'ok' } }));
  await page.route('**/api/v1/providers', route => route.fulfill({ json: { providers: [{ id: 'local', configured, available: false, models: ['ivrit-turbo'] }] } }));
  await page.goto('/?settings=models'); await loginFixture(page);
  await page.getByLabel('Transcription model').selectOption('local/ivrit-turbo');
  const models = page.getByRole('dialog', { name: 'Settings' });
  await expect(models).toContainText('model weights');
  await expect(models).toContainText('GPU worker');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Try a dictation here' })).toBeEnabled();
  await page.goto('/?settings=models'); await loginFixture(page);
  await page.getByLabel('Transcription model').selectOption('local/whisper-tiny');
  await expect(models).toContainText('Select the configured local model: ivrit-turbo');
});
