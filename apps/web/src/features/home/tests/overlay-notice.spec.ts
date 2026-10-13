import { expect, test, type Page } from '@playwright/test';

/*
 * Notices on the floating panel: a shortcut press Writer could not act on, a
 * paste it skipped, and the hint that insertion is waiting for the shortcut
 * keys to be released. The shell decides whether the window is on screen;
 * these check what the page inside it shows.
 */
async function setup(page: Page) {
  await page.addInitScript(() => {
    const listeners: Record<string, (event: { payload: unknown }) => void> = {};
    Object.assign(window, { overlayFixture: { listeners }, __TAURI__: {
      event: { listen: async (name: string, listener: (event: { payload: unknown }) => void) => { listeners[name] = listener; } },
      core: { invoke: async () => {} },
    } });
  });
  await page.setViewportSize({ width: 112, height: 44 });
  await page.goto('/desktop-overlay.html');
  await emit(page, 'writer-preferences', { auto_dismiss: true, completion_hold_ms: 1200, reduced_motion: false, visibility: 'recording', scale: 1 });
}
async function emit(page: Page, name: string, payload: unknown) {
  await page.evaluate(({ name, payload }) => {
    (window as unknown as { overlayFixture: { listeners: Record<string, (event: { payload: unknown }) => void> } }).overlayFixture.listeners[name]({ payload });
  }, { name, payload });
}
const snapshot = (state: string, revision = 1, session = 'session') =>
  ({ kind: 'snapshot', epoch: 'epoch', session_id: session, revision, state, audio_saved: true });

test('a notice shows on a panel that is otherwise hidden, then clears itself', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-session', snapshot('success'));
  await expect(page.locator('section')).toBeHidden();
  const detail = 'TrickTic Dictate could not start a new dictation yet. Press the shortcut again in a moment.';
  await emit(page, 'writer-notice', { session_id: null, text: 'Busy', detail, ms: 2000 });
  await expect(page.locator('section')).toBeVisible();
  await expect(page.locator('#notice')).toHaveText('Busy');
  await expect(page.locator('#detail')).toHaveText(detail);
  await expect(page.locator('section')).toBeHidden({ timeout: 6000 });
});

test('the release-keys hint belongs to its own dictation and ends with its next change', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-session', snapshot('transcribing'));
  // An earlier dictation finishing in the background does not speak over this one.
  await emit(page, 'writer-notice', { session_id: 'earlier', text: 'Release keys', detail: 'Release the shortcut keys to insert the text.', ms: 0 });
  await expect(page.locator('#notice')).toBeHidden();
  await expect(page.locator('#loading')).toBeVisible();

  await emit(page, 'writer-notice', { session_id: 'session', text: 'Release keys', detail: 'Release the shortcut keys to insert the text.', ms: 0 });
  await expect(page.locator('#notice')).toHaveText('Release keys');
  await expect(page.locator('#loading')).toBeHidden();
  await emit(page, 'writer-session', snapshot('success', 2));
  await expect(page.locator('#notice')).toBeHidden();
  await expect(page.locator('section')).toBeHidden();
});

test('a skipped paste stays readable after its dictation finishes, until a new one starts', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-session', snapshot('transcribing'));
  const detail = 'Copied to clipboard. The application you dictated into has closed, so automatic paste was skipped.';
  await emit(page, 'writer-notice', { session_id: 'session', text: 'Copied', detail, ms: 5000 });
  await emit(page, 'writer-session', snapshot('success', 2));
  await expect(page.locator('section')).toBeVisible();
  await expect(page.locator('#notice')).toHaveText('Copied');
  await expect(page.locator('#detail')).toHaveText(detail);

  await emit(page, 'writer-session', snapshot('requesting', 3, 'next'));
  await expect(page.locator('#notice')).toBeHidden();
  // Opening the microphone has its own look: a breathing mic, not the processing dots.
  await expect(page.locator('#opening')).toBeVisible();
});

test('an earlier text still to come is counted on the pill until it lands', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-session', snapshot('capturing', 1, 'next'));
  await expect(page.locator('#waiting')).toBeHidden();
  await emit(page, 'writer-waiting', { count: 1 });
  await expect(page.locator('#waiting')).toBeVisible();
  // A counter on the pill's corner.
  await expect(page.locator('#waiting > span[aria-hidden]')).toHaveText('1');
  await expect(page.getByRole('status').filter({ hasText: '1 earlier dictation still to be inserted' })).toHaveCount(1);
  // It makes way for the controls while the pointer is on the panel.
  await page.locator('section').hover();
  await expect(page.locator('#waiting')).toBeHidden();
  await page.mouse.move(1, 1);
  await expect(page.locator('#waiting')).toBeVisible();
  await emit(page, 'writer-waiting', { count: 0 });
  await expect(page.locator('#waiting')).toBeHidden();
});

test('a pasted dictation says so with a tick, then goes even after it succeeds', async ({ page }) => {
  await setup(page);
  await emit(page, 'writer-session', snapshot('transcribing'));
  await emit(page, 'writer-notice', { session_id: 'session', text: 'Pasted', detail: 'Your text is in.', ms: 350, tone: 'done' });
  await expect(page.locator('body')).toHaveAttribute('data-notice-tone', 'done');
  await expect(page.locator('#notice svg')).toBeVisible();
  await expect(page.locator('#notice')).toHaveText('Pasted');
  // Success arrives while the tick is up: it stays for its moment.
  await emit(page, 'writer-session', snapshot('success', 2));
  await expect(page.locator('section')).toBeVisible();
  await expect(page.locator('section')).toBeHidden({ timeout: 2000 });
  expect(await page.locator('body').getAttribute('data-notice-tone')).toBeNull();
  // A plain notice afterwards carries no tick.
  await emit(page, 'writer-notice', { session_id: null, text: 'Busy', detail: 'Press it again.', ms: 500 });
  await expect(page.locator('#notice svg')).toHaveCount(0);
});
