import { expect, test, type Page } from '@playwright/test';

/*
 * Settings → API keys (MW-SETTINGS-1): every provider's key in one place, with
 * what uses it and the same native-dialog controls Transcription and Text
 * processing have. No key ever reaches this window.
 */

type Credential = 'absent' | 'environment' | 'protected_store' | 'unreadable';
interface Cloud { provider: string; models: string[]; credential: Credential; check: { outcome: string; checked_at_ms: number } | null; verified: boolean; selected: boolean }

async function routeSetup(page: Page) {
  const setup = {
    cloud: [
      { provider: 'openai', models: ['gpt-transcribe'], credential: 'protected_store', check: { outcome: 'accepted', checked_at_ms: 1_800_000_000_000 }, verified: false, selected: false },
      { provider: 'gemini', models: ['gemini-3.8-flash'], credential: 'absent', check: null, verified: false, selected: true },
      { provider: 'grok', models: ['grok-speech-to-text'], credential: 'environment', check: { outcome: 'accepted', checked_at_ms: 1_800_000_000_000 }, verified: false, selected: false },
      { provider: 'openrouter', models: [], credential: 'absent', check: null, verified: false, selected: false },
    ] as Cloud[],
    posts: [] as string[],
    patch(provider: string, fields: Partial<Cloud>) { setup.cloud = setup.cloud.map(item => item.provider === provider ? { ...item, ...fields } : item); },
  };
  const document = () => ({ api_version: '1', data: {
    mode: 'desktop', first_run: false,
    settings: { ui_language: 'en', transcription: { selection: { provider: 'gemini', model: 'gemini-3.8-flash' }, input_language: 'en', output_language: { mode: 'preserve' }, vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' } } },
    runtime: { worker: { state: 'idle' }, problem: null, packaged: true, worker_executable_present: true, vulkan_library_present: true, ffmpeg_present: true, external_server: false, models_dir: null },
    models: [], cloud: setup.cloud,
  } });
  await page.route('**/api/v1/setup', r => r.fulfill({ json: document() }));
  await page.route('**/api/v1/setup/cloud/*/test', r => {
    const path = new URL(r.request().url()).pathname;
    setup.posts.push(path);
    setup.patch(path.split('/').at(-2)!, { check: { outcome: 'accepted', checked_at_ms: 1_800_000_000_000 } });
    return r.fulfill({ json: document() });
  });
  return setup;
}

test('every provider is listed with what uses its key, and a missing key in use is called out', async ({ page }) => {
  await routeSetup(page);
  await page.goto('/src/features/settings/tests/keys.html');
  for (const name of ['OpenAI', 'Gemini', 'Grok', 'OpenRouter']) await expect(page.getByRole('listitem', { name: `${name} key` })).toBeVisible();
  const openrouter = page.getByRole('listitem', { name: 'OpenRouter key' });
  await expect(openrouter).toContainText('Text models · Used by transcript cleanup');
  await expect(openrouter.getByRole('alert')).toHaveText('OpenRouter is chosen for transcript cleanup but has no key.');
  await expect(page.getByRole('listitem', { name: 'Gemini key' })).toContainText('Used by the speech engine');
  // An environment key is not Writer's to delete.
  await expect(page.getByRole('listitem', { name: 'Grok key' }).getByRole('button', { name: 'Remove key…' })).toHaveCount(0);
});

test('adding a key opens the native dialog, then asks the provider about it', async ({ page }) => {
  const setup = await routeSetup(page);
  await page.goto('/src/features/settings/tests/keys.html');
  const openrouter = page.getByRole('listitem', { name: 'OpenRouter key' });
  const add = openrouter.getByRole('button', { name: 'Add key…' });
  await expect(add).toBeVisible();
  setup.patch('openrouter', { credential: 'protected_store' }); // what the native dialog stores
  await add.click();
  await expect.poll(() => page.evaluate(() => window.nativeCalls)).toEqual(['prompt:openrouter']);
  await expect.poll(() => setup.posts).toContain('/api/v1/setup/cloud/openrouter/test');
  await expect(openrouter).toContainText('Connected');
  await expect(openrouter.getByRole('alert')).toHaveCount(0);
  await expect(page.getByText('OpenRouter key stored.')).toBeVisible();
});

test('removing a key needs a confirmation that says what stops working', async ({ page }) => {
  await routeSetup(page);
  await page.goto('/src/features/settings/tests/keys.html');
  const openai = page.getByRole('listitem', { name: 'OpenAI key' });
  await openai.getByRole('button', { name: 'Remove key…' }).click();
  const confirm = page.getByRole('group', { name: 'Confirm OpenAI key removal' });
  await expect(confirm).toContainText('Saved audio and transcripts stay.');
  await confirm.getByRole('button', { name: 'Keep key' }).click();
  expect(await page.evaluate(() => window.nativeCalls)).toEqual([]);
  await openai.getByRole('button', { name: 'Remove key…' }).click();
  await page.getByRole('group', { name: 'Confirm OpenAI key removal' }).getByRole('button', { name: 'Remove key' }).click();
  await expect.poll(() => page.evaluate(() => window.nativeCalls)).toEqual(['remove:openai']);
});

test('a cancelled key dialog stores nothing, and a failing one is an error', async ({ page }) => {
  const setup = await routeSetup(page);
  await page.goto('/src/features/settings/tests/keys.html');
  const openrouter = page.getByRole('listitem', { name: 'OpenRouter key' });
  await page.evaluate(() => { window.credentialOutcome = 'canceled'; });
  await openrouter.getByRole('button', { name: 'Add key…' }).click();
  await expect.poll(() => page.evaluate(() => window.nativeCalls)).toEqual(['prompt:openrouter']);
  await expect(page.getByText('OpenRouter key stored.')).toHaveCount(0);
  await expect(openrouter.getByRole('status', { name: 'OpenRouter key state' })).toHaveText('No key stored.');
  expect(setup.posts).toEqual([]);
  await page.evaluate(() => { window.credentialFails = true; });
  await openrouter.getByRole('button', { name: 'Add key…' }).click();
  await expect(page.getByRole('alert').filter({ hasText: 'Native credential dialog failed.' })).toBeVisible();
});

test('opened from another page for one provider, that provider is marked and focused', async ({ page }) => {
  await routeSetup(page);
  await page.goto('/src/features/settings/tests/keys.html?focus=openrouter');
  const openrouter = page.getByRole('listitem', { name: 'OpenRouter key' });
  await expect(openrouter).toHaveClass(/selected/);
  await expect(openrouter.getByRole('button', { name: 'Add key…' })).toBeFocused();
  await expect(page.getByRole('listitem', { name: 'OpenAI key' })).not.toHaveClass(/selected/);
});

test('links lead to the pages that choose where keys are used', async ({ page }) => {
  await routeSetup(page);
  await page.goto('/src/features/settings/tests/keys.html');
  await page.getByRole('button', { name: 'Transcript cleanup' }).click();
  expect(await page.evaluate(() => window.navigated)).toEqual(['general']);
});

test('the self-hosted browser client says keys stay on the server and offers no key controls', async ({ page }) => {
  await page.goto('/src/features/settings/tests/keys.html?browser');
  await expect(page.getByText('Keys stay on your server')).toBeVisible();
  await expect(page.getByRole('button', { name: /key/ })).toHaveCount(0);
});
