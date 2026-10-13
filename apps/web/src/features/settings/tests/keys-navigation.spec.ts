import { expect, test, type Page } from '@playwright/test';

/*
 * Keys are changed only in API keys (MW-SETTINGS-9). This runs the real
 * Settings dialog: a page that reports a key sends you to API keys for that
 * provider, and Back returns you to that page as you left it.
 */

const cloud = [
  { provider: 'openai', models: ['gpt-transcribe'], credential: 'absent', check: null, verified: false, selected: false },
  { provider: 'gemini', models: ['gemini-3.8-flash'], credential: 'protected_store', check: { outcome: 'accepted', checked_at_ms: 1_800_000_000_000 }, verified: false, selected: false },
  { provider: 'grok', models: ['grok-speech-to-text'], credential: 'absent', check: null, verified: false, selected: false },
  { provider: 'openrouter', models: [], credential: 'absent', check: null, verified: false, selected: false },
];

async function routes(page: Page) {
  const document = { api_version: '1', data: {
    mode: 'desktop', first_run: false,
    settings: { ui_language: 'en', transcription: { selection: { provider: 'local', model: 'whisper-large-v3-turbo' }, input_language: 'en', output_language: { mode: 'preserve' }, vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' } } },
    runtime: { worker: { state: 'idle' }, problem: null, packaged: true, worker_executable_present: true, vulkan_library_present: true, ffmpeg_present: true, external_server: false, models_dir: null },
    models: [], cloud,
  } };
  await page.route('**/api/v1/**', route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/v1/setup' || path.startsWith('/api/v1/setup/cloud/')) return route.fulfill({ json: document });
    return route.fulfill({ status: 404, json: {} });
  });
}

test('a page that reports a key opens API keys for it, and Back returns to that page as it was', async ({ page }) => {
  await page.setViewportSize({ width: 1137, height: 790 });
  await routes(page);
  await page.goto('/src/features/settings/tests/modal-focus.html');
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  const nav = dialog.getByRole('navigation', { name: 'Settings sections' });
  const heading = dialog.locator('.modal-head h2');

  await nav.getByRole('button', { name: 'Speech engine', exact: true }).click();
  // The saved engine is local, so choosing the Cloud side is state this page holds.
  await dialog.getByRole('button', { name: 'Cloud', exact: true }).click();
  const grok = dialog.getByRole('listitem', { name: 'Grok', exact: true });
  const body = dialog.locator('.modal-body');
  const scrolled = await body.evaluate(el => { el.scrollTop = el.scrollHeight; return el.scrollTop; });
  expect(scrolled).toBeGreaterThan(100);
  await expect(dialog.getByRole('button', { name: /Add key|Replace key|Check key|Remove key/ })).toHaveCount(0);
  await grok.getByRole('button', { name: 'Set up key' }).click();

  await expect(heading).toHaveText('API keys');
  const row = dialog.getByRole('listitem', { name: 'Grok key' });
  await expect(row).toHaveClass(/selected/);
  await expect(row.getByRole('button', { name: 'Add key…' })).toBeFocused();
  // The way back sits in the header, on screen however far down the provider is.
  const back = dialog.getByRole('button', { name: 'Back to Speech engine' });
  await expect(back).toBeInViewport();
  await back.click();

  await expect(heading).toHaveText('Speech engine');
  await expect(dialog.getByRole('button', { name: 'Cloud', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(grok).toBeVisible();
  expect(Math.abs(await body.evaluate(el => el.scrollTop) - scrolled)).toBeLessThan(2);

  // API keys from the sidebar offers no way back; neither does it after you
  // have left it for another page and come back by the sidebar.
  await nav.getByRole('button', { name: 'API keys', exact: true }).click();
  await expect(dialog.getByRole('listitem', { name: 'OpenAI key' })).toBeVisible();
  await expect(dialog.getByRole('button', { name: /^Back to/ })).toHaveCount(0);
});

test('General’s cleanup key row opens API keys with the way back to General', async ({ page }) => {
  await routes(page);
  await page.goto('/src/features/settings/tests/modal-focus.html');
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('navigation', { name: 'Settings sections' }).getByRole('button', { name: 'General', exact: true }).click();
  await dialog.getByRole('button', { name: 'Manage API keys' }).click();
  await expect(dialog.locator('.modal-head h2')).toHaveText('API keys');
  await dialog.getByRole('button', { name: 'Back to General' }).click();
  await expect(dialog.locator('.modal-head h2')).toHaveText('General');
  await expect(dialog.getByRole('button', { name: 'Manage API keys' })).toBeVisible();
});
