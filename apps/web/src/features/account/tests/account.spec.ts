import { expect, test, type Page } from '@playwright/test';

/*
 * The shell is simulated: `account_*` commands are recorded, and each test
 * publishes `writer-account` events itself. Only fixed kinds and display
 * metadata ever reach this page; the fixture has no tokens to leak.
 */
type Snapshot = Record<string, unknown>;
async function open(page: Page, snapshot: Snapshot) {
  await page.route('http://tauri.localhost/**', async route => {
    const response = await route.fetch({ url: route.request().url().replace('http://tauri.localhost', 'http://127.0.0.1:15231') });
    await route.fulfill({ response });
  });
  await page.addInitScript((initial) => {
    Object.defineProperty(navigator, 'platform', { value: 'Win32', configurable: true });
    const calls: { command: string; args: unknown }[] = [];
    const listeners: { name: string; cb: (event: { payload: unknown }) => void }[] = [];
    const publish = (name: string) => (payload: unknown) => listeners.filter(l => l.name === name).forEach(l => l.cb({ payload }));
    Object.assign(window, {
      nativeCalls: calls,
      emitAccount: publish('writer-account'),
      emitTask: publish('writer-account-task'),
      __TAURI__: {
        core: { invoke: async (command: string, args: unknown) => {
          calls.push({ command, args });
          if (command === 'account_snapshot') return initial;
          if (sessionStorage.getItem('refuse')) throw sessionStorage.getItem('refuse');
          if (command === 'account_local_copy') return JSON.parse(sessionStorage.getItem('copy') ?? '{"items":0,"unsynced":0}');
          if (command === 'account_tasks') return JSON.parse(sessionStorage.getItem('tasks') ?? '{"configured":true,"unfinished":[]}');
          if (command === 'account_sensitive') return 'operation';
        } },
        event: { listen: async (name: string, cb: (event: { payload: unknown }) => void) => { listeners.push({ name, cb }); return () => {}; } },
      },
    });
  }, snapshot);
  await page.goto('http://tauri.localhost/src/features/account/tests/account.html');
}
const guest = { state: 'guest', availability: 'ready', generation: 1, reauthenticating: false };
const calls = (page: Page) => page.evaluate(() => (window as unknown as { nativeCalls: { command: string; args: { attemptId?: string } }[] }).nativeCalls);
const emit = (page: Page, payload: unknown) => page.evaluate((p) => (window as unknown as { emitAccount: (p: unknown) => void }).emitAccount(p), payload);
const emitTask = (page: Page, payload: unknown) => page.evaluate((p) => (window as unknown as { emitTask: (p: unknown) => void }).emitTask(p), payload);
const signedIn = { ...{ state: 'guest', availability: 'ready', generation: 1, reauthenticating: false }, state: 'signed_in', accountKey: { projectId: 'metrix-writer', uid: 'u' }, display: { email: 'person@example.test', name: 'Synthetic Person' } };
/** Opens the page with the shell's stored answers already in place. */
async function openWith(page: Page, snapshot: Snapshot, stored: Record<string, unknown>) {
  await page.addInitScript((entries) => { for (const [key, value] of Object.entries(entries)) sessionStorage.setItem(key, JSON.stringify(value)); }, stored);
  await open(page, snapshot);
}
const command = async (page: Page, name: string) => (await calls(page)).filter(call => call.command === name).map(call => call.args as Record<string, unknown>);

test('a public build offers no sign-in at all', async ({ page }) => {
  await open(page, { ...guest, availability: 'unavailable' });
  await expect(page.getByText('No account needed')).toBeVisible();
  await expect(page.getByRole('button', { name: /Sign in/ })).toHaveCount(0);
  await expect(page.getByText('Google account')).toHaveCount(0);
});

test('an unconfigured developer build says so instead of starting', async ({ page }) => {
  await open(page, { ...guest, availability: 'not_configured' });
  await expect(page.getByRole('button', { name: 'Sign in with Google' })).toBeDisabled();
  await expect(page.getByText('This build has no Google sign-in configuration.', { exact: false })).toBeVisible();
});

test('sign-in starts in the browser, can be cancelled, and states that nothing uploads', async ({ page }) => {
  await open(page, guest);
  await expect(page.getByText('Signing in uploads nothing', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Sign in with Google' }).click();
  const started = (await calls(page)).find(call => call.command === 'account_sign_in');
  expect(started?.args.attemptId).toMatch(/^[0-9a-f-]{36}$/);
  const attemptId = started!.args.attemptId;
  await emit(page, { version: 1, sequence: 1, generation: 1, type: 'sign_in_started', attemptId, reauthenticate: false });
  await emit(page, { version: 1, sequence: 2, generation: 1, type: 'auth_snapshot', snapshot: { ...guest, state: 'signing_in', attemptId } });
  await expect(page.getByText('Waiting for Google')).toBeVisible();
  await page.getByRole('button', { name: 'Cancel sign-in' }).click();
  expect((await calls(page)).find(call => call.command === 'account_cancel_sign_in')?.args).toEqual({ attemptId });
  await emit(page, { version: 1, sequence: 3, generation: 2, type: 'sign_in_finished', attemptId, outcome: 'cancelled' });
  await emit(page, { version: 1, sequence: 4, generation: 2, type: 'auth_snapshot', snapshot: { ...guest, generation: 2 } });
  await expect(page.getByText('Sign-in was cancelled. Nothing changed.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sign in with Google' })).toBeEnabled();
});

test('failures show fixed wording and stale events are ignored', async ({ page }) => {
  await open(page, guest);
  await emit(page, { version: 1, sequence: 5, generation: 1, type: 'sign_in_finished', attemptId: 'a', outcome: 'error', error: 'offline' });
  await expect(page.getByRole('alert')).toHaveText('Google could not be reached. Check your connection and try again.');
  // An older snapshot arriving late must not overwrite newer state.
  await emit(page, { version: 1, sequence: 6, generation: 1, type: 'auth_snapshot', snapshot: { ...guest, state: 'signed_in', display: { email: 'person@example.test', name: null }, accountKey: { projectId: 'metrix-writer', uid: 'u' } } });
  await emit(page, { version: 1, sequence: 4, generation: 1, type: 'auth_snapshot', snapshot: guest });
  await expect(page.getByText('person@example.test')).toBeVisible();
  await page.evaluate(() => sessionStorage.setItem('refuse', 'not a known kind'));
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  // The local-copy count is unknown too, so the page asks rather than assuming none.
  await page.getByRole('dialog').getByRole('button', { name: 'Sign out', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('could not be removed');
});

test('a signed-in account shows its identity and signs out', async ({ page }) => {
  await open(page, { ...guest, state: 'signed_in', accountKey: { projectId: 'metrix-writer', uid: 'u' }, display: { email: 'person@example.test', name: 'Synthetic Person' } });
  await expect(page.getByText('Synthetic Person')).toBeVisible();
  await expect(page.getByText('person@example.test')).toBeVisible();
  await expect(page.getByText('Signing in uploaded nothing', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  expect((await calls(page)).map(call => call.command)).toContain('account_sign_out');
});

test('an expired session asks for the same account again without blocking anything', async ({ page }) => {
  await open(page, { ...guest, state: 'reauth_required', accountKey: { projectId: 'metrix-writer', uid: 'u' }, display: { email: 'person@example.test', name: null } });
  await expect(page.getByText('Sign-in needed')).toBeVisible();
  await expect(page.getByText('Dictation is unaffected', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Sign in again' }).click();
  expect((await calls(page)).find(call => call.command === 'account_reauthenticate')?.args.attemptId).toMatch(/^[0-9a-f-]{36}$/);
});

test('signing out asks about synced copies only when this account has some', async ({ page }) => {
  await openWith(page, signedIn, { copy: { items: 4, unsynced: 1 } });
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Sign out' });
  await expect(dialog.getByText('This account has 4 synced items on this PC')).toBeVisible();
  await expect(dialog.getByText('1 change has not reached the cloud yet', { exact: false })).toBeVisible();
  await dialog.getByLabel('Remove them from this PC', { exact: false }).check();
  await dialog.getByRole('button', { name: 'Sign out', exact: true }).click();
  expect(await command(page, 'account_sign_out')).toEqual([{ localCopy: 'remove' }]);
});

test('switching accounts starts at once when nothing is stored for this account', async ({ page }) => {
  await open(page, signedIn);
  await page.getByRole('button', { name: 'Switch account' }).click();
  await expect.poll(() => command(page, 'account_switch')).toHaveLength(1);
  const [args] = await command(page, 'account_switch');
  expect(args.attemptId).toMatch(/^[0-9a-f-]{36}$/);
  expect(args.localCopy).toBeUndefined();
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('deleting cloud data names the categories and what devices keep', async ({ page }) => {
  await open(page, signedIn);
  await page.getByRole('button', { name: 'Delete cloud data…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete cloud data' });
  const confirm = dialog.getByRole('button', { name: 'Confirm with Google' });
  await expect(confirm).toBeDisabled();
  await dialog.getByLabel('Dictionary').check();
  await dialog.getByLabel('Transcript history').check();
  await dialog.getByLabel('Delete everywhere', { exact: false }).check();
  await expect(dialog.getByText('Files you exported are not affected', { exact: false })).toBeVisible();
  await confirm.click();
  const [args] = await command(page, 'account_sensitive');
  expect(args.action).toEqual({ kind: 'delete_cloud', categories: ['dictionary', 'transcripts'], everywhere: true });
  const action = args.action;
  await emitTask(page, { version: 1, operationId: 'operation', action, state: 'reauthenticating' });
  await expect(page.getByText('Confirm it is you in the Google tab', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Delete account…' })).toBeDisabled();
  await emitTask(page, { version: 1, operationId: 'operation', action, state: 'running' });
  await expect(page.getByText('Deleting from the cloud', { exact: false })).toBeVisible();
  await emitTask(page, { version: 1, operationId: 'operation', action, state: 'completed' });
  await expect(page.getByText('Other devices remove their synced copies when they next connect.', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Delete account…' })).toBeEnabled();
});

test('deleting the Writer account says the Google account stays and carries the local-copy choice', async ({ page }) => {
  await openWith(page, signedIn, { copy: { items: 2, unsynced: 0 } });
  await page.getByRole('button', { name: 'Delete account…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete Writer account' });
  await expect(dialog.getByText('Your Google account is not deleted.')).toBeVisible();
  await dialog.getByLabel('Keep a copy without the account', { exact: false }).check();
  await dialog.getByRole('button', { name: 'Confirm with Google and delete' }).click();
  const [args] = await command(page, 'account_sensitive');
  expect(args).toMatchObject({ action: { kind: 'delete_account' }, localCopy: 'keep' });
  await emitTask(page, { version: 1, operationId: 'operation', action: { kind: 'delete_account' }, state: 'failed', error: 'stale' });
  await expect(page.getByRole('alert')).toHaveText('Your account changed on another device. Review your choice and try again.');
});

test('sign out everywhere confirms first and reports the fixed result', async ({ page }) => {
  await open(page, signedIn);
  await page.getByRole('button', { name: 'Sign out everywhere' }).click();
  const dialog = page.getByRole('dialog', { name: 'Sign out on all devices' });
  await expect(dialog.getByText('within an hour', { exact: false })).toBeVisible();
  await dialog.getByRole('button', { name: 'Confirm with Google' }).click();
  expect((await command(page, 'account_sensitive'))[0].action).toEqual({ kind: 'revoke_sessions' });
});

test('an unfinished cleanup can be retried under the same operation', async ({ page }) => {
  const unfinished = { version: 1, operationId: 'stored', action: { kind: 'delete_account' }, state: 'failed_retryable' };
  await openWith(page, signedIn, { tasks: { configured: true, unfinished: [unfinished] } });
  await page.getByRole('button', { name: 'Retry' }).click();
  expect((await command(page, 'account_sensitive'))[0].action).toEqual({ kind: 'retry', operationId: 'stored' });
});

test('a build without the account service disables only the service actions', async ({ page }) => {
  await openWith(page, signedIn, { tasks: { configured: false, unfinished: [] } });
  await expect(page.getByText('This build has no account service configured', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Delete account…' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Switch account' })).toBeEnabled();
});
