import { test, expect, type Page } from '@playwright/test';

// A native microphone that is open but has sent only digital silence -- a
// Bluetooth headset switching to its call profile -- is not yet listening.
async function setup(page: Page) {
  await page.route('**/warm-up-test', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Microphone warm-up fixture</title>' }));
  await page.goto('/warm-up-test');
  await page.evaluate(async () => {
    const { SessionOwner } = await import('/src/features/desktop-session/runtime/owner.ts' as string);
    const w = window as any;
    w.saved = {}; w.drains = []; w.states = [];
    const listeners = new Set<() => void>();
    const capture = {
      snapshot: { state: 'idle', error: null, elapsed: 0 } as any,
      supported: true, elapsed: 0, activeStream: undefined, id: '',
      store: { get: async (id: string) => w.saved[id], list: async () => [] },
      subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
      set(change: object) { this.snapshot = { ...this.snapshot, ...change }; listeners.forEach(f => f()); },
      async start(_language: string, id: string) { this.id = id; this.set({ state: 'requesting', listening: false }); this.set({ state: 'recording', listening: false }); },
      stop() { void this.stopAndDrain(false); },
      async stopAndDrain(transcribe?: boolean) {
        w.drains.push(transcribe);
        if (this.snapshot.state === 'idle') return;
        this.set({ state: 'stopping' });
        w.saved[this.id] = { id: this.id, byte_length: 3200, duration_ms: 100, capture_state: 'complete', created_at_ms: Date.now(),
          input_language: 'en', mime_type: 'audio/wav', chunk_count: 1 };
        this.set({ state: 'idle' });
      },
      list: async () => [], exportRecording: async () => new Blob(), deleteRecording: async () => {}, dispose() {},
    };
    w.capture = capture;
    w.owner = await SessionOwner.create({ settings: () => ({ language: 'en', options: null }), capture: capture as any,
      jobs: { load: async () => [], save: async () => {}, upload: async () => {}, submit: async () => { throw Error('offline'); },
        get: async () => { throw Error('offline'); } } as any });
    w.owner.subscribe((event: any) => { if (event.kind === 'snapshot') w.states.push(event.state); });
    w.send = (action: string, id?: string) => w.owner.dispatch({ version: 1, kind: 'command', epoch: w.owner.snapshot.epoch,
      command_id: crypto.randomUUID(), action, session_id: id ?? w.owner.snapshot.session_id });
  });
}
async function startWarming(page: Page) {
  await page.evaluate(() => (window as any).send('start', crypto.randomUUID()));
  await expect.poll(() => page.evaluate(() => (window as any).capture.snapshot.state)).toBe('recording');
}
const state = (page: Page) => page.evaluate(() => (window as any).owner.snapshot.state);

test('an open microphone reports Listening only once it sends sound', async ({ page }) => {
  await setup(page);
  await startWarming(page);
  expect(await state(page)).toBe('requesting');
  await page.evaluate(() => (window as any).capture.set({ listening: true }));
  await expect.poll(() => state(page)).toBe('capturing');
});

test('releasing during warm-up finishes the recording instead of dropping it', async ({ page }) => {
  await setup(page);
  await startWarming(page);
  expect(await page.evaluate(() => (window as any).send('stop'))).toMatchObject({ outcome: 'accepted' });
  await expect.poll(() => state(page)).toBe('saved');
  const result = await page.evaluate(() => ({ states: (window as any).states, drains: (window as any).drains }));
  expect(result.states).toEqual(['requesting', 'capturing', 'saving', 'saved']);
  expect(result.drains).toEqual([true]);
});

test('cancel during warm-up still discards', async ({ page }) => {
  await setup(page);
  await startWarming(page);
  await page.evaluate(() => (window as any).send('cancel'));
  await expect.poll(() => state(page)).toBe('canceled');
  expect(await page.evaluate(() => (window as any).drains)).toEqual([false]);
});

test('a microphone lost during warm-up ends as an interrupted recording, not a stuck start', async ({ page }) => {
  await setup(page);
  await startWarming(page);
  await page.evaluate(() => {
    const w = window as any;
    w.saved[w.capture.id] = { id: w.capture.id, byte_length: 1600, duration_ms: 50, capture_state: 'interrupted', created_at_ms: Date.now(),
      input_language: 'en', mime_type: 'audio/wav', chunk_count: 1 };
    w.capture.set({ state: 'idle', error: 'Device disconnected.' });
  });
  await expect.poll(() => page.evaluate(() => (window as any).owner.snapshot)).toMatchObject({ state: 'error', audio_saved: true });
});
