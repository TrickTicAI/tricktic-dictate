import { expect, test, type Page } from '@playwright/test';
const original = { schema_version: 1, id: '11111111-1111-4111-8111-000000000020', origin_device_id: '11111111-1111-4111-8111-000000000001', category: 'transcripts', revision: 1,
  content: { state: 'live', data: { kind: 'original', attempt_id: '11111111-1111-4111-8111-000000000021', recorded_at_ms: 1789948800000, input_language: 'mixed', output_language: { mode: 'preserve' }, selection: { provider: 'gemini', model: 'synthetic-stt' }, text: 'שלום Writer, English é 👋' } } };
const edited = { ...original, id: '11111111-1111-4111-8111-000000000024', content: { state: 'live', data: { kind: 'edited', original_record_id: original.id, parent_record_id: original.id, created_at_ms: 1789948800001, text: 'עריכה Edited sibling' } } };
async function open(page: Page, joined = false) {
  await page.route('http://tauri.localhost/**', async route => {
    const response = await route.fetch({ url: route.request().url().replace('http://tauri.localhost', 'http://127.0.0.1:15231') }); await route.fulfill({ response });
  });
  await page.addInitScript(({ joined, original, edited }) => {
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    Object.assign(window, { nativeCalls: calls, __TAURI__: {
      core: { invoke: async (command: string, args: Record<string, unknown>) => {
        calls.push({ command, args });
        if (command === 'account_snapshot') return { state: 'signed_in', availability: 'ready', generation: 1, reauthenticating: false, accountKey: { projectId: 'demo-metrix-writer', uid: 'synthetic' }, display: { email: 'synthetic@example.test', name: null } };
        if (command === 'portable_status') return { sync_available: true, last_import: null, kept: [], theme: null, sync: null };
        if (command === 'transcript_status') return { joined, pending: 0, copies: 2, error: null, last_success_ms: null, not_synced: 0 };
        if (command === 'transcript_preview') return { id: 'preview', mode: args.mode, local_records: args.mode === 'include_existing' ? 2 : 0, trust: 'firebase-service-readable-v1' };
        if (command === 'transcript_page') return { records: args.after ? [edited] : [original], next: args.after ? null : original.id };
        return null;
      } }, event: { listen: async () => () => {} },
    } });
  }, { joined, original, edited });
  await page.goto('http://tauri.localhost/src/features/account/tests/account.html');
}
const calls = (p: Page) => p.evaluate(() => (window as unknown as { nativeCalls: { command: string; args: Record<string, unknown> }[] }).nativeCalls);
test('history requires its own preview, selection and service-access acknowledgement', async ({ page }) => {
  await open(page);
  await expect(page.getByText('Transcript sync is off', { exact: true })).toBeVisible();
  expect((await calls(page)).filter(c => ['transcript_preview', 'transcript_confirm', 'transcript_page'].includes(c.command))).toHaveLength(0);
  await page.getByRole('button', { name: 'Set up history…' }).click();
  await expect(page.getByLabel('Future attempts only')).toBeChecked();
  await page.getByLabel('Include existing history shown in the preview').check();
  await page.getByRole('button', { name: 'Preview history upload' }).click();
  await expect(page.getByText('2 existing text records will be enrolled.', { exact: false })).toBeVisible();
  const enable = page.getByRole('button', { name: 'Enable transcript sync' }); await expect(enable).toBeDisabled();
  await page.getByLabel('I agree to upload transcript text to Firebase', { exact: false }).check(); await enable.click();
  expect((await calls(page)).find(c => c.command === 'transcript_confirm')?.args).toEqual({ id: 'preview', trust: 'firebase-service-readable-v1' });
});
test('paged restore preserves Unicode, exposes versions, and disables audio without triggering jobs', async ({ page }) => {
  await open(page, true);
  await page.getByRole('button', { name: 'Restore a page from account' }).click();
  await expect(page.getByText(original.content.data.text, { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Play audio' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Retry transcription' })).toBeDisabled();
  await page.getByRole('button', { name: 'Load next page' }).click();
  await expect(page.getByText(edited.content.data.text, { exact: true })).toBeVisible();
  await expect(page.getByText(original.content.data.text, { exact: true })).toBeVisible();
  expect((await calls(page)).filter(c => c.command === 'transcript_page').map(c => c.args)).toEqual([{ after: null, restore: true }, { after: original.id, restore: true }]);
  expect((await calls(page)).filter(c => /capture_|deliver_|transcription|cleanup/.test(c.command))).toHaveLength(0);
  await page.getByRole('button', { name: 'Save an edited version…' }).first().click();
  await page.getByLabel('Edited text').fill('new שלום'); await page.getByRole('button', { name: 'Save version', exact: true }).click();
  expect((await calls(page)).find(c => c.command === 'transcript_edit')?.args).toEqual({ parent: original.id, text: 'new שלום' });
});
