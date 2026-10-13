import { expect, test } from '@playwright/test';
import type { CleanupResult, Job } from '@metrix-writer/contracts';
const id = 'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee';
function job(cleanup: CleanupResult) {
  // Request metadata is not consumed by the control; the full typed fixture lives in panel.tsx.
  return { id, created_at_ms: 1, updated_at_ms: 2, status: { state: 'succeeded', result: {
    transcript: { raw_text: 'שלום hello Foo@EXAMPLE.com' }, cleanup,
  } } } as Job;
}
test('opt-in failure, retry and separate result keep raw visible', async ({ page }) => {
  let posts = 0;
  let cleanup: CleanupResult = { state: 'off' };
  await page.route('**/api/v1/cleanup/providers', (route) => route.fulfill({ json: { api_version: '1', data: [{ selection: { provider: 'openai', model: 'gpt-4.1-mini' }, configured: true, execution_location: 'cloud' }] } }));
  await page.route(`**/api/v1/jobs/${id}/cleanup`, async (route) => {
    posts++;
    expect(route.request().postDataJSON()).toEqual({ api_version: '1', data: { provider: 'openai', model: 'gpt-4.1-mini' } });
    cleanup = posts === 1 ? { state: 'failed', error: { code: 'provider_timeout', message: 'Cleanup timed out. Original available.', retryable: true } } : { state: 'succeeded', cleaned_text: 'שלום Hello Foo@EXAMPLE.com.', selection: {provider: 'openai', model: 'gpt-4.1-mini'} };
    await route.fulfill({ status: 202, json: { api_version: '1', data: job({ state: 'pending' }) } });
  });
  await page.route(`**/api/v1/jobs/${id}`, (route) => route.fulfill({ json: { api_version: '1', data: job(cleanup) } }));
  await page.goto('/src/features/cleanup/tests/panel.html');
  await expect(page.getByRole('button', {name: 'Clean up transcript', exact: true})).toBeEnabled();
  expect(posts).toBe(0);
  await expect(page.getByRole('textbox', {name: 'Original transcript', exact: true})).toHaveValue('שלום hello Foo@EXAMPLE.com');
  await page.getByRole('button', {name: 'Clean up transcript', exact: true}).click();
  await expect(page.getByRole('alert')).toContainText('Cleanup timed out');
  await expect(page.getByRole('textbox', {name: 'Original transcript', exact: true})).toHaveValue('שלום hello Foo@EXAMPLE.com');
  await page.getByRole('button', {name: 'Retry cleanup', exact: true}).click();
  await expect(page.getByRole('textbox', {name: 'Cleaned transcript', exact: true})).toHaveValue('שלום Hello Foo@EXAMPLE.com.');
  await expect(page.getByRole('textbox', {name: 'Original transcript', exact: true})).toHaveValue('שלום hello Foo@EXAMPLE.com');
  expect(posts).toBe(2);
});
test('restored pending is visibly retryable without auto posting', async ({page}) => {
  let posts = 0;
  await page.route('**/api/v1/cleanup/providers', (route) => route.fulfill({json: {api_version: '1', data: [{selection: {provider: 'gemini', model: 'gemini-3.8-flash'}, configured: true, execution_location: 'cloud'}]}}));
  await page.route(`**/api/v1/jobs/${id}`, (route) => route.fulfill({json: {api_version: '1', data: job({state: 'pending'})}}));
  await page.route(`**/api/v1/jobs/${id}/cleanup`, async (route) => { posts++; await route.fulfill({status: 202, json: {api_version: '1', data: job({state:'pending'})}}); });
  await page.goto('/src/features/cleanup/tests/panel.html?pending=1');
  await expect(page.getByRole('status')).toContainText('server restarted');
  await expect(page.getByRole('button', {name: 'Retry cleanup'})).toBeEnabled();
  expect(posts).toBe(0);
  await page.getByRole('button', {name: 'Retry cleanup'}).click();
  await expect.poll(() => posts).toBe(1);
  await expect(page.getByRole('textbox', {name: 'Original transcript', exact: true})).toBeVisible();
});
test('missing keys disable cloud action without hiding original', async ({page}) => {
  await page.route('**/api/v1/cleanup/providers', (route) => route.fulfill({json: {api_version: '1', data: [{selection: {provider: 'openai', model: 'gpt-4.1-mini'}, configured: false, execution_location: 'cloud'}]}}));
  await page.goto('/src/features/cleanup/tests/panel.html');
  await expect(page.getByText('Configure this provider’s API key on the server to enable cleanup.')).toBeVisible();
  await expect(page.getByRole('button', {name: 'Clean up transcript', exact: true})).toBeDisabled();
  await expect(page.getByRole('textbox', {name: 'Original transcript', exact: true})).toBeVisible();
});
