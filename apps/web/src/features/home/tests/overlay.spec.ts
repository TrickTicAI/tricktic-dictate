import { expect, test, type Page } from '@playwright/test';

async function setup(page: Page) {
  await page.addInitScript(() => {
    const listeners: Record<string, (event: { payload: unknown }) => void> = {};
    const calls: string[] = [];
    const shown: unknown[] = [];
    Object.assign(window, { overlayFixture: { listeners, calls, shown }, __TAURI__: {
      event: { listen: async (name: string, listener: (event: { payload: unknown }) => void) => { listeners[name] = listener; } },
      core: { invoke: async (_command: string, options: { action: string; shown?: unknown }) => {
        calls.push(options.action);
        if (options.shown) shown.push(options.shown);
      } },
    } });
  });
  await page.setViewportSize({ width: 112, height: 44 });
  await page.goto('/desktop-overlay.html');
}
async function emit(page: Page, name: string, payload: unknown) {
  await page.evaluate(({ name, payload }) => {
    (window as unknown as { overlayFixture: { listeners: Record<string, (event: { payload: unknown }) => void> } }).overlayFixture.listeners[name]({ payload });
  }, { name, payload });
}
type Shown = { session_id: string; state: string; received_ms: number; painted_ms: number; via: string };
const shownReports = (page: Page) => page.evaluate(() => (window as unknown as { overlayFixture: { shown: Shown[] } }).overlayFixture.shown);
/** Fill percentage painted into each processing dot, in order. */
async function fills(page: Page) {
  return page.locator('#loading i').evaluateAll((nodes) => nodes.map((node) => {
    const match = /([\d.]+)%/.exec((node as HTMLElement).style.backgroundImage);
    return match ? Number(match[1]) : -1;
  }));
}

const snapshot = (state: string, revision = 1) => ({ kind: 'snapshot', epoch: 'epoch', session_id: 'session', revision, state, audio_saved: true });
const level = (rms: number, sequence = 1, revision = 1) => ({ kind: 'level', epoch: 'epoch', session_id: 'session', revision, sequence, rms, peak: rms * 2 });

test('quiet real levels animate without claiming there is no speech; stale meter stays neutral', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-session', snapshot('capturing'));
  await emit(page, 'writer-session', level(0.004));
  expect(await page.locator('#bars i').last().evaluate(element => parseFloat((element as HTMLElement).style.height))).toBeGreaterThan(3);
  await emit(page, 'writer-session', level(0, 2));
  await expect(page.locator('#detail')).toHaveText('Recording');
  await emit(page, 'writer-session', level(1, 1)); // out-of-order frame ignored
  await expect(page.locator('#bars i').last()).toHaveAttribute('style', 'height: 4px;');
  await expect(page.locator('#detail')).toHaveText('Recording · meter unavailable', { timeout: 3000 });
  await page.locator('section').hover();
  await expect(page.getByRole('button', { name: 'Stop recording' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollHeight)).toBe(44);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(112);
});

test('processing keeps filling where the system asks for reduced motion; completion has no screen', async ({ page }) => {
  // Windows reports its animation-effects toggle to WebView2 as this media
  // preference. It used to stop the processing indicator on a machine that had
  // simply turned window animations off, which reads as a hung dictation.
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await setup(page);
  await emit(page, 'writer-preferences', { auto_dismiss: false, completion_hold_ms: 1800, reduced_motion: false });
  await emit(page, 'writer-session', snapshot('transcribing'));
  await expect(page.locator('#loading')).toBeVisible();

  const start = await fills(page);
  expect(start).toHaveLength(3);
  await expect.poll(async () => (await fills(page)).join(), { timeout: 4000 }).not.toBe(start.join());
  // A sweep rather than an on/off blink: a dot is caught part way through.
  await expect.poll(async () => (await fills(page)).some((v) => v > 0 && v < 100), { timeout: 4000 }).toBe(true);

  // Writer's own preference is what reduces it, and it steps rather than stops.
  await emit(page, 'writer-preferences', { auto_dismiss: false, reduced_motion: true });
  const stepped: number[][] = [];
  for (let sample = 0; sample < 6; sample++) {
    stepped.push(await fills(page));
    await page.waitForTimeout(200);
  }
  expect(stepped.flat().every((v) => v === 0 || v === 100)).toBe(true);
  expect(new Set(stepped.map((values) => values.join())).size).toBeGreaterThan(1);

  await emit(page, 'writer-session', snapshot('success', 2));
  await expect(page.locator('section')).toBeHidden();
  await emit(page, 'writer-session', snapshot('capturing', 3));
  await expect(page.locator('#bars')).toBeVisible();
  await expect(page.locator('#loading')).toBeHidden();
  await emit(page, 'writer-session', snapshot('idle', 4));
  await expect(page.locator('section')).toBeHidden();
});

const panel = (over: Record<string, unknown> = {}) =>
  ({ auto_dismiss: true, completion_hold_ms: 1200, reduced_motion: false, visibility: 'recording', scale: 1, ...over });

test('an always-visible panel stays up between dictations and starts the next one', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-preferences', panel({ visibility: 'always' }));
  await emit(page, 'writer-session', snapshot('idle'));
  await expect(page.locator('section')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start dictation' })).toBeVisible();
  // There is nothing to dismiss to when the panel is meant to stay on screen.
  await expect(page.getByRole('button', { name: 'Dismiss panel' })).toBeHidden();

  await page.getByRole('button', { name: 'Start dictation' }).click();
  expect(await page.evaluate(() => (window as unknown as { overlayFixture: { calls: string[] } }).overlayFixture.calls)).toContain('start');

  // It is a dictation panel again while one is running.
  await emit(page, 'writer-session', snapshot('capturing', 2));
  await expect(page.getByRole('button', { name: 'Start dictation' })).toBeHidden();
  await expect(page.getByRole('button', { name: 'Stop recording' })).toBeVisible();
  await emit(page, 'writer-session', snapshot('success', 3));
  await expect(page.locator('section')).toBeVisible();

  // Changing the preference alone puts it back to appearing only while dictating.
  await emit(page, 'writer-preferences', panel());
  await expect(page.locator('section')).toBeHidden();
});

test('the panel size preference scales what is inside the pill', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-preferences', panel());
  await emit(page, 'writer-session', snapshot('transcribing'));
  const dot = page.locator('#loading i').first();
  const standard = (await dot.boundingBox())!.width;
  await emit(page, 'writer-preferences', panel({ scale: 2 }));
  const doubled = (await dot.boundingBox())!.width;
  // The window itself is resized natively; what this proves is that the
  // contents grow with it instead of leaving a bigger empty pill.
  expect(doubled).toBeGreaterThan(standard * 1.5);
  await emit(page, 'writer-preferences', panel({ scale: 0.75 }));
  expect((await dot.boundingBox())!.width).toBeLessThan(standard);
});

test('a pending saved-audio dismissal cannot survive a new recording', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-preferences', { auto_dismiss: true, completion_hold_ms: 300 });
  await emit(page, 'writer-session', snapshot('saved'));
  await emit(page, 'writer-session', snapshot('capturing', 2));
  await page.waitForTimeout(400);
  const calls = await page.evaluate(() => (window as unknown as { overlayFixture: { calls: string[] } }).overlayFixture.calls);
  expect(calls).not.toContain('dismiss');
});

const paper = { kind: 'light', bg: '#fdfbf7', border: '#00000030', text: '#1f1b16', text_muted: '#6b6155', accent: '#b4541e', accent_text: '#fffdf9', surface: '#00000010', danger: '#b3261e' };
const computed = (page: Page, selector: string, property: 'backgroundColor' | 'borderTopColor') =>
  page.locator(selector).first().evaluate((node, property) => getComputedStyle(node)[property], property);

test('panel colours replace the theme fill and voice animation, and clear back to the theme', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-theme', paper);
  await emit(page, 'writer-preferences', panel());
  await emit(page, 'writer-session', snapshot('capturing'));
  expect(await computed(page, 'section', 'backgroundColor')).toBe('rgb(253, 251, 247)');
  expect(await computed(page, '#bars i', 'backgroundColor')).toBe('rgb(31, 27, 22)');

  await emit(page, 'writer-preferences', panel({ panel_colors: { background: '#282a36', wave: '#bd93f9' } }));
  expect(await computed(page, 'section', 'backgroundColor')).toBe('rgb(40, 42, 54)');
  expect(await computed(page, '#bars i', 'backgroundColor')).toBe('rgb(189, 147, 249)');
  expect(await computed(page, 'section', 'borderTopColor')).toBe('rgba(189, 147, 249, 0.2)');
  // A theme change underneath does not paint over the panel's own colours.
  await emit(page, 'writer-theme', { ...paper, bg: '#000000' });
  expect(await computed(page, 'section', 'backgroundColor')).toBe('rgb(40, 42, 54)');

  // The processing dots' unfilled base follows the animation colour instead of
  // staying the fixed white that vanished on a light panel.
  await emit(page, 'writer-preferences', panel({ panel_colors: { background: '#ffffff', wave: '#000000' } }));
  await emit(page, 'writer-session', snapshot('transcribing', 2));
  expect(await computed(page, '#loading i', 'backgroundColor')).not.toBe('rgba(255, 255, 255, 0.18)');

  // Only a literal colour is painted; anything else leaves the theme's.
  await emit(page, 'writer-preferences', panel({ panel_colors: { background: 'red', wave: '#bd93f9' } }));
  expect(await computed(page, 'section', 'backgroundColor')).toBe('rgb(0, 0, 0)');
  await emit(page, 'writer-preferences', panel({ panel_colors: { background: '#282a36', wave: '#bd93f9' } }));
  await emit(page, 'writer-preferences', panel({ panel_colors: null }));
  expect(await computed(page, 'section', 'backgroundColor')).toBe('rgb(0, 0, 0)');
  expect(await computed(page, 'section', 'borderTopColor')).toBe('rgba(0, 0, 0, 0.19)');
});

test('preview mode is driven only by the page that frames it and reaches no shell', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.route('**/panel-host', (route) => route.fulfill({ contentType: 'text/html',
    body: '<!doctype html><iframe src="/desktop-overlay.html?preview" style="width:224px;height:88px;border:0"></iframe>' }));
  await page.goto('/panel-host');
  const frame = page.frameLocator('iframe');
  // It starts, with no Tauri API anywhere on the page.
  await expect(frame.locator('body')).toHaveAttribute('data-state', 'idle');
  const post = (name: string, payload: unknown) => page.evaluate(({ name, payload }) => {
    document.querySelector('iframe')!.contentWindow!.postMessage({ source: 'writer-panel-preview', name, payload }, location.origin);
  }, { name, payload });

  await post('writer-preferences', panel({ scale: 2 }));
  await post('writer-session', snapshot('capturing'));
  await expect(frame.locator('body')).toHaveAttribute('data-state', 'capturing');
  await expect.poll(() => frame.locator('html').evaluate((root) => getComputedStyle(root).getPropertyValue('--scale').trim())).toBe('2');
  await post('writer-session', level(0.02));
  await expect.poll(() => frame.locator('#bars i').nth(5).evaluate((bar) => parseFloat((bar as HTMLElement).style.height))).toBeGreaterThan(10);

  // Neither a message the frame posts to itself nor one without the preview's
  // mark is taken as coming from Settings.
  await frame.locator('body').evaluate(() => window.postMessage({ source: 'writer-panel-preview', name: 'writer-session',
    payload: { kind: 'snapshot', epoch: 'epoch', session_id: 'session', revision: 5, state: 'transcribing', audio_saved: true } }, '*'));
  await page.evaluate(() => document.querySelector('iframe')!.contentWindow!.postMessage({ name: 'writer-session',
    payload: { kind: 'snapshot', epoch: 'epoch', session_id: 'session', revision: 6, state: 'transcribing', audio_saved: true } }, location.origin));
  await page.waitForTimeout(150);
  await expect(frame.locator('body')).toHaveAttribute('data-state', 'capturing');

  // Its controls do nothing, and pressing one is not an error.
  await frame.locator('section').hover();
  await frame.getByRole('button', { name: 'Stop recording' }).click();
  await expect(frame.locator('body')).toHaveAttribute('data-state', 'capturing');
  expect(errors).toEqual([]);
});

test('a native state check repairs a missed transition before acknowledging rendered frames', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-session', snapshot('success'));
  await expect(page.locator('section')).toBeHidden();
  await emit(page, 'writer-panel-sync', {
    nonce: 42, snapshot: snapshot('capturing', 2), preferences: panel(),
    theme: { kind: 'light', bg: '#ffffff', text: '#000000' }, waiting: 1,
  });
  await expect(page.locator('section')).toBeVisible();
  await expect(page.locator('#title')).toHaveText('Listening');
  // A counter on the pill's corner: one earlier dictation is still to come.
  await expect(page.locator('#waiting')).toContainText('1 earlier dictation still to be inserted');
  await expect(page.locator('#waiting > span[aria-hidden]')).toHaveText('1');
  await expect.poll(() => page.evaluate(() => (window as unknown as { overlayFixture: { calls: string[] } }).overlayFixture.calls)).toContain('alive');
  await expect(page.locator('html')).toHaveCSS('--bg', '#ffffff');
});

test('a responsive event handler cannot claim health when frames have stopped', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => { window.requestAnimationFrame = () => 0; });
  await emit(page, 'writer-panel-sync', { nonce: 1, snapshot: snapshot('capturing'), preferences: panel(), waiting: 0 });
  await expect(page.locator('#title')).toHaveText('Listening');
  await page.waitForTimeout(150);
  expect(await page.evaluate(() => (window as unknown as { overlayFixture: { calls: string[] } }).overlayFixture.calls)).not.toContain('alive');
  // Nor has "Listening" been painted while no frame is produced.
  expect(await shownReports(page)).toEqual([]);
});

test('reports when the start states reached the panel and a frame showed them, once each', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-session', snapshot('requesting'));
  await emit(page, 'writer-session', snapshot('capturing', 2));
  await emit(page, 'writer-session', snapshot('capturing', 3)); // Still the same "Listening".
  await expect.poll(async () => (await shownReports(page)).map(r => `${r.state}/${r.via}`)).toEqual(['requesting/event', 'capturing/event']);
  for (const report of await shownReports(page)) {
    expect(report.session_id).toBe('session');
    expect(report.received_ms).toBeGreaterThan(0);
    expect(report.painted_ms).toBeGreaterThanOrEqual(report.received_ms);
  }

  // A later dictation whose "Listening" arrives only with the shell's resync.
  await emit(page, 'writer-panel-sync', { nonce: 1, snapshot: { ...snapshot('capturing', 4), session_id: 'next' }, preferences: panel(), waiting: 0 });
  await expect.poll(async () => (await shownReports(page)).at(-1)).toMatchObject({ session_id: 'next', state: 'capturing', via: 'sync' });
  expect(await shownReports(page)).toHaveLength(3);
});

test('state checks do not keep postponing saved-audio dismissal', async ({ page }) => {
  await page.clock.install();
  await setup(page);
  const preferences = panel({ completion_hold_ms: 6000 });
  await emit(page, 'writer-preferences', preferences);
  await emit(page, 'writer-session', snapshot('saved'));
  for (const nonce of [1, 2]) {
    await page.clock.runFor(2000);
    await emit(page, 'writer-panel-sync', { nonce, snapshot: snapshot('saved'), preferences, waiting: 0 });
  }
  await page.clock.runFor(2100);
  expect(await page.evaluate(() => (window as unknown as { overlayFixture: { calls: string[] } }).overlayFixture.calls)).toContain('dismiss');
});

test('reload readiness waits for every subscription before requesting retained state', async ({ page }) => {
  await page.addInitScript(() => {
    const calls: string[] = [];
    let release: () => void;
    Object.assign(window, { overlayReadyFixture: { calls, release: () => release() }, __TAURI__: {
      event: { listen: async (name: string) => {
        if (name === 'writer-session') await new Promise<void>(resolve => { release = resolve; });
      } },
      core: { invoke: async (_command: string, options: { action: string }) => { calls.push(options.action); } },
    } });
  });
  await page.goto('/desktop-overlay.html');
  expect(await page.evaluate(() => (window as unknown as { overlayReadyFixture: { calls: string[] } }).overlayReadyFixture.calls)).not.toContain('ready');
  await page.evaluate(() => (window as unknown as { overlayReadyFixture: { release: () => void } }).overlayReadyFixture.release());
  await expect.poll(() => page.evaluate(() => (window as unknown as { overlayReadyFixture: { calls: string[] } }).overlayReadyFixture.calls)).toContain('ready');
});

test('each state looks different: opening, listening, no level, words, failure and its choices', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-preferences', panel());
  // Opening the microphone is a breathing mic, never the processing dots.
  await emit(page, 'writer-session', snapshot('requesting'));
  await expect(page.locator('#opening')).toBeVisible();
  await expect(page.locator('#loading')).toBeHidden();
  // Listening: eleven bars, the newest level in the middle.
  await emit(page, 'writer-session', snapshot('capturing', 2));
  await emit(page, 'writer-session', level(0.04, 1, 2));
  const heights = () => page.locator('#bars i').evaluateAll((nodes) => nodes.map((node) => parseFloat((node as HTMLElement).style.height)));
  const first = await heights();
  expect(first).toHaveLength(11);
  expect(first[5]).toBeGreaterThan(first[0]);
  await emit(page, 'writer-session', level(0.0, 2, 2));
  const rolled = await heights();
  // The earlier, louder level has moved one step outward from the middle.
  expect(rolled[4]).toBeGreaterThan(rolled[5]);
  expect(rolled[6]).toBeGreaterThan(rolled[5]);
  // No level for a while: a dashed line, not flat bars that look like silence.
  await expect(page.locator('#dash')).toBeVisible({ timeout: 3000 });
  await expect(page.locator('#bars')).toBeHidden();
  // Hover shows how long you have been talking, with Stop and Discard.
  await page.locator('section').hover();
  await expect(page.locator('#elapsed')).toHaveText(/^\d+:\d\d$/);
  await expect(page.getByRole('button', { name: 'Discard dictation' })).toBeVisible();

  // Failed: a word with a red dot; on hover, Retry (the audio was kept), Open and Dismiss.
  await page.mouse.move(0, 0);
  await emit(page, 'writer-session', { ...snapshot('error', 3), error_code: 'provider_unavailable' });
  await expect(page.locator('#word')).toHaveText('Failed');
  await expect(page.locator('.activity')).not.toContainText('!');
  await page.locator('section').hover();
  for (const name of ['Retry transcription', 'Open TrickTic Dictate', 'Dismiss panel']) {
    await expect(page.getByRole('button', { name })).toBeVisible();
  }
  await page.getByRole('button', { name: 'Retry transcription' }).click();
  expect(await page.evaluate(() => (window as unknown as { overlayFixture: { calls: string[] } }).overlayFixture.calls)).toContain('retry');

  // Saved without text keeps its word readable, with its two choices.
  await emit(page, 'writer-session', snapshot('saved', 4));
  await expect(page.locator('#word')).toHaveText('Saved');
  await expect(page.getByRole('button', { name: 'Retry transcription' })).toBeVisible();
});
