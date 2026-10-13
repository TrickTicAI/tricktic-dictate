import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
const fixtures = JSON.parse(readFileSync(new URL('../../../../../../../packages/contracts/examples.json', import.meta.url), 'utf8'));

test('canceling slow mode processing permits recording again and ignores old completion', async ({ page }) => {
  await page.route('**/mode-owner-test', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Mode cancellation fixture</title>' }));
  await page.goto('/mode-owner-test');
  await page.evaluate(async fixtures => {
    const path = '/src/features/desktop-session/runtime/owner.ts';
    const { SessionOwner } = await import(path);
    const w = window as any;
    w.completed = []; w.releases = [];
    w.owner = await SessionOwner.create({
      settings: () => ({ language: 'en', options: fixtures.transcription_request.data.options }),
      deliver: async () => new Promise(resolve => w.releases.push(() => resolve({ copied: false, pasted: false, message: 'Fixture complete' }))),
      jobs: {
        load: async () => [], save: async () => {}, upload: async () => {},
        submit: async (request: any) => {
          const job = { ...structuredClone(fixtures.job.data), id: crypto.randomUUID(), request };
          w.completed.push(job); return job;
        }, get: async (id: string) => w.completed.find((j: any) => j.id === id),
      },
    });
    w.send = (action: string, id?: string) => w.owner.dispatch({ version: 1, kind: 'command', epoch: w.owner.snapshot.epoch,
      command_id: crypto.randomUUID(), action, session_id: id ?? w.owner.snapshot.session_id });
  }, fixtures);
  for (let index = 0; index < 2; index++) {
    expect(await page.evaluate(() => (window as any).send('start', crypto.randomUUID()))).toMatchObject({ outcome: 'accepted' });
    await expect.poll(() => page.evaluate(() => (window as any).owner.snapshot.state)).toBe('capturing');
    await expect.poll(() => page.evaluate(async () => {
      const w = window as any; return (await w.owner.capture.store.get(w.owner.snapshot.session_id))?.byte_length ?? 0;
    })).toBeGreaterThan(0);
    await page.evaluate(() => (window as any).send('stop'));
    await expect.poll(() => page.evaluate(() => (window as any).releases.length)).toBe(index + 1);
    if (index === 0) await page.evaluate(() => (window as any).send('cancel'));
  }
  await page.evaluate(() => (window as any).releases[0]());
  await page.waitForTimeout(100);
  // The discarded dictation's late completion changes nothing: the second one
  // is still being inserted. (Starting a third meanwhile is accepted; see
  // tests/browser/desktop-session/turnaround.spec.ts.)
  expect(await page.evaluate(() => (window as any).owner.snapshot.state)).toBe('transcribing');
  await page.evaluate(() => (window as any).releases[1]());
  await expect.poll(() => page.evaluate(() => (window as any).owner.snapshot.state)).toBe('success');
});
