import { test, expect, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
const fixtures = JSON.parse(readFileSync(new URL('../../../../../../../packages/contracts/examples.json', import.meta.url), 'utf8'));

/*
 * Cancelling is the "never mind" gesture: the dictation and its audio go away.
 * Stopping, quitting and every failure path still keep whatever was saved.
 */
async function setup(page: Page) {
  await page.route('**/discard-owner-test', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Discard fixture</title>' }));
  await page.goto('/discard-owner-test');
  await page.evaluate(async fixtures => {
    const path = '/src/features/desktop-session/runtime/owner.ts';
    const { SessionOwner } = await import(path);
    const w = window as any;
    w.completed = []; w.releases = []; w.forgotten = []; w.deletedIds = []; w.refuseDelete = false;
    w.owner = await SessionOwner.create({
      settings: () => ({ language: 'en', options: fixtures.transcription_request.data.options }),
      deliver: async () => new Promise(resolve => w.releases.push(() => resolve({ copied: false, pasted: false, message: 'Fixture complete' }))),
      jobs: {
        load: async () => [], save: async () => {}, upload: async () => {},
        forget: async (id: string) => { w.forgotten.push(id); },
        submit: async (request: any) => {
          const job = { ...structuredClone(fixtures.job.data), id: crypto.randomUUID(), request };
          w.completed.push(job); return job;
        }, get: async (id: string) => w.completed.find((j: any) => j.id === id),
      },
    });
    const realDelete = w.owner.capture.deleteRecording.bind(w.owner.capture);
    w.owner.capture.deleteRecording = async (id: string) => {
      w.deletedIds.push(id);
      if (w.refuseDelete) throw new Error('Recording has queued or active transcription.');
      return realDelete(id);
    };
    w.send = (action: string, id?: string) => w.owner.dispatch({ version: 1, kind: 'command', epoch: w.owner.snapshot.epoch,
      command_id: crypto.randomUUID(), action, session_id: id ?? w.owner.snapshot.session_id });
    w.record = async () => {
      const id = crypto.randomUUID();
      await w.send('start', id);
      return id;
    };
    w.stored = async (id: string) => (await w.owner.capture.store.get(id))?.byte_length ?? 0;
  }, fixtures);

  const capturing = async () => {
    await expect.poll(() => page.evaluate(() => (window as any).owner.snapshot.state)).toBe('capturing');
    await expect.poll(() => page.evaluate(() => (window as any).stored((window as any).owner.snapshot.session_id))).toBeGreaterThan(0);
    return page.evaluate(() => (window as any).owner.snapshot.session_id as string);
  };
  const state = () => page.evaluate(() => (window as any).owner.snapshot);
  return { capturing, state };
}

test('cancel discards the recording, stop keeps it, and a refused delete still lists it', async ({ page }) => {
  const { capturing, state } = await setup(page);

  // Cancelled while capturing: nothing is left to show under history.
  await page.evaluate(() => (window as any).record());
  const discarded = await capturing();
  await page.evaluate(() => (window as any).send('cancel'));
  await expect.poll(async () => (await state()).state).toBe('canceled');
  expect((await state()).audio_saved).toBe(false);
  await expect.poll(() => page.evaluate((id) => (window as any).stored(id), discarded)).toBe(0);
  expect(await page.evaluate(() => (window as any).forgotten)).toContain(discarded);

  // Stopping is untouched: the audio survives and the transcript is delivered.
  await page.evaluate(() => (window as any).record());
  const kept = await capturing();
  await page.evaluate(() => (window as any).send('stop'));
  await expect.poll(() => page.evaluate(() => (window as any).releases.length)).toBe(1);
  await page.evaluate(() => (window as any).releases[0]());
  await expect.poll(async () => (await state()).state).toBe('success');
  expect(await page.evaluate((id) => (window as any).stored(id), kept)).toBeGreaterThan(0);

  // A delete the store refuses reports the recording as still saved rather
  // than claiming a discard that did not happen.
  await page.evaluate(() => { (window as any).refuseDelete = true; });
  await page.evaluate(() => (window as any).record());
  const refused = await capturing();
  await page.evaluate(() => (window as any).send('cancel'));
  await expect.poll(async () => (await state()).state).toBe('canceled');
  expect((await state()).audio_saved).toBe(true);
  expect(await page.evaluate((id) => (window as any).stored(id), refused)).toBeGreaterThan(0);

  // Quitting drains and cancels too, and must never delete the audio.
  await page.evaluate(() => (window as any).record());
  const onQuit = await capturing();
  // Earlier refused deletions retry in the background. Check this session,
  // not a global counter that includes those unrelated retries.
  expect(await page.evaluate(() => (window as any).deletedIds)).not.toContain(onQuit);
  await page.evaluate(() => (window as any).owner.stopAndDrain());
  await expect.poll(async () => (await state()).state).toBe('canceled');
  expect((await state()).audio_saved).toBe(true);
  expect(await page.evaluate((id) => (window as any).stored(id), onQuit)).toBeGreaterThan(0);
  expect(await page.evaluate(() => (window as any).deletedIds)).not.toContain(onQuit);
});

/*
 * The window's Discard offers Undo for a few seconds, so its delete waits that
 * long. Keeping the recording leaves it in history, untranscribed and unpasted;
 * letting the window pass deletes it exactly as an immediate discard does.
 */
test('the window\'s Discard deletes only after its Undo window, and keep() brings the recording back', async ({ page }) => {
  const { capturing, state } = await setup(page);

  await page.evaluate(() => (window as any).record());
  const kept = await capturing();
  await page.evaluate(() => (window as any).owner.discardWithUndo(1500));
  await expect.poll(async () => (await state()).state).toBe('canceled');
  // Nothing is claimed deleted during the window.
  expect((await state()).audio_saved).toBe(true);
  expect(await page.evaluate((id) => (window as any).stored(id), kept)).toBeGreaterThan(0);
  expect(await page.evaluate((id) => (window as any).owner.keep(id), kept)).toBe(true);
  await page.waitForTimeout(2600);
  expect(await page.evaluate((id) => (window as any).stored(id), kept)).toBeGreaterThan(0);
  expect(await page.evaluate(() => (window as any).deletedIds)).not.toContain(kept);
  expect(await page.evaluate(() => (window as any).releases.length)).toBe(0);

  await page.evaluate(() => (window as any).record());
  const gone = await capturing();
  await page.evaluate(() => (window as any).owner.discardWithUndo(800));
  await expect.poll(async () => (await state()).state).toBe('canceled');
  expect(await page.evaluate((id) => (window as any).stored(id), gone)).toBeGreaterThan(0);
  await expect.poll(() => page.evaluate((id) => (window as any).stored(id), gone)).toBe(0);
  await expect.poll(async () => (await state()).audio_saved).toBe(false);
  expect(await page.evaluate((id) => (window as any).owner.keep(id), gone)).toBe(false);
  expect(await page.evaluate(() => (window as any).forgotten)).toContain(gone);
});
