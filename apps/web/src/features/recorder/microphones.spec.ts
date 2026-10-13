import { stubOwnerSession, loginFixture } from '../auth/tests/fixture';
import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.route('**/api/v1/**', route => route.fulfill({ status: 503, body: 'test: offline' }));
  await stubOwnerSession(page);
  await page.goto('/'); await loginFixture(page);
});

test('selection persists and is passed exactly to actual browser capture', async ({ page }) => {
  const result = await page.evaluate(async () => {
    const path = '/src/features/recorder/microphones.ts';
    const microphones = await import(path);
    const list = await microphones.listMicrophones(true);
    const selected = list.devices[0].id;
    await microphones.selectMicrophone(selected);
    return { selected, constraints: microphones.microphoneConstraints() };
  });
  expect(result.constraints).toEqual({ audio: { deviceId: { exact: result.selected } } });
  const capture = await page.evaluate(async () => {
    const path = '/src/features/recorder/capture.ts';
    const { DurableCapture } = await import(path);
    const capture = new DurableCapture();
    await capture.start('en');
    const result = { state: capture.snapshot.state, error: capture.snapshot.error, selected: capture.activeStream?.getAudioTracks()[0]?.getSettings().deviceId };
    await capture.stopAndDrain();
    return result;
  });
  expect(capture).toEqual({ state: 'recording', error: null, selected: result.selected });
  await page.reload(); await loginFixture(page);
  expect(await page.evaluate(() => localStorage.getItem('writer.microphone.device'))).toBe(result.selected);
});

test('missing selected device fails explicitly without silently using the default', async ({ page }) => {
  const state = await page.evaluate(async () => {
    localStorage.setItem('writer.microphone.device', 'disconnected-input');
    const path = '/src/features/recorder/capture.ts';
    const capture = new (await import(path)).DurableCapture();
    await capture.start('en');
    return { snapshot: capture.snapshot, sessions: await capture.list() };
  });
  expect(state.snapshot.state).toBe('idle');
  expect(state.snapshot.error).toContain('selected microphone is unavailable');
  expect(state.sessions).toHaveLength(0);
});

test('microphone settings list real inputs and persist the user selection', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  // The device has its own section since the Settings regrouping.
  await page.getByRole('navigation', { name: 'Settings sections' }).getByRole('button', { name: 'Microphone & sound' }).click();
  await page.getByRole('button', { name: 'Allow access and refresh' }).click();
  const select = page.getByRole('combobox', { name: 'Microphone device' });
  await expect(select).toBeEnabled();
  const id = await select.locator('option').nth(1).getAttribute('value');
  await select.selectOption(id!);
  await expect(select).toHaveValue(id!);
  await page.reload(); await loginFixture(page);
  await expect(page.getByRole('combobox', { name: 'Microphone device' })).toHaveValue(id!);
});

test('quiet speech is visible and missing telemetry clears the waveform', async ({ page }) => {
  await page.evaluate(async () => {
    const reactPath = '/node_modules/.vite/deps/react.js';
    const rootPath = '/node_modules/.vite/deps/react-dom_client.js';
    const levelsPath = '/src/features/home/useLevels.ts';
    const { createElement } = (await import(reactPath)).default;
    const { createRoot } = (await import(rootPath)).default;
    const { useLevels } = await import(levelsPath);
    const owner = { subscribe: (listener: (event: unknown) => void) => {
      (window as any).publishLevel = (rms: number) => listener({ kind: 'level', rms });
      return () => {};
    } };
    const element = document.createElement('div'); element.id = 'test-meter'; document.body.append(element);
    function Meter() { const levels = useLevels(true, owner, undefined); return createElement('output', null, Math.max(...levels).toString()); }
    createRoot(element).render(createElement(Meter));
  });
  await expect(page.locator('#test-meter output')).toHaveText('0');
  await page.evaluate(() => (window as any).publishLevel(0.02));
  await expect.poll(async () => Number(await page.locator('#test-meter output').textContent())).toBeGreaterThan(0.4);
  await expect(page.locator('#test-meter output')).toHaveText('0');
  await page.evaluate(() => (window as any).publishLevel(0));
  await expect(page.locator('#test-meter output')).toHaveText('0');
});

test('native status events drive levels with throttled timers and reject stale sessions', async ({ page }) => {
  await page.route('http://tauri.localhost/**', async route => {
    if (new URL(route.request().url()).pathname === '/') {
      await route.fulfill({ contentType: 'text/html', body: '<html><body>Native bridge test</body></html>' });
    } else {
      const response = await route.fetch({ url: route.request().url().replace('http://tauri.localhost', 'http://127.0.0.1:15175') });
      await route.fulfill({ response });
    }
  });
  await page.goto('http://tauri.localhost/');
  const result = await page.evaluate(async () => {
    const id = crypto.randomUUID();
    const initial = { active: true, id, elapsed: 0, saved_bytes: 0, error: null, rms: 0, peak: 0, level_sequence: 0 };
    let statusEvent: ((value: { payload: typeof initial }) => void) | undefined;
    let unlistened = false;
    (window as any).__TAURI__ = {
      core: { invoke: async (name: string) => name === 'capture_stop' ? { ...initial, active: false } : initial },
      event: { listen: async (name: string, callback: typeof statusEvent) => {
        if (name === 'writer-capture-status') statusEvent = callback;
        return () => { unlistened = true; };
      } },
    };
    const path = '/src/features/desktop-session/runtime/native.ts';
    const { NativeCapture } = await import(path);
    const capture = new NativeCapture();
    const samples: number[] = [];
    capture.subscribeLevel((rms: number) => samples.push(rms));
    const originalTimer = window.setTimeout;
    window.setTimeout = (() => 0) as unknown as typeof window.setTimeout;
    try {
      await capture.start('en', id);
      statusEvent!({ payload: { ...initial, elapsed: 100, rms: 0.02, peak: 0.05, level_sequence: 1 } });
      statusEvent!({ payload: { ...initial, id: crypto.randomUUID(), rms: 1, peak: 1, level_sequence: 20 } });
      statusEvent!({ payload: { ...initial, rms: 0.02, peak: 0.05, level_sequence: 1 } });
      const liveState = capture.snapshot.state;
      await capture.stopAndDrain();
      statusEvent!({ payload: { ...initial, rms: 0.8, peak: 1, level_sequence: 2 } });
      const stoppedState = capture.snapshot.state;
      const stoppedRecordingMatches = capture.snapshot.recordingId === id;
      capture.dispose();
      return { samples, liveState, stoppedState, stoppedRecordingMatches, unlistened };
    } finally { window.setTimeout = originalTimer; }
  });
  expect(result).toEqual({ samples: [0.02], liveState: 'recording', stoppedState: 'idle', stoppedRecordingMatches: true, unlistened: true });
});
