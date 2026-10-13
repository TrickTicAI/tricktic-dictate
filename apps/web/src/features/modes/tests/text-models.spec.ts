import { test, expect, type Page } from '@playwright/test';

interface Weights { state: string; resumable_bytes?: number | null; downloaded_bytes?: number; total_bytes?: number; bytes?: number }
interface Model { id: string; display_name: string; bytes: number; sha256: string; source_url: string; license: string; notes: string; weights: Weights }
interface View { supported: boolean; cpu_available: boolean; gpu_available: boolean; requested: string; last_execution: unknown; models: Model[] }

function view(): View {
  return {
    supported: true, cpu_available: true, gpu_available: true, requested: 'automatic', last_execution: null,
    models: [
      {
        id: 'qwen3-4b-instruct-2507-q4', display_name: 'Qwen3 4B Instruct 2507 (Q4_K_M)', bytes: 2_497_280_448,
        sha256: 'a'.repeat(64), source_url: 'https://huggingface.co/fixture/model/resolve/pin/qwen.gguf', license: 'Apache-2.0',
        notes: 'Published by Alibaba (China). Smaller and faster of the two. 8192-token shared input/output context.',
        weights: { state: 'not_installed', resumable_bytes: null },
      },
      {
        id: 'mistral-7b-instruct-v0-3-q4', display_name: 'Mistral 7B Instruct v0.3 (Q4_K_M)', bytes: 4_372_812_000,
        sha256: 'b'.repeat(64), source_url: 'https://huggingface.co/fixture/model/resolve/pin/mistral.gguf', license: 'Apache-2.0',
        notes: 'Published by Mistral AI (France). Larger download and slower on CPU. 8192-token shared input/output context.',
        weights: { state: 'not_installed', resumable_bytes: null },
      },
    ],
  };
}

async function route(page: Page, state: { current: View; actions: string[] }, configured = { openai: true, gemini: false, grok: false }, extra: object[] = []) {
  await page.route('**/api/v1/modes/providers', r => r.fulfill({ json: { api_version: '1', data: [
    { selection: { provider: 'openai', model: 'gpt-4.1-mini' }, configured: configured.openai, execution_location: 'cloud' },
    { selection: { provider: 'openai', model: 'gpt-5.6-luna' }, configured: configured.openai, execution_location: 'cloud' },
    { selection: { provider: 'gemini', model: 'gemini-3.8-flash' }, configured: configured.gemini, execution_location: 'cloud' },
    { selection: { provider: 'grok', model: 'grok-4.6' }, configured: configured.grok, execution_location: 'cloud' },
    { selection: { provider: 'local', model: 'qwen3-4b-instruct-2507-q4' }, configured: false, execution_location: 'self_hosted_server' },
    ...extra,
  ] } }));
  await page.route('**/api/v1/modes/local-models**', r => {
    const path = new URL(r.request().url()).pathname;
    if (r.request().method() === 'POST') {
      const action = path.split('/').at(-1)!;
      const id = path.split('/').at(-2)!;
      state.actions.push(`${id}:${action}`);
      const model = state.current.models.find(m => m.id === id);
      if (model && action === 'download') model.weights = { state: 'downloading', downloaded_bytes: 2_000_000_000, total_bytes: model.bytes };
      if (model && action === 'cancel') model.weights = { state: 'not_installed', resumable_bytes: 2_000_000_000 };
      if (model && action === 'verify') model.weights = { state: 'installed', bytes: model.bytes };
      if (model && action === 'delete') model.weights = { state: 'not_installed', resumable_bytes: null };
      if (action === 'execution') state.current.requested = r.request().postDataJSON().data;
    }
    return r.fulfill({ json: { api_version: '1', data: state.current } });
  });
}

test('both local text models are listed with who published them, and install independently', async ({ page }) => {
  const state = { current: view(), actions: [] as string[] };
  await route(page, state);
  await page.goto('/src/features/modes/tests/text-models.html');

  const qwen = page.getByRole('listitem', { name: 'Qwen3 4B Instruct 2507 (Q4_K_M)' });
  const mistral = page.getByRole('listitem', { name: 'Mistral 7B Instruct v0.3 (Q4_K_M)' });
  // Provenance is on the collapsed row: it is part of the choice, not a detail.
  await expect(qwen).toContainText('2.50 GB · Apache License 2.0 · Published by Alibaba (China)');
  await expect(mistral).toContainText('4.37 GB · Apache License 2.0 · Published by Mistral AI (France)');

  await mistral.getByRole('button', { name: 'Download 4.37 GB' }).click();
  expect(state.actions).toEqual(['mistral-7b-instruct-v0-3-q4:download']);
  await expect(mistral.getByRole('status', { name: /weights/ })).toContainText('Downloading 2.00 GB of 4.37 GB');
  // Downloading one leaves the other alone.
  await expect(qwen.getByRole('button', { name: 'Download 2.50 GB' })).toBeVisible();

  await mistral.getByRole('button', { name: 'Cancel download' }).click();
  await expect(mistral.getByRole('button', { name: 'Resume download (2.00 GB kept)' })).toBeVisible();
});

test('deletion needs an explicit confirmation and names what survives it', async ({ page }) => {
  const state = { current: view(), actions: [] as string[] };
  state.current.models[0].weights = { state: 'installed', bytes: state.current.models[0].bytes };
  await route(page, state);
  await page.goto('/src/features/modes/tests/text-models.html');
  const qwen = page.getByRole('listitem', { name: 'Qwen3 4B Instruct 2507 (Q4_K_M)' });
  await expect(qwen.getByText('Installed', { exact: true })).toBeVisible();

  await qwen.getByRole('button', { name: /Qwen3 4B/ }).click();
  await qwen.getByRole('button', { name: 'Delete text model' }).click();
  expect(state.actions).not.toContain('qwen3-4b-instruct-2507-q4:delete');
  await expect(page.getByRole('group', { name: 'Confirm text model deletion' })).toContainText('Modes that use it keep the assignment');
  await page.getByRole('button', { name: 'Keep model' }).click();
  expect(state.actions).not.toContain('qwen3-4b-instruct-2507-q4:delete');

  await qwen.getByRole('button', { name: 'Delete text model' }).click();
  await page.getByRole('button', { name: 'Confirm deletion' }).click();
  expect(state.actions).toContain('qwen3-4b-instruct-2507-q4:delete');
});

type Credential = 'absent' | 'environment' | 'protected_store' | 'unreadable';
interface Cloud { provider: string; models: string[]; credential: Credential; check: { outcome: string; checked_at_ms: number } | null; verified: boolean; selected: boolean }

/**
 * The document the speech page reads, served to this page too.
 *
 * One key per provider, so one answer about it; it is written only in API
 * keys. `patch` is how a test says a key appeared there, and the next reload
 * here sees it.
 */
async function routeSetup(page: Page, overrides: Record<string, Partial<Cloud>> = {}) {
  const setup = {
    cloud: [
      { provider: 'openai', models: ['gpt-transcribe'], credential: 'protected_store', check: { outcome: 'accepted', checked_at_ms: 1_800_000_000_000 }, verified: false, selected: false },
      { provider: 'gemini', models: ['gemini-3.8-flash'], credential: 'absent', check: null, verified: false, selected: false },
      { provider: 'grok', models: ['grok-speech-to-text'], credential: 'absent', check: null, verified: false, selected: false },
    ] as Cloud[],
    posts: [] as string[],
    patch(provider: string, fields: Partial<Cloud>) {
      setup.cloud = setup.cloud.map(item => item.provider === provider ? { ...item, ...fields } : item);
    },
  };
  for (const [provider, fields] of Object.entries(overrides)) setup.patch(provider, fields);
  const document = () => ({ api_version: '1', data: {
    mode: 'desktop', first_run: false,
    settings: { ui_language: 'en', transcription: { selection: { provider: 'local', model: 'whisper-large-v3-turbo' }, input_language: 'en', output_language: { mode: 'preserve' }, vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' } } },
    runtime: { worker: { state: 'idle' }, problem: null, packaged: true, worker_executable_present: true, vulkan_library_present: true, ffmpeg_present: true, external_server: false, models_dir: null },
    models: [],
    cloud: setup.cloud,
  } });
  await page.route('**/api/v1/setup', r => r.fulfill({ json: document() }));
  await page.route('**/api/v1/setup/cloud/*/test', r => {
    const path = new URL(r.request().url()).pathname;
    setup.posts.push(path);
    const provider = path.split('/').at(-2)!;
    setup.patch(provider, { check: { outcome: 'accepted', checked_at_ms: 1_800_000_000_000 } });
    return r.fulfill({ json: document() });
  });
  return setup;
}

test('cloud text providers carry the provider’s own answer, and the key is set here', async ({ page }) => {
  const state = { current: view(), actions: [] as string[] };
  await route(page, state, { openai: true, gemini: false, grok: false });
  const setup = await routeSetup(page);
  await page.goto('/src/features/modes/tests/text-models.html');
  await page.getByRole('button', { name: 'Cloud', exact: true }).click();

  const openai = page.getByRole('listitem', { name: 'OpenAI text model' });
  await expect(openai).toHaveCount(1);
  await expect(openai).toContainText('gpt-4.1-mini');
  await expect(openai).toContainText('gpt-5.6-luna');
  await expect(openai).toContainText('Key stored');
  await expect(openai).toContainText('Connected');
  await expect(openai.getByRole('status')).toContainText('OpenAI accepted it');
  await expect(openai.getByRole('status')).toContainText('Ready to assign to a mode');

  const grok = page.getByRole('listitem', { name: 'Grok text model' });
  await expect(grok).toContainText('grok-4.6');
  await expect(grok).toContainText('No key');
  await expect(grok.getByRole('status')).toContainText('One Grok key serves both');

  // The page that reports the missing key is one step from fixing it, and the
  // key is changed only in API keys (MW-SETTINGS-9), for that provider.
  await expect(page.getByRole('button', { name: /Add key|Replace key|Check key|Remove key/ })).toHaveCount(0);
  await grok.getByRole('button', { name: 'Set up key' }).click();
  await openai.getByRole('button', { name: 'Manage key' }).click();
  expect(await page.evaluate(() => window.navigated)).toEqual(['keys:grok', 'keys:openai']);
  // A key that appears is still checked here as it appears.
  setup.patch('grok', { credential: 'protected_store' });
  await expect.poll(() => setup.posts, { timeout: 10_000 }).toContain('/api/v1/setup/cloud/grok/test');
  await expect(grok).toContainText('Connected');

  // A text model is assigned per mode, so nothing here offers to select one.
  await expect(page.getByRole('button', { name: /^Use / })).toHaveCount(0);
});

test('OpenRouter is a text-only key row: its models are listed and its key is not described as serving transcription', async ({ page }) => {
  const state = { current: view(), actions: [] as string[] };
  await route(page, state, { openai: true, gemini: false, grok: false }, [
    { selection: { provider: 'openrouter', model: 'anthropic/claude-haiku-4.5' }, configured: false, execution_location: 'cloud' },
    { selection: { provider: 'openrouter', model: 'inception/mercury-2.5' }, configured: false, execution_location: 'cloud' },
  ]);
  const setup = await routeSetup(page);
  setup.cloud.push({ provider: 'openrouter', models: [], credential: 'absent', check: null, verified: false, selected: false });
  await page.goto('/src/features/modes/tests/text-models.html');
  await page.getByRole('button', { name: 'Cloud', exact: true }).click();

  const openrouter = page.getByRole('listitem', { name: 'OpenRouter text model' });
  await expect(openrouter).toContainText('anthropic/claude-haiku-4.5 · inception/mercury-2.5');
  await expect(openrouter.getByRole('status')).toContainText('OpenRouter is used for text processing only');

  await openrouter.getByRole('button', { name: 'Set up key' }).click();
  expect(await page.evaluate(() => window.navigated)).toEqual(['keys:openrouter']);
});

test('an environment key is reported here and, like every key, managed in API keys', async ({ page }) => {
  const state = { current: view(), actions: [] as string[] };
  await route(page, state, { openai: true, gemini: true, grok: false });
  await routeSetup(page, { gemini: { credential: 'environment' } });
  await page.goto('/src/features/modes/tests/text-models.html');
  await page.getByRole('button', { name: 'Cloud', exact: true }).click();
  const gemini = page.getByRole('listitem', { name: 'Gemini text model' });
  await expect(gemini.getByRole('status')).toContainText('Key read from the server environment');
  await gemini.getByRole('button', { name: 'Manage key' }).click();
  expect(await page.evaluate(() => window.navigated)).toEqual(['keys:gemini']);
  await expect(page.getByText('Keys are added, checked and removed in Settings › API keys.', { exact: false })).toBeVisible();
});

test('the text processor is a separate runtime from the speech one and persists', async ({ page }) => {
  const state = { current: view(), actions: [] as string[] };
  await route(page, state);
  await page.goto('/src/features/modes/tests/text-models.html');
  const processor = page.getByLabel('Processor for text processing');
  await expect(processor).toHaveValue('automatic');
  await processor.selectOption('cpu');
  expect(state.actions).toContain('local-models:execution');
  await page.reload();
  await expect(page.getByLabel('Processor for text processing')).toHaveValue('cpu');
});

test('custom models are not managed here, and the page says where they are', async ({ page }) => {
  const state = { current: view(), actions: [] as string[] };
  await route(page, state);
  await routeSetup(page);
  await page.goto('/src/features/modes/tests/text-models.html');
  await page.getByRole('button', { name: 'Cloud', exact: true }).click();
  await expect(page.getByRole('listitem', { name: 'OpenAI text model' })).toBeVisible();
  await expect(page.getByRole('button', { name: /custom model/ })).toHaveCount(0);
  await expect(page.getByText('custom cloud models are added there with Manage custom models')).toBeVisible();
});

