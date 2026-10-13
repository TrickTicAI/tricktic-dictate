import { test, expect } from '@playwright/test';

async function nativePage(page: import('@playwright/test').Page, processed: boolean) {
  await page.route('http://tauri.localhost/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith('/live-processing')) {
      // This fixture predates mode processing and uses a non-UUID job id, which the
      // real route rejects. An unbound job is what it has always meant to exercise.
      await route.fulfill({ status: 200, json: { api_version: '1', data: { bound: false, attempt: null } } });
    } else if (path.endsWith('/processed')) {
      await route.fulfill({ status: processed ? 200 : 503, json: processed ? { api_version: '1', data: { text: 'שלום — exact snippet' } } : {} });
    } else {
      const response = await route.fetch({ url: `http://127.0.0.1:15191${new URL(route.request().url()).pathname}${new URL(route.request().url()).search}` });
      await route.fulfill({ response });
    }
  });
  await page.addInitScript(() => {
    // Delivery bridge checks run as Windows; Mac capability polling is separate.
    Object.defineProperty(navigator, 'platform', { value: 'Win32' });
    const calls: unknown[] = [];
    Object.assign(window, { deliveryCalls: calls, __TAURI__: { core: { invoke: async (command: string, args: unknown) => {
      calls.push({ command, args }); return { copied: true, pasted: false, message: 'Copied.' };
    } } } });
  });
  await page.goto('http://tauri.localhost/src/features/settings/tests/shortcuts.html');
}

for (const processed of [true, false]) {
  test(`delivery deduplicates a completed job and uses ${processed ? 'processed' : 'fallback raw'} text`, async ({ page }) => {
    await nativePage(page, processed);
    const result = await page.evaluate(async () => {
      // This fixture invokes the real module without mounting an owner or
      // reading any microphone/clipboard state from the test machine.
      const { deliverTranscript } = await import('../../desktop-session/runtime/delivery');
      const job = { id: 'job-1', request: { recording_id: 'session-1' }, status: { state: 'succeeded', result: { transcript: { raw_text: 'raw Hebrew שלום' } } } };
      const [a, b] = await Promise.all([deliverTranscript(job as never), deliverTranscript(job as never)]);
      return { a, b, calls: (window as unknown as { deliveryCalls: unknown[] }).deliveryCalls };
    });
    expect(result.a.copied).toBe(true);
    expect(result.a).toEqual(result.b);
    // The native completion path is asked first. This fixture's native bridge
    // answers it with a receipt rather than text, which is what an older host
    // does, so the text comes from the HTTP path the test is about.
    expect(result.calls).toEqual([
      { command: 'prepare_delivery', args: { jobId: 'job-1' } },
      { command: 'deliver_transcript', args: { sessionId: 'session-1', jobId: 'job-1', text: processed ? 'שלום — exact snippet' : 'raw Hebrew שלום' } },
    ]);
  });
}

test('an empty dictation does not prepare, copy or paste text', async ({ page }) => {
  await nativePage(page, true);
  const result = await page.evaluate(async () => {
    const { deliverTranscript } = await import('../../desktop-session/runtime/delivery');
    const job = { id: 'silent-job', request: { recording_id: 'silent-session' }, status: { state: 'succeeded', result: { transcript: { raw_text: ' \n' } } } };
    return { outcome: await deliverTranscript(job as never), calls: (window as any).deliveryCalls };
  });
  expect(result.outcome).toEqual({ copied: false, pasted: false, message: 'No speech detected.' });
  expect(result.calls).toEqual([]);
});

for (const cancel of [false, true]) test(`bounded original delivery ${cancel ? 'respects cancellation' : 'inserts once after both models finish'}`, async ({ page }) => {
  await nativePage(page, true);
  await page.evaluate(() => {
    const w = window as any; w.deliveryCalls = []; w.wanted = true;
    w.__TAURI__.core.invoke = async (command: string, args: unknown) => {
      w.deliveryCalls.push({ command, args });
      if (command === 'prepare_delivery') return new Promise(resolve => { w.finishPolish = resolve; });
      return { copied: true, pasted: true, message: 'Inserted.' };
    };
  });
  await page.evaluate(async () => {
    const { deliverTranscript } = await import('../../desktop-session/runtime/delivery');
    const w = window as any;
    const job = { id: 'bounded-job', request: { recording_id: 'bounded-session' }, status: { state: 'succeeded', result: { transcript: { raw_text: 'raw words' } } } };
    w.deliveryRun = Promise.all([deliverTranscript(job as never, () => w.wanted), deliverTranscript(job as never)]);
  });
  expect(await page.evaluate(() => (window as any).deliveryCalls.length)).toBe(1);
  await page.evaluate(cancel => {
    const w = window as any; if (cancel) w.wanted = false;
    w.finishPolish({ text: 'Corrected original · exact snippet', original_used: true, notice: 'Polishing unavailable.' });
  }, cancel);
  const result = await page.evaluate(async () => ({ outcomes: await (window as any).deliveryRun, calls: (window as any).deliveryCalls }));
  expect(result.outcomes[0]).toEqual(result.outcomes[1]);
  expect(result.calls.map((c: any) => c.command)).toEqual(cancel ? ['prepare_delivery'] : ['prepare_delivery','deliver_transcript']);
  if (cancel) expect(result.outcomes[0].pasted).toBe(false);
  else { expect(result.outcomes[0].message).toContain('Original inserted'); expect((result.calls[1] as any).args.text).toBe('Corrected original · exact snippet'); }
  await page.evaluate(() => (window as any).finishPolish({text:'Late model answer',original_used:false}));
  expect(await page.evaluate(() => (window as any).deliveryCalls.length)).toBe(cancel ? 1 : 2);
});
