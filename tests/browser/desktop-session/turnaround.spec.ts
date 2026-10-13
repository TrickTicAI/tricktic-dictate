import { test, expect, type Page } from '@playwright/test';
import fixtures from '../../../packages/contracts/examples.json';

/*
 * The next dictation must not wait for the previous one. These drive the real
 * session owner with synthetic media and a fake job service in which each
 * recording's transcription, and each insertion, finish only when the test
 * says so.
 */
async function setup(page: Page) {
  await page.route('**/turnaround-test', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Turnaround fixture</title>' }));
  await page.goto('/turnaround-test');
  await page.evaluate(async fixtures => {
    const ownerPath = '/src/features/desktop-session/runtime/owner.ts';
    const { SessionOwner } = await import(ownerPath);
    const w = window as any;
    w.jobs = {}; w.deliveries = []; w.releases = {}; w.events = [];
    w.owner = await SessionOwner.create({
      settings: () => ({ language: 'en', options: structuredClone(fixtures.transcription_request.data.options) }),
      deliver: async (job: any) => {
        const id = job.request.recording_id;
        w.deliveries.push(id);
        return new Promise(resolve => { w.releases[id] = () => resolve({ copied: true, pasted: true, message: 'Pasted' }); });
      },
      jobs: {
        load: async () => [], save: async () => {}, upload: async () => {},
        submit: async (request: any) => {
          const job = { ...structuredClone(fixtures.job.data), id: crypto.randomUUID(), request: structuredClone(request), status: { state: 'queued' } };
          w.jobs[request.recording_id] = job;
          return structuredClone(job);
        },
        get: async (id: string) => {
          const job = Object.values(w.jobs).find((candidate: any) => candidate.id === id);
          if (!job) throw Error('offline');
          return structuredClone(job);
        },
        pollMs: 50,
      },
    });
    w.owner.subscribe((event: any) => { if (event.kind === 'snapshot') w.events.push({ state: event.state, session: event.session_id }); });
    w.waitingCounts = [];
    w.owner.subscribeWaiting(() => w.waitingCounts.push(w.owner.waiting));
    // What the job service does when one recording's transcription finishes.
    w.complete = (id: string) => { w.jobs[id].status = structuredClone(fixtures.job.data.status); };
    w.send = (action: string, id?: string) => w.owner.dispatch({ version: 1, kind: 'command', epoch: w.owner.snapshot.epoch,
      command_id: crypto.randomUUID(), action, session_id: id ?? w.owner.snapshot.session_id });
  }, fixtures);
}
const snapshot = (page: Page) => page.evaluate(() => (window as any).owner.snapshot);
const deliveries = (page: Page) => page.evaluate(() => (window as any).deliveries);
async function record(page: Page) {
  const id = await page.evaluate(() => crypto.randomUUID());
  expect(await page.evaluate(id => (window as any).send('start', id), id)).toMatchObject({ outcome: 'accepted' });
  await expect.poll(async () => (await snapshot(page)).state).toBe('capturing');
  await expect.poll(() => page.evaluate(async id => (await (window as any).owner.capture.store.get(id))?.byte_length ?? 0, id)).toBeGreaterThan(0);
  return id;
}
async function stop(page: Page, id: string) {
  await page.evaluate(id => (window as any).send('stop', id), id);
  await expect.poll(() => page.evaluate(id => !!(window as any).jobs[id], id)).toBe(true);
}
/** Blocks the next audio write, so a Stop stays in "saving" until released. */
async function holdNextWrite(page: Page) {
  await page.evaluate(() => {
    const w = window as any, store = w.owner.capture.store, append = store.append.bind(store);
    store.append = async (...args: any[]) => {
      store.append = append;
      await new Promise(resolve => { w.releaseWrite = resolve; });
      return append(...args);
    };
  });
}

test('a new dictation starts while the previous one is transcribing, and the earlier text is still inserted once', async ({ page }) => {
  await setup(page);
  const first = await record(page);
  await stop(page, first);
  await expect.poll(async () => (await snapshot(page)).state).toBe('queued');

  const second = await record(page);
  // Counted as still to come while the newer one records, for the panel's "+1".
  expect(await page.evaluate(() => (window as any).owner.waiting)).toBe(1);
  await page.evaluate(id => (window as any).complete(id), first);
  await expect.poll(() => deliveries(page)).toEqual([first]);
  // The earlier dictation finishes behind the newer one; it never takes the
  // panel back, not even when its insertion returns.
  await page.evaluate(id => (window as any).releases[id](), first);
  await page.waitForTimeout(200);
  expect(await snapshot(page)).toMatchObject({ state: 'capturing', session_id: second });
  expect(await page.evaluate(() => (window as any).waitingCounts)).toEqual([1, 0]);
  const events = await page.evaluate(() => (window as any).events);
  expect(events.slice(events.findIndex((event: any) => event.session === second)).every((event: any) => event.session === second)).toBe(true);

  await stop(page, second);
  await page.evaluate(id => (window as any).complete(id), second);
  await expect.poll(() => deliveries(page)).toEqual([first, second]);
  // A live dictation is not finished until its text is in place.
  expect((await snapshot(page)).state).toBe('transcribing');
  await page.evaluate(id => (window as any).releases[id](), second);
  await expect.poll(async () => (await snapshot(page)).state).toBe('success');

  // A repeated completion event inserts nothing twice, and nothing is left
  // holding history deletion back.
  await page.evaluate(async id => { const w = window as any; await w.owner.jobs.updateJob(w.owner.jobs.get(id).job); }, first);
  expect(await deliveries(page)).toEqual([first, second]);
  await expect.poll(() => page.evaluate(() => (window as any).writerCanDeleteHistory())).toBe(true);
});

test('a start pressed while the previous text is being inserted hands that insertion to the background', async ({ page }) => {
  await setup(page);
  const first = await record(page);
  await stop(page, first);
  await page.evaluate(id => (window as any).complete(id), first);
  await expect.poll(() => deliveries(page)).toEqual([first]);
  expect((await snapshot(page)).state).toBe('transcribing');

  const second = await record(page);
  await page.evaluate(id => (window as any).releases[id](), first);
  await page.waitForTimeout(200);
  expect(await snapshot(page)).toMatchObject({ state: 'capturing', session_id: second });
  expect(await deliveries(page)).toEqual([first]);
});

test('a start pressed while the previous recording is still being saved waits for it, and both are kept', async ({ page }) => {
  await setup(page);
  const first = await record(page);
  await holdNextWrite(page);
  await page.evaluate(id => { void (window as any).send('stop', id); }, first);
  await expect.poll(() => page.evaluate(() => !!(window as any).releaseWrite)).toBe(true);
  expect((await snapshot(page)).state).toBe('saving');

  const second = await page.evaluate(() => crypto.randomUUID());
  await page.evaluate(id => { const w = window as any; w.secondAck = w.send('start', id); }, second);
  await page.waitForTimeout(100);
  // Nothing starts over the drain.
  expect(await snapshot(page)).toMatchObject({ state: 'saving', session_id: first });
  await page.evaluate(() => (window as any).releaseWrite());

  expect(await page.evaluate(() => (window as any).secondAck)).toMatchObject({ outcome: 'accepted' });
  await expect.poll(async () => (await snapshot(page)).session_id).toBe(second);
  await expect.poll(async () => (await snapshot(page)).state).toBe('capturing');
  // The first was saved and submitted before the second began.
  await expect.poll(() => page.evaluate(id => !!(window as any).jobs[id], first)).toBe(true);
  const events = await page.evaluate(() => (window as any).events);
  expect(events.some((event: any) => event.session === first && event.state === 'saved')).toBe(false);
  expect(events.some((event: any) => event.session === second && event.state === 'requesting')).toBe(true);
  expect(await page.evaluate(async id => (await (window as any).owner.capture.store.get(id))?.capture_state, first)).toBe('complete');
});

test('letting go of a start that is still waiting for the previous save abandons it', async ({ page }) => {
  await setup(page);
  const first = await record(page);
  await holdNextWrite(page);
  await page.evaluate(id => { void (window as any).send('stop', id); }, first);
  await expect.poll(() => page.evaluate(() => !!(window as any).releaseWrite)).toBe(true);

  const second = await page.evaluate(() => crypto.randomUUID());
  await page.evaluate(id => { const w = window as any; w.secondAck = w.send('start', id); }, second);
  expect(await page.evaluate(id => (window as any).send('stop', id), second)).toMatchObject({ outcome: 'accepted' });
  await page.evaluate(() => (window as any).releaseWrite());

  expect(await page.evaluate(() => (window as any).secondAck)).toMatchObject({ outcome: 'rejected', error_code: 'canceled' });
  await expect.poll(() => page.evaluate(id => !!(window as any).jobs[id], first)).toBe(true);
  expect((await snapshot(page)).session_id).toBe(first);
  expect(await page.evaluate(async id => (await (window as any).owner.capture.store.get(id)) ?? null, second)).toBeNull();
});
