import { expect, test } from '@playwright/test';

for (const slowStartup of [false, true]) test(`native stops update one recording without rescanning a large history (slow startup: ${slowStartup})`, async ({ page }) => {
  await page.route('**/history-fixture', route => route.fulfill({ contentType: 'text/html', body: '<div id="fixture"></div>' }));
  await page.goto('/history-fixture');
  await page.evaluate(async (slowStartup) => {
    const reactPath = '/node_modules/.vite/deps/react.js';
    const rootPath = '/node_modules/.vite/deps/react-dom_client.js';
    const recorderPath = '/src/recorder.ts';
    const { createElement } = (await import(reactPath)).default;
    const { createRoot } = (await import(rootPath)).default;
    const { useRecorder } = await import(recorderPath);
    const metadata = (id: string, created_at_ms: number) => ({ id, created_at_ms, input_language: 'en', duration_ms: 1000,
      byte_length: 32080, mime_type: 'audio/wav', capture_state: 'complete', chunk_count: 1 });
    const rows = Array.from({ length: 1200 }, (_, i) => metadata(`old-${i}`, i));
    const latest = metadata('new-dictation', 5000);
    const listeners = new Set<() => void>();
    const calls = { list: 0, get: [] as string[] };
    let releaseHistory: () => void;
    const pendingHistory = new Promise<typeof rows>(resolve => { releaseHistory = () => resolve(rows); });
    const capture = {
      snapshot: { state: 'idle', error: null, elapsed: 0 } as Record<string, unknown>, elapsed: 0, supported: true,
      subscribe: (listener: () => void) => { listeners.add(listener); return () => listeners.delete(listener); },
      list: async () => { ++calls.list; return slowStartup ? pendingHistory : rows; },
      store: { get: async (id: string) => { calls.get.push(id); return latest; } },
      exportRecording: async () => new Blob(['audio'], { type: 'audio/wav' }),
    };
    const owner = { capture, start: async () => {}, stop: async () => {} };
    function Fixture() {
      const recorder = useRecorder('en', owner);
      return createElement('output', null, `${recorder.recordings.length}:${recorder.recordings[0]?.id ?? ''}`);
    }
    (window as any).historyFixture = { calls, releaseHistory: () => releaseHistory(), finish: () => {
      capture.snapshot = { state: 'idle', error: null, elapsed: 1000, recordingId: latest.id };
      listeners.forEach(listener => listener());
    } };
    createRoot(document.getElementById('fixture')!).render(createElement(Fixture));
  }, slowStartup);
  await expect(page.locator('output')).toHaveText(slowStartup ? '0:' : '1200:old-0');
  await page.evaluate(() => (window as any).historyFixture.finish());
  if (slowStartup) {
    await expect(page.locator('output')).toHaveText('1:new-dictation');
    await page.evaluate(() => (window as any).historyFixture.releaseHistory());
  }
  await expect(page.locator('output')).toHaveText('1201:new-dictation');
  // Repeated idle notifications update the same row rather than duplicating it.
  await page.evaluate(() => (window as any).historyFixture.finish());
  await expect.poll(() => page.evaluate(() => (window as any).historyFixture.calls)).toEqual({ list: 1, get: ['new-dictation', 'new-dictation'] });
  await expect(page.locator('output')).toHaveText('1201:new-dictation');
});
