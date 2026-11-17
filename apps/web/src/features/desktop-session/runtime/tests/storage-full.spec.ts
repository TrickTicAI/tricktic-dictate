import { test, expect, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
const fixtures = JSON.parse(readFileSync(new URL('../../../../../../../packages/contracts/examples.json', import.meta.url), 'utf8'));

/*
 * A dictation refused because saved data reached the storage budget reports
 * `storage_full`, not a generic backend/storage failure, whichever step refused
 * it: the native start (the shell rejects with the code), the native import
 * (same), or the HTTP job submission (507 Insufficient Storage).
 */
type Refusal = 'import' | 'submit' | 'other' | 'start';
async function dictate(page: Page, refusal: Refusal) {
  await page.route('**/storage-full-test', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Storage full fixture</title>' }));
  await page.goto('/storage-full-test');
  await page.evaluate(async ({ fixtures, refusal }) => {
    const { SessionOwner } = await import('/src/features/desktop-session/runtime/owner.ts' as string);
    const { ApiFailure } = await import('/src/features/transcripts/api.ts' as string);
    const w = window as any;
    // A native host whose capture_start was refused: the start resolves with
    // the capture idle again, its error and code set, as NativeCapture does.
    const listeners = new Set<() => void>();
    const refusedStart = {
      store: { get: async () => undefined }, supported: true, elapsed: 0, activeStream: undefined,
      snapshot: { state: 'idle', error: null, elapsed: 0 } as Record<string, unknown>,
      subscribe(listener: () => void) { listeners.add(listener); return () => listeners.delete(listener); },
      async start() {
        this.snapshot = { state: 'idle', error: 'Storage is full.', errorCode: 'storage_full', elapsed: 0 };
        listeners.forEach(listener => listener());
      },
      stop() {}, async stopAndDrain() {}, async list() { return []; },
      async exportRecording() { return new Blob(); }, async deleteRecording() {}, dispose() {},
    };
    w.owner = await SessionOwner.create({
      settings: () => ({ language: 'en', options: fixtures.transcription_request.data.options }),
      ...(refusal === 'start' ? { capture: refusedStart } : {}),
      jobs: {
        load: async () => [], save: async () => {},
        upload: async () => {
          if (refusal === 'import') throw 'storage_full';
          if (refusal === 'other') throw 'Saved audio unavailable.';
        },
        submit: async () => { throw new ApiFailure('Storage is full.', 507); },
        get: async () => { throw new Error('unused'); },
      },
    });
    w.send = (action: string, id?: string) => w.owner.dispatch({ version: 1, kind: 'command', epoch: w.owner.snapshot.epoch,
      command_id: crypto.randomUUID(), action, session_id: id ?? w.owner.snapshot.session_id });
  }, { fixtures, refusal });
  expect(await page.evaluate(() => (window as any).send('start', crypto.randomUUID()))).toMatchObject({ outcome: 'accepted' });
  if (refusal !== 'start') {
    await expect.poll(() => page.evaluate(() => (window as any).owner.snapshot.state)).toBe('capturing');
    await expect.poll(() => page.evaluate(async () => {
      const w = window as any; return (await w.owner.capture.store.get(w.owner.snapshot.session_id))?.byte_length ?? 0;
    })).toBeGreaterThan(0);
    await page.evaluate(() => (window as any).send('stop'));
  }
  await expect.poll(() => page.evaluate(() => (window as any).owner.snapshot.state)).toBe('error');
  return page.evaluate(() => { const { error_code, audio_saved } = (window as any).owner.snapshot; return { error_code, audio_saved }; });
}

test('a native import refused for storage reports storage_full and keeps the audio', async ({ page }) => {
  expect(await dictate(page, 'import')).toEqual({ error_code: 'storage_full', audio_saved: true });
});

test('a job submission refused with 507 reports storage_full', async ({ page }) => {
  expect(await dictate(page, 'submit')).toEqual({ error_code: 'storage_full', audio_saved: true });
});

test('any other import failure keeps the generic code', async ({ page }) => {
  expect(await dictate(page, 'other')).toEqual({ error_code: 'backend_or_storage_unavailable', audio_saved: true });
});

test('a native start refused for storage reports storage_full', async ({ page }) => {
  expect(await dictate(page, 'start')).toEqual({ error_code: 'storage_full', audio_saved: false });
});
