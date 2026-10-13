import { expect, test, type Page } from '@playwright/test';

/*
 * Settings → Account → Backup (MW-OUATH-6) against a simulated shell. Each
 * command is recorded and answered from `replies`; nothing here reaches a
 * network, and no reply carries a token.
 */
type Replies = Record<string, unknown>;
async function open(page: Page, replies: Replies) {
  await page.route('http://tauri.localhost/**', async route => {
    const response = await route.fetch({ url: route.request().url().replace('http://tauri.localhost', 'http://127.0.0.1:15231') });
    await route.fulfill({ response });
  });
  await page.addInitScript((initial) => {
    Object.defineProperty(navigator, 'platform', { value: 'Win32', configurable: true });
    const calls: { command: string; args: unknown }[] = [];
    Object.assign(window, {
      nativeCalls: calls,
      replies: initial,
      __TAURI__: {
        core: { invoke: async (command: string, args: unknown) => {
          calls.push({ command, args });
          const reply = (window as unknown as { replies: Replies }).replies[command];
          if (reply && typeof reply === 'object' && 'refuse' in (reply as object)) throw (reply as { refuse: string }).refuse;
          return reply;
        } },
        event: { listen: async () => () => {} },
      },
    });
  }, replies);
  await page.goto('http://tauri.localhost/src/features/account/tests/account.html');
}
const calls = (page: Page) => page.evaluate(() => (window as unknown as { nativeCalls: { command: string; args: Record<string, unknown> }[] }).nativeCalls);

const guest = { state: 'guest', availability: 'unavailable', generation: 1, reauthenticating: false };
const signedIn = { state: 'signed_in', availability: 'ready', generation: 1, reauthenticating: false, accountKey: { projectId: 'metrix-writer', uid: 'u' }, display: { email: 'person@example.test', name: null } };
const status = (sync: unknown = null, extra: Record<string, unknown> = {}) => ({ sync_available: true, last_import: null, kept: [], theme: null, sync, ...extra });
const entry = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const preview = {
  import_id: '11111111-1111-4111-8111-111111111111', export_id: '22222222-2222-4222-8222-222222222222',
  mode: 'merge', categories: ['settings', 'dictionary'], disposition: 'restore_once', expected_local_revision: 3,
  settings: [
    { id: '00000000-0000-4000-8000-000000000002', key: 'input_language', current: 'en', incoming: 'mixed', outcome: { outcome: 'applied' } },
    { id: '00000000-0000-4000-8000-000000000004', key: 'transcription_model', current: { provider: 'local', model: 'whisper-small' }, incoming: { provider: 'gemini', model: 'gemini-3.8-flash' }, outcome: { outcome: 'preserved', reason: 'missing_key' } },
  ],
  dictionary: { added: [{ id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', entry: { kind: 'hint', enabled: true, text: 'שלום Writer' } }], unchanged: 1,
    conflicts: [{ id: entry, local: { kind: 'hint', enabled: true, text: 'mine' }, incoming: { kind: 'hint', enabled: true, text: 'theirs' } }],
    removed: [], overlapping: 0, reordered: true },
  apply_ids: [], preserve_ids: [], conflict_ids: [entry],
};

test('a build without sign-in still exports and imports, and shows no sync', async ({ page }) => {
  await open(page, { account_snapshot: guest, portable_status: { ...status(), sync_available: false }, portable_export: true });
  await expect(page.getByText('Export settings and dictionary')).toBeVisible();
  await expect(page.getByText('Not included: provider keys, recordings, transcripts', { exact: false })).toBeVisible();
  await expect(page.getByText('Sync between devices')).toHaveCount(0);
  await page.getByRole('button', { name: 'Export…' }).click();
  await expect(page.getByText('Exported.', { exact: false })).toBeVisible();
});

test('an import is previewed, needs a choice for each conflict and reports kept values', async ({ page }) => {
  await open(page, { account_snapshot: guest, portable_status: status(), portable_import_preview: preview,
    portable_import_apply: { state: 'applied', import_id: preview.import_id, local_revision: 4, rollback_snapshot_id: '33333333-3333-4333-8333-333333333333', outcomes: { transcription_model: { outcome: 'preserved', reason: 'missing_key' } }, theme: null } });
  await page.getByRole('button', { name: 'Import…' }).click();
  await page.getByRole('button', { name: 'Choose file…' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByText('Spoken language: English → Mixed Hebrew and English')).toBeVisible();
  await expect(dialog.getByText('Add this provider’s key on this computer to use it.')).toBeVisible();
  await expect(dialog.getByText('Differs')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Import' })).toBeDisabled();
  await dialog.getByLabel('Keep mine').check();
  // Declining one setting change keeps this computer's value.
  await dialog.getByRole('checkbox', { name: 'Use the file’s value' }).first().uncheck();
  await dialog.getByRole('button', { name: 'Import' }).click();
  const applied = (await calls(page)).find(call => call.command === 'portable_import_apply');
  expect(applied?.args).toEqual({ importId: preview.import_id, choices: { [entry]: 'local', '00000000-0000-4000-8000-000000000002': 'local' } });
  await expect(page.getByText('1 setting was kept but not applied', { exact: false })).toBeVisible();
});

test('a newer file is refused with fixed wording', async ({ page }) => {
  await open(page, { account_snapshot: guest, portable_status: status(), portable_import_preview: { refuse: 'unsupported_version' } });
  await page.getByRole('button', { name: 'Import…' }).click();
  await page.getByRole('button', { name: 'Choose file…' }).click();
  await expect(page.getByRole('alert')).toContainText('made by a newer version');
});

test('undo is offered only while nothing changed since the import', async ({ page }) => {
  const last = { import_id: preview.import_id, rollback_snapshot_id: '33333333-3333-4333-8333-333333333333', local_revision: 4, current_revision: 4, categories: ['settings'] };
  await open(page, { account_snapshot: guest, portable_status: status(null, { last_import: last, kept: [{ key: 'delivery', value: 'cleaned', reason: 'unsupported' }] }),
    portable_rollback: { state: 'applied', import_id: last.import_id, local_revision: 5, rollback_snapshot_id: '44444444-4444-4444-8444-444444444444', outcomes: {}, theme: null } });
  await expect(page.getByText('Pasted text: Cleaned.', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Undo import' }).click();
  expect((await calls(page)).find(call => call.command === 'portable_rollback')?.args).toEqual({ importId: last.import_id, rollbackSnapshotId: last.rollback_snapshot_id, expectedLocalRevision: 4 });
});

test('undo is refused after later edits', async ({ page }) => {
  const last = { import_id: preview.import_id, rollback_snapshot_id: '33333333-3333-4333-8333-333333333333', local_revision: 4, current_revision: 6, categories: ['dictionary'] };
  await open(page, { account_snapshot: guest, portable_status: status(null, { last_import: last }) });
  await expect(page.getByRole('button', { name: 'Undo import' })).toBeDisabled();
  await expect(page.getByText('changed after the last import', { exact: false })).toBeVisible();
});

test('signing in sends nothing; sync starts only after a confirmed preview', async ({ page }) => {
  await open(page, { account_snapshot: signedIn, portable_status: status({ joined: [], paused: false, consent: null, pending: 0, conflicts: [], last_success_ms: null, last_error: null, notices: [] }),
    sync_preview: { preview: { ...preview, disposition: 'restore_and_sync', conflict_ids: [], dictionary: { ...preview.dictionary, conflicts: [] } }, cloud: { settings: 8, dictionary: 12 }, uploads: { dictionary: 3 }, enabled: { settings: true, dictionary: true } },
    sync_confirm: { state: 'applied', import_id: preview.import_id, local_revision: 4, rollback_snapshot_id: '33333333-3333-4333-8333-333333333333', outcomes: {}, theme: null } });
  await expect(page.getByText('Settings and dictionary sync is off')).toBeVisible();
  expect((await calls(page)).map(call => call.command).filter(c => c.startsWith('sync_'))).toEqual([]);
  await page.getByRole('button', { name: 'Set up sync…' }).click();
  await page.getByRole('button', { name: 'Compare' }).click();
  expect((await calls(page)).find(call => call.command === 'sync_preview')?.args).toEqual({ categories: ['settings', 'dictionary'], disposition: 'restore_and_sync' });
  await expect(page.getByText('In your account: 8 settings and 12 dictionary entries.', { exact: false })).toBeVisible();
  await expect(page.getByText('upload 0 settings and 3 entries', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Restore and keep syncing' }).click();
  expect((await calls(page)).some(call => call.command === 'sync_confirm')).toBe(true);
  await expect(page.getByText('Sync is on for this device.', { exact: false })).toBeVisible();
});

test('conflicts show both values and turning off asks first', async ({ page }) => {
  const sync = { joined: ['settings', 'dictionary'], paused: false, consent: { settings: true, dictionary: true }, pending: 2, last_success_ms: Date.now() - 120000, last_error: 'offline', notices: [],
    conflicts: [{ id: '00000000-0000-4000-8000-000000000002', category: 'settings', key: 'input_language', local: 'en', remote: 'he' }] };
  await open(page, { account_snapshot: signedIn, portable_status: status(sync) });
  await expect(page.getByText('Syncing settings and dictionary', { exact: true })).toBeVisible();
  await expect(page.getByText('2 changes waiting to upload.', { exact: false })).toBeVisible();
  await expect(page.getByText('Your account could not be reached.', { exact: false })).toBeVisible();
  await expect(page.getByText('On this PC: English')).toBeVisible();
  await expect(page.getByText('In your account: Hebrew')).toBeVisible();
  await page.getByRole('button', { name: 'Keep this device’s' }).click();
  expect((await calls(page)).find(call => call.command === 'sync_resolve')?.args).toEqual({ id: '00000000-0000-4000-8000-000000000002', choice: 'local' });
  await page.getByRole('button', { name: 'Turn off…' }).click();
  expect((await calls(page)).some(call => call.command === 'sync_turn_off')).toBe(false);
  await page.getByRole('button', { name: 'Turn off on all devices' }).click();
  expect((await calls(page)).filter(call => call.command === 'sync_turn_off').map(call => call.args.category)).toEqual(['settings', 'dictionary']);
});
