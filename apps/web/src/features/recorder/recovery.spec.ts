import { stubOwnerSession, loginFixture } from '../auth/tests/fixture';
import { expect, test, type Page } from '@playwright/test';

async function setup(page: Page) {
  await page.route('**/api/v1/**', (route) => route.fulfill({ status: 503, body: 'test: API offline' }));
  await stubOwnerSession(page);
  await page.goto('/'); await loginFixture(page);
  await page.evaluate(async () => {
    const path = '/src/features/recorder/capture.ts';
    const { DurableCapture } = await import(path);
    (window as any).captureTest = new DurableCapture();
  });
}
async function start(page: Page) {
  await page.evaluate(() => (window as any).captureTest.start('he'));
  await expect.poll(() => page.evaluate(() => (window as any).captureTest.snapshot.state)).toBe('recording');
  await expect.poll(() => page.evaluate(async () => (await (window as any).captureTest.list())[0]?.byte_length ?? 0)).toBeGreaterThan(0);
}
async function stop(page: Page) {
  await page.evaluate(() => (window as any).captureTest.stop());
  await expect.poll(() => page.evaluate(() => (window as any).captureTest.snapshot.state)).toBe('idle');
}

test('completed real synthetic microphone audio survives refresh and plays', async ({ page }) => {
  await setup(page);
  await start(page);
  await stop(page);
  const before = await page.evaluate(async () => (await (window as any).captureTest.list())[0]);
  expect(before.capture_state).toBe('complete');
  await page.reload(); await loginFixture(page);
  await expect(page.locator('.session-row')).toHaveCount(1);
  const audio = page.locator('audio');
  await expect.poll(() => audio.evaluate((element: HTMLAudioElement) => element.readyState)).toBeGreaterThan(0);
  await audio.evaluate((element: HTMLAudioElement) => element.play());
  await expect.poll(() => audio.evaluate((element: HTMLAudioElement) => element.currentTime)).toBeGreaterThan(0);
  const download = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Save audio', exact: true }).click();
  expect(await (await download).failure()).toBeNull();
  await page.evaluate(async () => {
    const path = '/src/features/recorder/capture.ts';
    (window as any).captureTest = new (await import(path)).DurableCapture();
  });
  const after = await page.evaluate(async () => (await (window as any).captureTest.list())[0]);
  expect(after).toEqual(before);
});

test('abrupt tab close recovers committed prefix and marks capture interrupted', async ({ page, context }) => {
  await setup(page);
  await start(page);
  const saved = await page.evaluate(async () => (await (window as any).captureTest.list())[0]);
  await page.close();
  const recovered = await context.newPage();
  await setup(recovered);
  const session = await recovered.evaluate(async () => (await (window as any).captureTest.list())[0]);
  expect(session.id).toBe(saved.id);
  expect(session.capture_state).toBe('interrupted');
  expect(session.byte_length).toBeGreaterThanOrEqual(saved.byte_length);
  const audio = recovered.locator('audio');
  await expect.poll(() => audio.evaluate((element: HTMLAudioElement) => element.readyState)).toBeGreaterThan(0);
  await audio.evaluate((element: HTMLAudioElement) => element.play());
  await expect.poll(() => audio.evaluate((element: HTMLAudioElement) => element.currentTime)).toBeGreaterThan(0);
});

test('quota abort retains committed audio and presents an actionable stopped state', async ({ page }) => {
  await setup(page);
  await start(page);
  const saved = await page.evaluate(async () => (await (window as any).captureTest.list())[0]);
  await page.evaluate(() => {
    const original = IDBObjectStore.prototype.add;
    IDBObjectStore.prototype.add = function (value: any, key?: IDBValidKey) {
      if (this.name === 'chunks') throw new DOMException('Injected quota', 'QuotaExceededError');
      return original.call(this, value, key);
    };
  });
  await expect.poll(() => page.evaluate(() => (window as any).captureTest.snapshot.state)).toBe('idle');
  const result = await page.evaluate(async () => {
    const capture = (window as any).captureTest;
    const session = (await capture.list())[0];
    return { session, error: capture.snapshot.error, size: (await capture.exportRecording(session.id)).size };
  });
  expect(result.error).toContain('free browser storage');
  expect(result.session.capture_state).toBe('interrupted');
  expect(result.size).toBe(saved.byte_length);
  expect(result.session.byte_length).toBe(saved.byte_length);
});

test('unsupported format and microphone denial return actionable idle states', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => { MediaRecorder.isTypeSupported = () => false; });
  await page.evaluate(() => (window as any).captureTest.start('en'));
  expect(await page.evaluate(() => (window as any).captureTest.snapshot)).toMatchObject({ state: 'idle', error: expect.stringContaining('No supported audio') });
  await setup(page);
  await page.evaluate(() => { navigator.mediaDevices.getUserMedia = async () => { throw new DOMException('Denied', 'NotAllowedError'); }; });
  await page.evaluate(() => (window as any).captureTest.start('en'));
  expect(await page.evaluate(() => (window as any).captureTest.snapshot)).toMatchObject({ state: 'idle', error: expect.stringContaining('Microphone access was denied') });
});

test('a missing stop event recovers through timeout without losing saved chunks', async ({ page }) => {
  await setup(page);
  await start(page);
  await page.evaluate(() => { MediaRecorder.prototype.stop = function () {}; });
  await page.evaluate(() => (window as any).captureTest.stop());
  await expect.poll(() => page.evaluate(() => (window as any).captureTest.snapshot.state), { timeout: 8000 }).toBe('idle');
  expect(await page.evaluate(() => (window as any).captureTest.snapshot.error)).toContain('did not finish stopping');
  expect(await page.evaluate(async () => (await (window as any).captureTest.list())[0].capture_state)).toBe('interrupted');
});

test('explicit deletion removes only its own audio and metadata', async ({ page }) => {
  await setup(page);
  await start(page); await stop(page);
  await start(page); await stop(page);
  const sessions = await page.evaluate(async () => (window as any).captureTest.list());
  expect(sessions).toHaveLength(2);
  await page.evaluate((id) => (window as any).captureTest.deleteRecording(id), sessions[0].id);
  const remaining = await page.evaluate(async () => (window as any).captureTest.list());
  expect(remaining).toHaveLength(1);
  expect(remaining[0].id).toBe(sessions[1].id);
  expect(await page.evaluate(async (id) => (await (window as any).captureTest.exportRecording(id)).size, sessions[1].id)).toBeGreaterThan(0);
  expect(await page.evaluate(async (id) => {
    try { await (window as any).captureTest.exportRecording(id); return false; } catch { return true; }
  }, sessions[0].id)).toBe(true);
});

test('delayed events from an old recorder cannot change a new session', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => {
    const Original = MediaRecorder;
    (window as any).recorders = [];
    (window as any).MediaRecorder = class extends Original {
      constructor(stream: MediaStream, options: MediaRecorderOptions) {
        super(stream, options);
        (window as any).recorders.push(this);
      }
    };
  });
  await start(page);
  await page.evaluate(() => {
    const old = (window as any).recorders[0];
    (window as any).oldEvents = { data: old.ondataavailable, error: old.onerror, stop: old.onstop };
  });
  await stop(page);
  await start(page);
  const before = await page.evaluate(async () => (await (window as any).captureTest.list())[0]);
  await page.evaluate(() => {
    const events = (window as any).oldEvents;
    events.data({ data: new Blob(['stale']) });
    events.error(new Event('error'));
    events.stop(new Event('stop'));
  });
  expect(await page.evaluate(() => (window as any).captureTest.snapshot.state)).toBe('recording');
  expect(await page.evaluate(() => (window as any).captureTest.snapshot.error)).toBeNull();
  const after = await page.evaluate(async () => (await (window as any).captureTest.list())[0]);
  expect(after.id).toBe(before.id);
  await stop(page);
});

test('stop cancels a pending microphone request and releases a late stream', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => {
    const original = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getUserMedia = async (constraints) => {
      const stream = await original(constraints);
      (window as any).lateStream = stream;
      return new Promise<MediaStream>((resolve) => { (window as any).resolveMicrophone = () => resolve(stream); });
    };
    (window as any).startPromise = (window as any).captureTest.start('en');
  });
  await expect.poll(() => page.evaluate(() => !!(window as any).resolveMicrophone)).toBe(true);
  await page.evaluate(() => {
    (window as any).captureTest.stop();
    (window as any).resolveMicrophone();
    return (window as any).startPromise;
  });
  expect(await page.evaluate(() => (window as any).captureTest.snapshot.state)).toBe('idle');
  expect(await page.evaluate(() => (window as any).lateStream.getTracks().every((track: MediaStreamTrack) => track.readyState === 'ended'))).toBe(true);
  expect(await page.evaluate(async () => (window as any).captureTest.list())).toEqual([]);
});


test('microphone revocation preserves the committed audio prefix', async ({ page }) => {
  await setup(page);
  await start(page);
  const before = await page.evaluate(async () => (await (window as any).captureTest.list())[0]);
  await page.evaluate(() => {
    const track = (window as any).captureTest.activeStream.getAudioTracks()[0];
    // Browser permission/device loss reports ended; calling stop alone does not.
    track.stop();
    track.dispatchEvent(new Event('ended'));
  });
  await expect.poll(() => page.evaluate(() => (window as any).captureTest.snapshot.state)).toBe('idle');
  const after = await page.evaluate(async () => (await (window as any).captureTest.list())[0]);
  expect(after.id).toBe(before.id);
  expect(after.capture_state).toBe('interrupted');
  expect(after.byte_length).toBeGreaterThanOrEqual(before.byte_length);
  expect(await page.evaluate(async (id) => (await (window as any).captureTest.exportRecording(id)).size, before.id)).toBeGreaterThanOrEqual(before.byte_length);
});
