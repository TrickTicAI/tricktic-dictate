import { expect, test, type Page } from '@playwright/test';
import type { CloudCheck, SetupView, WeightsState, WorkerState } from '../api';

const pagePath = '/src/features/transcription-setup/tests/setup.html';
function view(overrides: Partial<SetupView> = {}, ivrit: WeightsState = { state: 'not_installed', resumable_bytes: null }, worker: WorkerState = { state: 'idle' }): SetupView {
  return {
    mode: 'desktop', first_run: true,
    settings: { ui_language: 'en', transcription: { selection: { provider: 'local', model: 'whisper-large-v3-turbo' }, input_language: 'mixed', output_language: { mode: 'preserve' }, vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' } } },
    runtime: { worker, problem: null, packaged: true, worker_executable_present: true, vulkan_library_present: true, ffmpeg_present: true, external_server: false, models_dir: null },
    models: [
      { id: 'ivrit-turbo', display_name: 'ivrit.ai Whisper large-v3-turbo (Hebrew only)', file_name: 'ivrit-turbo.bin', bytes: 1624555275, sha256: 'c8090411113357097bfafc2b8e228ec1639fa7f5fe4ecb5d054ac0ccef8641b1', source_url: 'https://huggingface.co/ivrit-ai/whisper-large-v3-turbo-ggml/resolve/main/ggml-model.bin', source_name: 'ivrit-ai/whisper-large-v3-turbo-ggml', license: 'Apache-2.0', notes: 'Hebrew fine-tune.', input_languages: ['he'], weights: ivrit, selected: false, loaded: worker.state === 'ready' && worker.model === 'ivrit-turbo' },
      { id: 'whisper-large-v3-turbo', display_name: 'Whisper large-v3-turbo (multilingual)', file_name: 'ggml-large-v3-turbo.bin', bytes: 1624555275, sha256: '1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69', source_url: 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin', source_name: 'ggerganov/whisper.cpp', license: 'MIT', notes: 'Multilingual.', input_languages: ['en', 'he', 'mixed'], weights: { state: 'not_installed', resumable_bytes: null }, selected: false, loaded: false },
    ],
    cloud: [
      { provider: 'openai', models: ['gpt-transcribe', 'whisper-1'], credential: 'absent', check: null, verified: false, selected: false },
      { provider: 'gemini', models: ['gemini-3.8-flash'], credential: 'protected_store', check: null, verified: false, selected: false },
      { provider: 'grok', models: ['grok-speech-to-text'], credential: 'absent', check: null, verified: false, selected: false },
    ],
    ...overrides,
  };
}
/** A view whose saved speech language is Hebrew, as if it had been changed in General. */
function hebrew(base: SetupView): SetupView {
  return { ...base, settings: { ...base.settings, transcription: { ...base.settings.transcription, input_language: 'he' } } };
}
interface Routed { current: SetupView; posts: string[]; puts: unknown[] }
async function route(page: Page, initial: SetupView, onPost?: (path: string, state: Routed) => void): Promise<Routed> {
  const state: Routed = { current: initial, posts: [], puts: [] };
  await page.route('**/api/v1/**', async r => {
    const url = new URL(r.request().url());
    if (url.pathname === '/api/v1/settings' && r.request().method() === 'PUT') {
      const body = r.request().postDataJSON() as { data: SetupView['settings'] };
      state.puts.push(body.data);
      state.current = { ...state.current, first_run: false, settings: body.data, models: state.current.models.map(m => ({ ...m, selected: body.data.transcription.selection.provider === 'local' && body.data.transcription.selection.model === m.id })), cloud: state.current.cloud.map(c => ({ ...c, selected: c.provider === body.data.transcription.selection.provider })) };
      return r.fulfill({ json: { api_version: '1', data: body.data } });
    }
    if (r.request().method() === 'POST') { state.posts.push(url.pathname); onPost?.(url.pathname, state); }
    return r.fulfill({ json: { api_version: '1', data: state.current } });
  });
  return state;
}
const native = (page: Page) => page.evaluate(() => window.nativeCalls);

test('the active engine is stated once, at the top, before anything can be changed', async ({ page }) => {
  const installed: WeightsState = { state: 'installed', bytes: 1624555275, sha256: 'c809', verified_at_ms: 1_800_000_000_000, origin: 'download' };
  await route(page, view({ first_run: false }, installed, { state: 'ready', model: 'whisper-large-v3-turbo', device: 'AMD Radeon RX 7800 XT', backend: 'vulkan', started_at_ms: 1, restarts: 0 }));
  await page.goto(pagePath);
  const active = page.getByRole('region', { name: 'Active transcription engine' });
  await expect(active).toContainText('Whisper large-v3-turbo (multilingual)');
  await expect(active).toContainText('Local');
  await expect(active).toContainText('GPU (Vulkan)');
  await expect(active).toContainText('Loaded');
  await expect(active.getByRole('status', { name: 'Active engine status' })).toContainText('whisper-large-v3-turbo is loaded on AMD Radeon RX 7800 XT (vulkan)');
  // The language whisper is told is part of the live answer, and it is set here.
  await expect(active).toContainText('I dictate in');
  await expect(active.getByRole('group', { name: 'Speech language' }).getByRole('button', { name: 'עברית + English' })).toHaveAttribute('aria-pressed', 'true');
  await expect(active.getByRole('button', { name: 'Unload model' })).toBeVisible();
});

test('a cloud engine is described in the same strip, by what the provider has actually said', async ({ page }) => {
  const checked: CloudCheck = { outcome: 'accepted', checked_at_ms: 1_800_000_000_000 };
  const base = view({ first_run: false });
  await route(page, {
    ...base,
    settings: { ...base.settings, transcription: { ...base.settings.transcription, selection: { provider: 'gemini', model: 'gemini-3.8-flash' } } },
    cloud: base.cloud.map(c => c.provider === 'gemini' ? { ...c, check: checked, selected: true } : c),
  });
  await page.goto(pagePath);
  const active = page.getByRole('region', { name: 'Active transcription engine' });
  await expect(active).toContainText('gemini-3.8-flash');
  await expect(active).toContainText('Cloud');
  await expect(active).toContainText('Connected');
  // Accepted is not verified: the strip says which one it means.
  await expect(active.getByRole('status', { name: 'Active engine status' })).toContainText('Gemini accepted it');
  await expect(active.getByRole('status', { name: 'Active engine status' })).toContainText('No transcript has run yet');
});

test('first run: every model is listed, downloaded with size, source and checksum, and only a compatible one can be used', async ({ page }) => {
  const installed: WeightsState = { state: 'installed', bytes: 1624555275, sha256: 'c809', verified_at_ms: 1_800_000_000_000, origin: 'download' };
  const state = await route(page, view(), (path, s) => {
    if (path.endsWith('/ivrit-turbo/download')) s.current = view({ first_run: true }, { state: 'downloading', downloaded_bytes: 400000000, total_bytes: 1624555275 });
  });
  await page.goto(pagePath);
  await expect(page.getByText('First run')).toBeVisible();
  const ivrit = page.getByRole('listitem', { name: 'ivrit.ai Whisper large-v3-turbo (Hebrew only)' });
  const multilingual = page.getByRole('listitem', { name: 'Whisper large-v3-turbo (multilingual)' });
  // Hebrew-only weights are not candidates while the spoken language is mixed,
  // so they are filtered out rather than offered and then refused.
  await expect(ivrit).toHaveCount(0);
  await expect(multilingual).toBeVisible();
  await page.getByRole('checkbox', { name: 'Only models that support עברית + English' }).uncheck();
  await expect(ivrit).toBeVisible();
  await expect(ivrit).toContainText('1.62 GB · F16 · עברית · from ivrit-ai/whisper-large-v3-turbo-ggml');
  // A catalog row carries no state line of its own: "Not installed" under every
  // entry is what the section it is listed in already says.
  await expect(ivrit.getByRole('status', { name: 'ivrit.ai Whisper large-v3-turbo (Hebrew only) weights' })).toHaveCount(0);
  await expect(ivrit).toContainText('Does not support עברית + English');
  await ivrit.getByRole('button', { name: 'Download 1.62 GB' }).click();
  expect(state.posts).toEqual(['/api/v1/setup/models/ivrit-turbo/download']);
  await expect(ivrit.getByRole('status', { name: /weights/ })).toContainText('Downloading 400.0 MB of 1.62 GB');
  await expect(ivrit.getByRole('button', { name: 'Cancel download' })).toBeVisible();
  // Each state is served until the panel has rendered it; polling picks up the next one.
  state.current = view({}, { state: 'verifying', bytes: 1624555275 });
  await expect(ivrit.getByRole('status', { name: /weights/ })).toContainText('Verifying SHA-256');
  state.current = view({}, installed, { state: 'starting', model: 'ivrit-turbo', since_ms: 1 });
  await expect(page.getByRole('status', { name: 'Active engine status' })).toContainText('Loading ivrit-turbo');
  state.current = view({}, installed, { state: 'ready', model: 'ivrit-turbo', device: 'AMD Radeon RX 7800 XT', backend: 'vulkan', started_at_ms: 1, restarts: 0 });
  await expect(page.getByRole('status', { name: 'Active engine status' })).toContainText('ivrit-turbo is loaded on AMD Radeon RX 7800 XT (vulkan)');
  await expect(ivrit.getByText('Loaded')).toBeVisible();
  // When it was verified is evidence, so it moved into the disclosure instead
  // of sitting under every installed row.
  await ivrit.getByRole('button', { name: /ivrit\.ai Whisper/ }).click();
  await expect(ivrit.getByRole('status', { name: /weights/ })).toContainText('Installed and verified (download)');
  await ivrit.getByRole('button', { name: /ivrit\.ai Whisper/ }).click();
  await expect(page.getByRole('button', { name: 'Unload model' })).toBeVisible();
  // Hebrew-only weights stay unusable until the speech language is compatible; nothing is implied.
  await expect(ivrit.getByRole('button', { name: 'Use this model' })).toBeDisabled();
  // The selected multilingual model can serve Hebrew, so the switch stores at once.
  await page.getByRole('group', { name: 'Speech language' }).getByRole('button', { name: 'עברית · Hebrew' }).click();
  await expect.poll(() => state.puts.length).toBe(1);
  expect((state.puts[0] as SetupView['settings']).transcription.input_language).toBe('he');
  await expect(ivrit.getByRole('button', { name: 'Use this model' })).toBeEnabled();
  await ivrit.getByRole('button', { name: 'Use this model' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Saved. TrickTic Dictate reuses this choice on the next launch.' })).toBeVisible();
  expect(state.puts).toHaveLength(2);
  const saved = state.puts[1] as SetupView['settings'];
  expect(saved.transcription.selection).toEqual({ provider: 'local', model: 'ivrit-turbo' });
  expect(saved.transcription.input_language).toBe('he');
  expect(saved.transcription.output_language).toEqual({ mode: 'preserve' });
  await expect(ivrit.getByText('In use')).toBeVisible();
  await expect(page.getByText('First run')).toHaveCount(0);
  expect(JSON.stringify(state.puts)).not.toContain('sk-');
});

test('explicit failure states: GPU refusal with retry, missing Vulkan runtime, checksum mismatch, cancel keeps resumable bytes', async ({ page }) => {
  const failed = hebrew(view({ first_run: false, settings: { ...view().settings, transcription: { ...view().settings.transcription, selection: { provider: 'local', model: 'ivrit-turbo' } } } },
    { state: 'failed', error: 'checksum_mismatch', resumable_bytes: null }, { state: 'failed', model: 'ivrit-turbo', error: 'gpu_backend_not_used', restarts: 3 }));
  const state = await route(page, failed, (path, s) => {
    if (path.endsWith('/runtime/retry')) s.current = { ...s.current, runtime: { ...s.current.runtime, worker: { state: 'missing_runtime', detail: 'vulkan_backend_missing' }, vulkan_library_present: false } };
    if (path.endsWith('/ivrit-turbo/download')) s.current = hebrew(view({ first_run: false }, { state: 'downloading', downloaded_bytes: 10, total_bytes: 1624555275 }, s.current.runtime.worker));
    if (path.endsWith('/ivrit-turbo/cancel')) s.current = hebrew(view({ first_run: false }, { state: 'failed', error: 'download_canceled', resumable_bytes: 700000000 }, s.current.runtime.worker));
  });
  await page.goto(pagePath);
  await expect(page.getByRole('alert').filter({ hasText: 'loaded without the requested GPU backend' })).toContainText('Check the GPU driver');
  await expect(page.getByRole('alert').filter({ hasText: 'requested GPU backend' })).toContainText('3 automatic restarts');
  const ivrit = page.getByRole('listitem', { name: 'ivrit.ai Whisper large-v3-turbo (Hebrew only)' });
  await expect(ivrit.getByRole('alert')).toContainText('did not match the published SHA-256 checksum and was removed');
  await page.getByRole('button', { name: 'Retry loading model' }).click();
  expect(state.posts).toContain('/api/v1/setup/runtime/retry');
  await expect(page.getByRole('alert').filter({ hasText: 'Vulkan' })).toContainText('GPU (Vulkan) runtime is missing. Install it or choose CPU');
  await ivrit.getByRole('button', { name: /Download 1.62 GB/ }).click();
  await ivrit.getByRole('button', { name: 'Cancel download' }).click();
  expect(state.posts).toContain('/api/v1/setup/models/ivrit-turbo/cancel');
  await expect(ivrit.getByRole('alert')).toContainText('Download canceled. The partial file was kept');
  await expect(ivrit.getByRole('button', { name: 'Resume download (700.0 MB kept)' })).toBeVisible();
});

test('a stored key is checked as soon as it is shown, and the answer is a pill beside the key', async ({ page }) => {
  const state = await route(page, view({ first_run: false }), (path, s) => {
    if (path === '/api/v1/setup/cloud/gemini/test') {
      s.current = { ...s.current, cloud: s.current.cloud.map(c => c.provider === 'gemini' ? { ...c, check: { outcome: 'accepted' as const, checked_at_ms: 1_800_000_000_000 } } : c) };
    }
    if (path === '/api/v1/setup/cloud/openai/test') {
      s.current = { ...s.current, cloud: s.current.cloud.map(c => c.provider === 'openai' ? { ...c, check: { outcome: 'rejected' as const, checked_at_ms: 1_800_000_000_000 } } : c) };
    }
  });
  await page.goto(pagePath);
  await page.getByRole('button', { name: 'Cloud', exact: true }).click();

  // The key state is inside the card it gates, one step from fixing it; the
  // key itself is changed only in API keys (MW-SETTINGS-9).
  const openai = page.getByRole('listitem', { name: 'OpenAI' });
  await expect(openai.getByRole('status', { name: 'OpenAI key' })).toHaveText('No key stored.');
  await expect(openai).toContainText('No key');
  await expect(openai.getByRole('button', { name: 'Set up key' })).toBeVisible();
  await expect(openai.getByRole('button', { name: /Add key|Replace key|Check key|Remove key/ })).toHaveCount(0);
  await expect(openai.getByRole('button', { name: 'Use OpenAI' })).toBeDisabled();

  // A stored key nobody has checked is checked without being asked — the check
  // is remembered only for this backend process, so after a restart every key
  // would otherwise read "not checked" until someone pressed a button.
  const gemini = page.getByRole('listitem', { name: 'Gemini' });
  await expect.poll(() => state.posts.filter(path => path === '/api/v1/setup/cloud/gemini/test')).toHaveLength(1);
  await expect(gemini).toContainText('Connected');
  await expect(gemini.getByRole('status', { name: 'Gemini key' })).toContainText('Gemini accepted it');
  await expect(gemini.getByRole('status', { name: 'Gemini key' })).toContainText('No transcript has run yet');

  // A rejected key says so, in one word and in full, instead of staying an
  // unexplained "unverified". A key that appears is checked as it appears.
  state.current = { ...state.current, cloud: state.current.cloud.map(c => c.provider === 'openai' ? { ...c, credential: 'protected_store' as const } : c) };
  await expect(openai).toContainText('Error');
  await expect(openai.getByRole('status', { name: 'OpenAI key' })).toContainText('OpenAI rejected it. Replace the key.');

  await gemini.getByRole('button', { name: 'Use Gemini' }).click();
  expect((state.puts[0] as SetupView['settings']).transcription.selection).toEqual({ provider: 'gemini', model: 'gemini-3.8-flash' });
  await expect(gemini.getByText('Selected')).toBeVisible();
  expect(page.url()).toContain(pagePath);
});

test('a provider card sends you to API keys for its key rather than changing it here', async ({ page }) => {
  await route(page, view({ first_run: false }));
  await page.goto(pagePath);
  await page.getByRole('button', { name: 'Cloud', exact: true }).click();
  await page.getByRole('listitem', { name: 'OpenAI' }).getByRole('button', { name: 'Set up key' }).click();
  expect(await native(page)).toEqual(['keys:openai']);
  // A stored key is managed there too.
  await page.getByRole('listitem', { name: 'Gemini' }).getByRole('button', { name: 'Manage key' }).click();
  expect(await native(page)).toEqual(['keys:openai', 'keys:gemini']);
  await expect(page.getByRole('button', { name: /Add key|Replace key|Check key|Remove key/ })).toHaveCount(0);
});

test('reference measurements identify their test host and do not make missing weights ready', async ({ page }) => {
  const data = view();
  data.models[1].smoke_tests = { host: 'Reference AMD Windows host', date: '2026-09-08', scope: 'Small public samples; not a general accuracy benchmark', tests: [{ sample: 'fixture-en', language: 'en', backend: 'cpu', elapsed_ms: 1500, word_errors: 0, reference_words: 22 }] };
  await route(page, data);
  await page.goto(pagePath);
  const multilingual = page.getByRole('listitem', { name: 'Whisper large-v3-turbo (multilingual)' });
  // Evidence and provenance are real, and one disclosure away rather than in the row.
  await expect(page.getByText('Reference sample tests')).toHaveCount(0);
  await multilingual.getByRole('button', { name: /Whisper large-v3-turbo/ }).click();
  await expect(multilingual).toContainText('SHA-256 1fc70f774d38…');
  await multilingual.getByText('Reference sample tests', { exact: true }).click();
  await expect(multilingual.getByText('Reference AMD Windows host', { exact: false })).toBeVisible();
  await expect(multilingual.getByRole('row').filter({ hasText: 'English' })).toContainText('1.5 s');
  await expect(multilingual.getByRole('row').filter({ hasText: 'English' })).toContainText('0 / 22');
  // Not installed: measured elsewhere is not ready here.
  await expect(multilingual.getByRole('button', { name: 'Use this model' })).toHaveCount(0);
  await expect(multilingual.getByRole('button', { name: 'Download 1.62 GB' })).toBeVisible();
});

test('processor selection persists through reload and model deletion requires an explicit confirmation', async ({ page }) => {
  const initial = view({}, { state: 'installed', bytes: 1624555275, sha256: 'fixture', verified_at_ms: 1, origin: 'import' });
  initial.runtime.execution = { requested: 'automatic', cpu_available: true, gpu_available: false, fallback_reason: 'vulkan_runtime_missing' };
  const state = await route(page, initial, (path, s) => {
    if (path.endsWith('/ivrit-turbo/delete')) s.current.models[0].weights = { state: 'not_installed', resumable_bytes: null };
  });
  await page.route('**/api/v1/setup/execution', async r => {
    const backend = r.request().postDataJSON().data;
    state.current.runtime.execution!.requested = backend;
    state.current.runtime.execution!.fallback_reason = null;
    await r.fulfill({ json: { api_version: '1', data: state.current } });
  });
  await page.goto(pagePath);
  await expect(page.getByText('Automatic selected CPU for the same model:', { exact: false })).toBeVisible();
  // Runtime knobs are real but secondary: named, closed, one click away.
  await expect(page.getByLabel('Processor for speech recognition')).toBeHidden();
  await page.getByText('Advanced', { exact: true }).click();
  await page.getByLabel('Processor for speech recognition').selectOption('cpu');
  await page.reload();
  await page.getByText('Advanced', { exact: true }).click();
  await expect(page.getByLabel('Processor for speech recognition')).toHaveValue('cpu');

  await page.getByRole('checkbox', { name: /Only models that support/ }).uncheck();
  const ivrit = page.getByRole('listitem', { name: 'ivrit.ai Whisper large-v3-turbo (Hebrew only)' });
  await ivrit.getByRole('button', { name: /ivrit\.ai Whisper/ }).click();
  await ivrit.getByRole('button', { name: 'Delete model files' }).click();
  expect(state.posts).not.toContain('/api/v1/setup/models/ivrit-turbo/delete');
  await expect(page.getByRole('group', { name: 'Confirm model deletion' })).toContainText('Saved recordings, transcripts and your original import file are kept');
  await page.getByRole('button', { name: 'Keep model' }).click();
  expect(state.posts).not.toContain('/api/v1/setup/models/ivrit-turbo/delete');
  await ivrit.getByRole('button', { name: 'Delete model files' }).click();
  await page.getByRole('button', { name: 'Confirm deletion' }).click();
  await expect(ivrit.getByRole('button', { name: /Download 1.62 GB/ })).toBeVisible();
  expect(state.posts).toContain('/api/v1/setup/models/ivrit-turbo/delete');
});

test('weights already on disk are imported for the model that needs them, and verified before use', async ({ page }) => {
  await route(page, view({ first_run: false }));
  await page.goto(pagePath);
  await page.getByRole('checkbox', { name: /Only models that support/ }).uncheck();
  const ivrit = page.getByRole('listitem', { name: 'ivrit.ai Whisper large-v3-turbo (Hebrew only)' });
  await ivrit.getByRole('button', { name: /ivrit\.ai Whisper/ }).click();
  await expect(ivrit).toContainText('checks its size and SHA-256 against the catalog, refuses anything that does not match, and stores it as ivrit-turbo.bin');
  await ivrit.getByRole('button', { name: 'Import weights file…' }).click();
  // One import per model, so the file can never be copied in under another model's name.
  expect(await native(page)).toEqual(['import:ivrit-turbo']);
  await expect(page.getByRole('status').filter({ hasText: 'Verify the file to confirm its checksum' })).toBeVisible();
});

test('load failure keeps recording independent and offers retry; unsupported language is rejected without saving', async ({ page }) => {
  let fail = true;
  await page.route('**/api/v1/setup', r => fail ? r.fulfill({ status: 503, json: { api_version: '1', error: { code: 'provider_unavailable', message: 'x', retryable: true } } }) : r.fulfill({ json: { api_version: '1', data: { ...view(), models: view().models.map(m => ({ ...m, weights: { state: 'installed', bytes: m.bytes, sha256: m.sha256, verified_at_ms: 1, origin: 'download' } })) } } }));
  const puts: unknown[] = [];
  await page.route('**/api/v1/settings', r => { puts.push(r.request().postDataJSON()); return r.fulfill({ status: 422, json: { api_version: '1', error: { code: 'unsupported_capability', message: 'x', retryable: false } } }); });
  await page.goto(pagePath);
  await expect(page.getByRole('alert')).toContainText('backend is unavailable. Saved audio and text remain on this device.');
  fail = false;
  await page.getByRole('button', { name: 'Retry loading' }).click();
  const multilingual = page.getByRole('listitem', { name: 'Whisper large-v3-turbo (multilingual)' });
  await expect(multilingual.getByRole('button', { name: 'Use this model' })).toBeEnabled();
  await multilingual.getByRole('button', { name: 'Use this model' }).click();
  await expect(page.getByRole('alert')).toContainText('does not support the selected speech language');
  expect(puts).toHaveLength(1);
});

test('the switcher opens on the side the saved engine is already on', async ({ page }) => {
  const base = view({ first_run: false });
  await route(page, {
    ...base,
    settings: { ...base.settings, transcription: { ...base.settings.transcription, selection: { provider: 'openai', model: 'gpt-transcribe' } } },
    cloud: base.cloud.map(c => c.provider === 'openai' ? { ...c, credential: 'environment' as const, selected: true } : c),
  });
  await page.goto(pagePath);
  await expect(page.getByRole('listitem', { name: 'OpenAI' })).toBeVisible();
  await expect(page.getByRole('listitem', { name: 'Whisper large-v3-turbo (multilingual)' })).toHaveCount(0);
  // An environment key is reported here and managed, like every key, in API keys.
  await expect(page.getByRole('listitem', { name: 'OpenAI' }).getByRole('button', { name: 'Manage key' })).toBeVisible();
  await page.getByRole('button', { name: 'Local', exact: true }).click();
  await expect(page.getByRole('listitem', { name: 'Whisper large-v3-turbo (multilingual)' })).toBeVisible();
});

test('the library is grouped by what is already here, and filtered by things that matter', async ({ page }) => {
  const installedState: WeightsState = { state: 'installed', bytes: 1624555275, sha256: 'c809', verified_at_ms: 1, origin: 'download' };
  await route(page, view({ first_run: false }, installedState));
  await page.goto(pagePath);
  await page.getByRole('checkbox', { name: /Only models that support/ }).uncheck();

  // Two questions, two lists: what do I have, and what could I get.
  await expect(page.getByRole('heading', { name: /^Installed/ })).toBeVisible();
  await expect(page.getByRole('heading', { name: /^Available to download/ })).toBeVisible();
  const installedSection = page.locator('.setup-section').filter({ has: page.getByRole('heading', { name: /^Installed/ }) });
  await expect(installedSection.getByRole('listitem')).toHaveCount(1);
  await expect(installedSection.getByRole('listitem', { name: 'ivrit.ai Whisper large-v3-turbo (Hebrew only)' })).toBeVisible();

  // Once something is installed the catalog opens closed, so everything under
  // the list is a scroll away rather than a library away.
  await expect(page.getByRole('listitem', { name: 'Whisper large-v3-turbo (multilingual)' })).toHaveCount(0);
  await page.getByRole('button', { name: /^Available to download/ }).click();
  await expect(page.getByRole('listitem', { name: 'Whisper large-v3-turbo (multilingual)' })).toBeVisible();

  await page.getByRole('searchbox').fill('ivrit');
  await expect(page.getByRole('listitem', { name: 'Whisper large-v3-turbo (multilingual)' })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: /^Available to download/ })).toHaveCount(0);

  await page.getByRole('searchbox').fill('');
  await page.getByLabel('Precision').selectOption('q5');
  await expect(page.getByText('No models match these filters.')).toBeVisible();
  await page.getByLabel('Precision').selectOption('f16');
  await expect(page.getByRole('listitem')).toHaveCount(2);

  // The language filter is on by default and is the one that matters most here.
  await page.reload();
  await expect(page.getByRole('checkbox', { name: 'Only models that support עברית + English' })).toBeChecked();
  await expect(page.getByRole('listitem', { name: 'ivrit.ai Whisper large-v3-turbo (Hebrew only)' })).toHaveCount(0);
});

test('a language the selected model cannot serve is held, explained, and saved with the model that can', async ({ page }) => {
  const installedState: WeightsState = { state: 'installed', bytes: 1624555275, sha256: 'c809', verified_at_ms: 1, origin: 'download' };
  const base = hebrew(view({ first_run: false }, installedState));
  const state = await route(page, {
    ...base,
    settings: { ...base.settings, transcription: { ...base.settings.transcription, selection: { provider: 'local', model: 'ivrit-turbo' } } },
    models: base.models.map(m => m.id === 'ivrit-turbo'
      ? { ...m, selected: true, weights: installedState }
      : { ...m, weights: { state: 'installed' as const, bytes: m.bytes, sha256: m.sha256, verified_at_ms: 1, origin: 'download' } }),
  });
  await page.goto(pagePath);
  const active = page.getByRole('region', { name: 'Active transcription engine' });
  const languages = active.getByRole('group', { name: 'Speech language' });

  // ivrit-turbo is Hebrew-only, so English cannot be stored against it. Nothing
  // is written and the strip keeps saying what is actually in force.
  await languages.getByRole('button', { name: 'English', exact: true }).click();
  expect(state.puts).toHaveLength(0);
  await expect(active).toContainText('Still transcribing in עברית');
  await expect(active).toContainText('needs a model that supports it');

  // The list is judged against the language you asked for, not the stored one.
  await expect(page.getByRole('listitem', { name: 'ivrit.ai Whisper large-v3-turbo (Hebrew only)' })).toHaveCount(0);
  const multilingual = page.getByRole('listitem', { name: 'Whisper large-v3-turbo (multilingual)' });
  await expect(multilingual).toBeVisible();
  await multilingual.getByRole('button', { name: 'Use this model' }).click();

  // Both halves land in one write, so no intermediate state is ever stored.
  await expect.poll(() => state.puts.length).toBe(1);
  const saved = state.puts[0] as SetupView['settings'];
  expect(saved.transcription.selection).toEqual({ provider: 'local', model: 'whisper-large-v3-turbo' });
  expect(saved.transcription.input_language).toBe('en');
});

test('a key-only cloud row with no speech model is not offered as a speech engine', async ({ page }) => {
  const base = view({ first_run: false });
  const state = await route(page, { ...base, cloud: [...base.cloud, { provider: 'openrouter', models: [], credential: 'protected_store', check: null, verified: false, selected: false }] });
  await page.goto(pagePath);
  await page.getByRole('button', { name: 'Cloud', exact: true }).click();
  await expect(page.getByRole('listitem', { name: 'Grok' })).toBeVisible();
  await expect(page.getByRole('listitem', { name: 'OpenRouter' })).toHaveCount(0);
  // Its stored key belongs to Text models; this page neither shows nor checks it.
  await expect.poll(() => state.posts).toContain('/api/v1/setup/cloud/gemini/test');
  expect(state.posts).not.toContain('/api/v1/setup/cloud/openrouter/test');
});

test('a provider that cannot serve the spoken language is not offered for it', async ({ page }) => {
  await route(page, view({ first_run: false }));
  await page.goto(pagePath);
  await page.getByRole('button', { name: 'Cloud', exact: true }).click();
  // Grok publishes no Hebrew, so with mixed speech its card says so instead of
  // letting a selection be made and refused later.
  const grok = page.getByRole('listitem', { name: 'Grok' });
  await expect(grok).toBeVisible();
  await expect(grok.getByRole('button', { name: 'Use Grok' })).toBeDisabled();
  await expect(grok).toContainText('does not support עברית + English');
});

test('a model is offered with its license, and a CC BY model with the credit its license requires', async ({ page }) => {
  const base = view();
  const attribution = 'Parakeet TDT 0.6B v3 by NVIDIA Corporation, licensed under CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/).';
  const parakeet = { ...base.models[1], id: 'parakeet-tdt-0-6b-v3', display_name: 'Parakeet TDT 0.6B v3', file_name: 'parakeet.gguf', source_name: 'nvidia/parakeet-tdt-0.6b-v3', license: 'CC-BY-4.0', input_languages: ['en' as const], selected: false, loaded: false,
    weights: { state: 'not_installed' as const, resumable_bytes: null },
    metadata: { family: 'parakeet-tdt-v3', precision: 'q8_0', runtime: 'nemo_cpp', backends: ['cpu'], revision: '541d1f99c6b0c3cd0b11a95167540bb8edefd82b', qualification: 'windows_amd_smoke_tested', attribution } };
  await route(page, { ...base, models: [...base.models, parakeet] });
  await page.goto(pagePath);
  await page.getByRole('checkbox', { name: /^Only models that support/ }).uncheck();
  const row = page.getByRole('listitem', { name: 'Parakeet TDT 0.6B v3' });
  // Visible beside the download button, before anything is expanded.
  await expect(row).toContainText('from nvidia/parakeet-tdt-0.6b-v3 · CC BY 4.0');
  await expect(row.getByText(attribution)).toBeVisible();
  await expect(row.getByRole('button', { name: /^Download/ })).toBeVisible();
  const ivrit = page.getByRole('listitem', { name: 'ivrit.ai Whisper large-v3-turbo (Hebrew only)' });
  await expect(ivrit).toContainText('· Apache License 2.0');
  await expect(ivrit.locator('.setup-attribution')).toHaveCount(0);
});
