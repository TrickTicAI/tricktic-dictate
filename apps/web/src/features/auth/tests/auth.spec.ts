import { expect, test, type Page } from '@playwright/test';
async function login(page: Page) {
  await page.getByLabel('Owner key').fill('a'.repeat(64));
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Sign out', exact: true })).toBeVisible();
}
test('real sign-in, memory-only sessions, logout revocation and recording recovery', async ({ page, context }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Sign in to TrickTic Dictate' })).toBeVisible();
  await page.getByLabel('Owner key').fill('wrong');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Sign-in failed');
  await expect(page.getByLabel('Owner key')).toHaveValue('');
  const authenticated = page.waitForRequest(request => new URL(request.url()).pathname === '/api/v1/providers');
  await login(page);
  const authorization = (await authenticated).headers().authorization;
  expect(!!authorization).toBeTruthy();
  // A first run has nothing in history, so Home leads with its first-dictation button.
  await expect(page.getByRole('button', { name: 'Try a dictation here', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Try a dictation here', exact: true }).click();
  await expect(page.getByLabel('Recording duration')).not.toHaveText('00:00');
  // Signing out while recording drains saved capture instead of orphaning a microphone.
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Sign in to TrickTic Dictate' })).toBeVisible();
  const revoked = await context.request.get('/api/v1/settings', { headers: { Authorization: authorization, 'X-Writer-Request': '1' } });
  expect(revoked.status()).toBe(401);
  await login(page);
  await expect(page.locator('.session-row')).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Record', exact: true })).toBeEnabled();
  expect(await context.cookies()).toHaveLength(0);
  const storedSecret = await page.evaluate(auth => [...Object.values(localStorage), ...Object.values(sessionStorage)].some(value => value.includes(auth.slice(7)) || value.includes('a'.repeat(64))), authorization);
  expect(storedSecret).toBeFalsy();
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Sign in to TrickTic Dictate' })).toBeVisible();
  await login(page);
  await expect(page.locator('.session-row')).toHaveCount(1);
});

test('browser CSRF, null origin and cross-origin preflights fail; authorized mutations work', async ({ page, context }) => {
  await page.goto('/');
  const authenticated = page.waitForRequest(request => new URL(request.url()).pathname === '/api/v1/providers');
  await login(page);
  const authorization = (await authenticated).headers().authorization;
  const api = 'http://127.0.0.1:15204';
  // A second origin in the same browser attempts a simple POST and a preflighted fetch.
  const attacker = await context.newPage();
  await attacker.goto('http://127.0.0.1:15205/');
  const failed = attacker.waitForResponse(response => response.url() === `${api}/api/v1/setup/runtime/stop`);
  await attacker.evaluate(async api => { await fetch(`${api}/api/v1/setup/runtime/stop`, { method: 'POST', mode: 'no-cors', body: '' }).catch(() => {}); }, api);
  expect((await failed).status()).toBe(403);
  const blocked = await attacker.evaluate(async api => {
    try { await fetch(`${api}/api/v1/session`, { method: 'POST', headers: { 'X-Writer-Request': '1', 'Content-Type': 'application/json' }, body: JSON.stringify({ owner_key: 'a'.repeat(64) }) }); return false; }
    catch { return true; }
  }, api);
  expect(blocked).toBeTruthy();
  const preflight = await context.request.fetch('/api/v1/session', { method: 'OPTIONS', headers: {
    Origin: 'http://127.0.0.1:15205', 'Access-Control-Request-Method': 'POST',
    'Access-Control-Request-Headers': 'content-type,x-writer-request',
  } });
  expect(preflight.status()).toBe(403);
  const sandbox = await context.request.post('/api/v1/setup/runtime/stop', { headers: { Origin: 'null', Authorization: authorization, 'X-Writer-Request': '1' } });
  expect(sandbox.status()).toBe(403);
  const vocabulary = { api_version: '1', data: { hints: [], corrections: [], snippets: [] } };
  const mutation = await context.request.put('/api/v1/vocabulary', { data: vocabulary, headers: { Origin: api, Authorization: authorization, 'X-Writer-Request': '1' } });
  expect(mutation.status()).toBe(200);
  await attacker.close();
});

test('server-side revocation returns the active UI to sign-in', async ({ page, context }) => {
  await page.goto('/');
  const authenticated = page.waitForRequest(request => new URL(request.url()).pathname === '/api/v1/providers');
  await login(page);
  const authorization = (await authenticated).headers().authorization;
  const revoked = await context.request.delete('/api/v1/session', { headers: { Authorization: authorization, 'X-Writer-Request': '1' } });
  expect(revoked.status()).toBe(204);
  await page.getByRole('button', { name: 'Dictionary', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Sign in to TrickTic Dictate' })).toBeVisible();
  await login(page);
  await expect(page.getByRole('button', { name: 'Sign out', exact: true })).toBeVisible();
});
