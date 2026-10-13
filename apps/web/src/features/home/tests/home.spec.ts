import { readFile } from 'node:fs/promises';
import { expect, test, type Page } from '@playwright/test';

/**
 * The Home surface over fabricated sessions. Everything it reaches for outside
 * itself is routed here, so a failure is about the interface rather than about
 * a backend that is not running.
 */
const MODES = {
  revision: 1,
  active: 'super',
  profiles: [
    { id: 'regular', revision: 1, name: 'Regular', kind: 'regular', instructions: '', text_model: null, context: { application: false, selected_text: false, clipboard: false } },
    { id: 'super', revision: 3, name: 'Super', kind: 'super', instructions: 'Polish', text_model: { provider: 'gemini', model: 'gemini-3.8-flash' }, context: { application: false, selected_text: false, clipboard: false } },
  ],
};

async function open(page: Page, query = '') {
  await page.route('**/api/v1/jobs/*/live-processing', route => route.fulfill({ json: { api_version: '1', data: { bound: false, attempt: null } } }));
  await page.route('**/api/v1/jobs/*/processed', (route) => route.fulfill({ json: { api_version: '1', data: { text: '' } } }));
  await page.route('**/api/v1/jobs/*/processing', (route) => route.fulfill({ json: { api_version: '1', data: [] } }));
  await page.route('**/api/v1/cleanup/providers', (route) => route.fulfill({ json: { api_version: '1', data: [] } }));
  await page.route('**/api/v1/modes', (route) => route.fulfill({ json: { api_version: '1', data: MODES } }));
  await page.route('**/api/v1/modes/providers', (route) => route.fulfill({ json: { api_version: '1', data: [{ selection: { provider: 'gemini', model: 'gemini-3.8-flash' }, configured: true, execution_location: 'cloud' }] } }));
  await page.goto(`/src/features/home/tests/home.html${query}`);
  if (query.includes('empty')) await expect(page.locator('.empty-hero')).toBeVisible();
  else await expect(page.locator('.session-row').first()).toBeVisible();
}

const rows = (page: Page) => page.locator('.session-row');
const reader = (page: Page) => page.getByRole('region', { name: 'Transcript', exact: true });
const dock = (page: Page) => page.getByRole('region', { name: 'Dictation', exact: true });

test('a row leads with what was said, and a Hebrew session mirrors it', async ({ page }) => {
  await open(page);

  const english = rows(page).first();
  const hebrew = rows(page).nth(1);
  await expect(english.locator('.row-text')).toContainText('The quick brown fox');
  await expect(hebrew.locator('.row-text')).toContainText('שלום');

  // The transcript is the loud object; the clock annotates it.
  const textSize = await english.locator('.row-text').evaluate((node) => parseFloat(getComputedStyle(node).fontSize));
  const clockSize = await english.locator('.row-when time').evaluate((node) => parseFloat(getComputedStyle(node).fontSize));
  expect(textSize).toBeGreaterThan(clockSize);

  // Each row reads in the direction of its own transcript, whatever the
  // interface language is: text leading, time trailing, mirrored for Hebrew.
  await expect(english).toHaveAttribute('dir', 'ltr');
  await expect(hebrew).toHaveAttribute('dir', 'rtl');
  const box = async (locator: ReturnType<Page['locator']>) => (await locator.boundingBox())!;
  const [enText, enWhen] = [await box(english.locator('.row-text')), await box(english.locator('.row-when'))];
  const [heText, heWhen] = [await box(hebrew.locator('.row-text')), await box(hebrew.locator('.row-when'))];
  expect(enText.x).toBeLessThan(enWhen.x);
  expect(heText.x).toBeGreaterThan(heWhen.x);
  // The meta line names the language.
  await expect(english.locator('.row-meta')).toContainText('EN');
});

test('a failed, interrupted or untranscribed row says what is kept and offers the one thing to do', async ({ page }) => {
  await open(page);
  const failedRow = rows(page).nth(2);
  await expect(failedRow.locator('.row-text')).toHaveText('Couldn’t transcribe — your audio is saved');
  await expect(failedRow.locator('.row-meta')).toContainText('Failed');
  await expect(failedRow.locator('.row-meta')).toContainText('Interrupted · partial audio kept');
  await failedRow.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect(page.getByTestId('transcribed')).toHaveText('c');

  const saved = rows(page).nth(3);
  await expect(saved.locator('.row-text')).toHaveText('Saved audio, not transcribed yet');
  await saved.getByRole('button', { name: 'Transcribe', exact: true }).click();
  await expect(page.getByTestId('transcribed')).toHaveText('c,d');
});

test('filters narrow history to what needs attention, or to Hebrew', async ({ page }) => {
  await open(page);
  const filters = page.getByRole('group', { name: 'Show' });
  await expect(filters.getByRole('button', { name: /Needs attention/ })).toContainText('2');
  await filters.getByRole('button', { name: /Needs attention/ }).click();
  await expect(rows(page)).toHaveCount(2);
  await expect(page).toHaveURL(/filter=attention/);
  await filters.getByRole('button', { name: 'Hebrew', exact: true }).click();
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first()).toHaveAttribute('dir', 'rtl');
  await filters.getByRole('button', { name: 'All', exact: true }).click();
  await expect(rows(page)).toHaveCount(4);
});

test('a transcript is copied with one press from the row it is on', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await open(page);

  const row = rows(page).first();
  // The control is revealed by the row, not parked on it permanently.
  await expect(row.getByRole('button', { name: /^Copy the/ })).toHaveCSS('opacity', '0');
  await row.hover();
  const copy = row.getByRole('button', { name: /^Copy the/ });
  await expect(copy).toHaveCSS('opacity', '1');

  await copy.click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain('The quick brown fox');
  await expect(page.locator('.session-column').getByRole('status')).toHaveText('Copied');
});

test('right-click opens Writer\'s own menu on a row and nothing at all on the background', async ({ page }) => {
  await open(page);

  await rows(page).first().click({ button: 'right' });
  const menu = page.getByRole('menu');
  await expect(menu).toBeVisible();
  await expect(menu.getByRole('menuitem')).toHaveText(['Copy text', 'Play audio', 'Transcribe again', 'Save audio…', 'Save text…']);

  // Escape returns without acting.
  await page.keyboard.press('Escape');
  await expect(menu).toHaveCount(0);

  // A surface with no actions of its own shows no menu — and still does not
  // hand the window over to the WebView's Reload/Save-as list.
  const prevented = await page.locator('.session-group').first().evaluate((node) => {
    let seen = false;
    document.addEventListener('contextmenu', (event) => { seen = event.defaultPrevented; }, { once: true });
    node.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 40, clientY: 120 }));
    return seen;
  });
  expect(prevented).toBe(true);
  await expect(page.getByRole('menu')).toHaveCount(0);
});

test('the row menu retries a transcription and plays the saved audio', async ({ page }) => {
  await open(page);
  await page.evaluate(() => {
    (window as unknown as { played: number }).played = 0;
    HTMLMediaElement.prototype.play = function play() {
      (window as unknown as { played: number }).played += 1;
      return Promise.resolve();
    };
  });

  await rows(page).first().click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Transcribe again' }).click();
  await expect(page.getByTestId('transcribed')).toHaveText('a');

  // Playing a session that is not open selects it first, then starts once its
  // saved audio is actually readable.
  await rows(page).nth(1).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Play audio' }).click();
  await expect(page.locator('.session-row.current .row-text')).toContainText('שלום');
  await expect.poll(() => page.evaluate(() => (window as unknown as { played: number }).played)).toBe(1);
});

test('the dock carries the record action, the shortcut and the two switches', async ({ page }) => {
  // This fixture checks Windows labels regardless of the test runner's host.
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'Win32', configurable: true }));
  await open(page);

  const bar = dock(page);
  await bar.getByRole('button', { name: 'Record', exact: true }).click();
  expect(await page.evaluate(() => window.started)).toBe(1);
  await expect(bar.locator('.dock-hint .kbd')).toHaveText('CtrlShiftD');
  // Nothing is timing anything yet, so the dock does not show a stopped clock.
  await expect(bar.getByLabel('Recording duration')).toHaveCount(0);

  // The mode switch says what the next dictation is processed with.
  const mode = bar.getByRole('button', { name: /Super/ });
  await mode.click();
  const modes = page.getByRole('menu', { name: 'Recording mode' });
  await expect(modes.getByRole('menuitemradio', { name: /Super/ })).toHaveAttribute('aria-checked', 'true');
  await expect(modes.getByRole('menuitemradio', { name: /Regular/ })).toBeEnabled();
  await page.keyboard.press('Escape');
  await expect(modes).toHaveCount(0);
  await expect(mode).toBeFocused();

  // The engine switch holds the language and the model, and the way into their settings.
  await bar.getByRole('button', { name: /EN/ }).click();
  const engine = page.getByRole('menu', { name: 'Language and speech model' });
  await expect(engine.getByRole('menuitemradio', { name: 'English', exact: true })).toHaveAttribute('aria-checked', 'true');
  await engine.getByRole('menuitem', { name: 'Speech engine settings…' }).click();
  expect(await page.evaluate(() => window.openModels)).toBe(1);

  // While recording the switches are not a decision you can act on, so their
  // place is taken by the elapsed time, the meter and the way out.
  await open(page, '?state=recording');
  const live = dock(page);
  await expect(live.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  await expect(live.getByLabel('Recording duration')).toHaveText('00:14');
  await expect(live.getByRole('button', { name: /Super/ })).toHaveCount(0);
  await expect(live.getByRole('button', { name: 'Discard' })).toBeVisible();
});

test('a dictation runs through the stage and the dock, then the finished one takes the reader', async ({ page }) => {
  await open(page, '?frame');
  const stable = () => page.evaluate(() => ['.dock', '.session-column'].map((selector) => {
    const { x, y, width, height } = document.querySelector(selector)!.getBoundingClientRect();
    return { x, y, width, height };
  }));
  const initial = await stable();

  await page.evaluate(() => window.session({ state: 'requesting', id: 'b' }));
  await expect(dock(page)).toContainText('Opening the microphone');
  await page.evaluate(() => window.session({ state: 'capturing', text: '' }));
  const stage = page.getByRole('region', { name: 'Dictation in progress' });
  await expect(stage).toContainText('Listening');
  await expect(stage).toContainText('Release');

  // The newest words stay in view however long the dictation runs, and
  // nothing else on Home moves.
  for (const text of ['First words', Array.from({ length: 40 }, (_, i) => `Line ${i}: the draft keeps growing as I speak.`).join('\n'), 'שלום עולם '.repeat(200)]) {
    await page.evaluate((text) => window.session({ state: 'capturing', text }), text);
    await expect.poll(() => page.locator('.live-viewport').evaluate((node) => node.scrollHeight - node.clientHeight - node.scrollTop)).toBeLessThanOrEqual(1);
    expect(await stable()).toEqual(initial);
  }

  await page.evaluate(() => window.session({ state: 'transcribing' }));
  await expect(stage.getByRole('list', { name: 'Progress' })).toContainText('Transcribing');
  await expect(dock(page).locator('.step[aria-current="step"]')).toHaveText('Transcribe');
  await page.evaluate(() => window.session({ state: 'transcribing', progress: 'processing_text' }));
  await expect(dock(page).locator('.step[aria-current="step"]')).toHaveText('Polish');
  expect(await stable()).toEqual(initial);

  await page.evaluate(() => window.session({ state: 'success', delivery: { copied: true, pasted: true, message: 'Pasted', app: 'Slack' } }));
  // Where the words went, by the application's own name.
  await expect(dock(page).locator('.dock-outcome')).toContainText('Pasted into Slack');
  await expect(page.locator('.session-row.current .row-text')).toContainText('שלום');
  await expect(reader(page)).toBeVisible();
  expect(await stable()).toEqual(initial);
  // The outcome is let go after a few seconds.
  await expect(dock(page).locator('.dock-outcome')).toHaveCount(0, { timeout: 7000 });
});

test('Discard throws the dictation away with five seconds to undo it', async ({ page }) => {
  await open(page, '?state=recording');
  await dock(page).getByRole('button', { name: 'Discard' }).click();
  const toast = page.locator('.toast');
  await expect(toast).toContainText('Dictation discarded');
  expect(await page.evaluate(() => window.discarded)).toBe(1);
  await toast.getByRole('button', { name: 'Undo' }).click();
  expect(await page.evaluate(() => window.kept)).toEqual(['live']);
  await expect(toast).toContainText('Kept');
});

test('the reader leads with Copy, names the versions plainly and says how the text was made', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await open(page);
  const pane = reader(page);
  await expect(pane.getByRole('heading', { level: 2 })).toContainText(':');
  await expect(pane.getByRole('button', { name: 'Copy', exact: true })).toHaveClass(/primary/);
  await expect(pane.locator('.made')).toContainText('Heard by');
  await expect(pane.locator('.made')).toContainText('Audio kept');

  // The page's own copy control and the text's menu.
  await pane.getByRole('button', { name: 'Copy the as spoken transcript' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain('The quick brown fox');

  // A selection inside the transcript is offered on its own, because copying
  // the sentence you highlighted is not the same request as copying all of it.
  const at = await page.evaluate(() => {
    const text = document.querySelector('.reader-column .transcript-text')!.firstChild!;
    const range = document.createRange();
    range.setStart(text, 4);
    range.setEnd(text, 9);
    const selection = getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
    const box = range.getBoundingClientRect();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  });
  // On the selection itself: right-clicking anywhere else collapses it.
  await page.mouse.click(at.x, at.y, { button: 'right' });
  await page.getByRole('menuitem', { name: 'Copy selection' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('quick');

  // The rest lives behind ⋯.
  await pane.getByRole('button', { name: 'More actions' }).click();
  await expect(page.getByRole('menu').getByRole('menuitem')).toHaveText(['Save text…', 'Save audio…', 'Clean up', 'Polish with Super']);
});

test('keyboard: arrows walk history, Ctrl F searches, Ctrl C copies the open dictation', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'Win32', configurable: true }));
  await open(page);
  await rows(page).first().locator('.row-hit').focus();
  await page.keyboard.press('ArrowDown');
  await expect(rows(page).nth(1)).toHaveClass(/current/);
  await expect(rows(page).nth(1).locator('.row-hit')).toBeFocused();
  await page.keyboard.press('End');
  await expect(rows(page).nth(3)).toHaveClass(/current/);
  await page.keyboard.press('Home');
  await expect(rows(page).first()).toHaveClass(/current/);

  await page.keyboard.press('Control+c');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain('The quick brown fox');

  await page.keyboard.press('Control+f');
  await expect(page.getByRole('searchbox', { name: 'Search dictations' })).toBeFocused();
});

const superResult = (text: string, state: 'succeeded' | 'pending' | 'failed' = 'succeeded') => ({
  bound: true,
  attempt: {
    id: 'job-a', job_id: 'job-a', created_at_ms: 1,
    profile: { id: 'super', name: 'Super', kind: 'super', revision: 3, instructions: 'Polish', context: {}, text_model: { provider: 'gemini', model: 'gemini-3.8-flash' } },
    raw_text: 'The quick brown fox', input_text: 'The quick brown fox', prompt_version: 4,
    request: { operation_id: 'job-a', profile_id: 'super', profile_revision: 3, context: { application: '', selected_text: '', clipboard: '' } },
    diagnostics: { processing_ms: 852, reused_draft: false },
    state: state === 'succeeded' ? { state, text } : state === 'pending' ? { state } : { state, error: { message: 'Model unavailable', retryable: true } },
  },
});

async function saveTextFromMenu(page: Page) {
  const download = page.waitForEvent('download');
  await reader(page).getByRole('button', { name: 'More actions' }).click();
  await page.getByRole('menuitem', { name: 'Save text…' }).click();
  return download;
}

test('the polished result is the default reader, row, copy, export and searchable text; what was said stays selectable', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await open(page);
  const formatted = 'Grocery list:\n- Milk\n- Coffee';
  await page.route('**/api/v1/jobs/job-a/live-processing', route => route.fulfill({ json: { api_version: '1', data: superResult(formatted) } }));
  await page.reload();
  const pane = reader(page);
  await expect(pane.getByRole('group', { name: 'Polished transcript', exact: true })).toContainText(formatted);
  await expect(rows(page).first().locator('.row-text')).toHaveText(formatted);
  await expect(rows(page).first().locator('.row-meta')).toContainText('Polished');
  await rows(page).first().hover();
  await rows(page).first().getByRole('button', { name: /^Copy the/ }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(formatted);
  await pane.getByRole('button', { name: 'Copy', exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(formatted);
  const file = await saveTextFromMenu(page);
  expect(file.suggestedFilename()).toContain('-result.txt');
  expect(await readFile((await file.path())!, 'utf8')).toBe(formatted);
  await expect(pane.locator('.made')).toContainText('Polished by Gemini 3.8 Flash in 0.85 s');

  // Show both directions of the change; copying still uses the actual result.
  await pane.getByRole('button', { name: 'Show changes' }).click();
  await expect(pane.locator('.transcript-text mark').first()).toBeVisible();
  await expect(pane.locator('.transcript-text del').first()).toBeVisible();
  await expect(pane.locator('.transcript-text del').first()).toContainText('quick');
  await pane.getByRole('button', { name: 'Copy', exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(formatted);

  await pane.getByRole('button', { name: 'As spoken', exact: true }).click();
  await expect(pane.getByRole('group', { name: 'As spoken transcript', exact: true })).toContainText('The quick brown fox');
  await pane.getByRole('button', { name: 'Copy', exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain('The quick brown fox');
  await page.getByRole('searchbox', { name: 'Search dictations' }).fill('Coffee');
  await expect(rows(page)).toHaveCount(1);
});

test('what was said can be copied and exported while polishing, then stays available after', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await open(page);
  let complete = false;
  await page.route('**/api/v1/jobs/job-a/live-processing', route => route.fulfill({ json: { api_version: '1', data: superResult('A polished result.', complete ? 'succeeded' : 'pending') } }));
  await page.reload();
  const pane = reader(page);
  await expect(pane.getByText('Polishing…', { exact: false }).first()).toBeVisible();
  await pane.getByRole('button', { name: 'Copy', exact: true }).click();
  const raw = await page.evaluate(() => navigator.clipboard.readText());
  expect(raw).toContain('The quick brown fox');
  await rows(page).first().hover();
  await rows(page).first().getByRole('button', { name: /^Copy the/ }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(raw);
  await rows(page).first().click({ button: 'right' });
  await expect(page.getByRole('menuitem', { name: 'Copy text', exact: true })).toBeEnabled();
  const download = page.waitForEvent('download');
  await page.getByRole('menuitem', { name: 'Save text…', exact: true }).click();
  expect(await readFile((await (await download).path())!, 'utf8')).toBe(raw);
  complete = true;
  await expect(pane.getByRole('group', { name: 'Polished transcript', exact: true })).toContainText('A polished result.');
  await pane.getByRole('button', { name: 'Copy', exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('A polished result.');
  await pane.getByRole('button', { name: 'As spoken', exact: true }).click();
  await pane.getByRole('button', { name: 'Copy', exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(raw);
});

test('failed and unchanged polishing are described truthfully', async ({ page }) => {
  await open(page);
  await page.route('**/api/v1/jobs/job-a/live-processing', route => route.fulfill({ json: { api_version: '1', data: superResult('', 'failed') } }));
  await page.reload();
  const pane = reader(page);
  await expect(pane.getByRole('alert')).toContainText('Super failed: Model unavailable');
  await expect(pane.getByRole('group', { name: 'As spoken transcript', exact: true })).toContainText('The quick brown fox');
  const raw = await pane.locator('.transcript-block .transcript-text').innerText();
  await page.route('**/api/v1/jobs/job-a/live-processing', route => route.fulfill({ json: { api_version: '1', data: superResult(raw) } }));
  await page.reload();
  await expect(pane.getByText('The model returned the original wording.', { exact: false })).toBeVisible();
  await expect(pane.getByRole('button', { name: 'Polished', exact: true })).toHaveAttribute('aria-pressed', 'true');
});

test('an unavailable result keeps what was said copyable and can be retried', async ({ page }) => {
  await open(page);
  let fail = true;
  await page.route('**/api/v1/jobs/job-a/live-processing', route => fail
    ? route.fulfill({ status: 503, json: { api_version: '1', error: { message: 'Unavailable' } } })
    : route.fulfill({ json: { api_version: '1', data: superResult('Restored polished output.') } }));
  await page.reload();
  const pane = reader(page);
  await expect(pane.getByText('Processed result could not be loaded')).toBeVisible();
  await expect(pane.getByRole('button', { name: 'Copy', exact: true })).toBeEnabled();
  await pane.getByRole('button', { name: 'Use original transcript' }).click();
  await expect(pane.getByRole('button', { name: 'Copy', exact: true })).toBeEnabled();
  fail = false;
  await expect(pane.getByRole('button', { name: 'Polished', exact: true })).toBeVisible();
  // An explicit choice of what was said survives asynchronous completion.
  await expect(pane.getByRole('group', { name: 'As spoken transcript', exact: true })).toBeVisible();
  await pane.getByRole('button', { name: 'Polished', exact: true }).click();
  await expect(pane.getByRole('group', { name: 'Polished transcript', exact: true })).toContainText('Restored polished output.');
});

test('a failed retry never presents the older mode result as the current job result', async ({ page }) => {
  await open(page, '?failedRetry=1');
  await page.route('**/api/v1/jobs/job-a/live-processing', route => route.fulfill({ json: { api_version: '1', data: superResult('Earlier polished output.') } }));
  await page.reload();
  await expect(rows(page).first().locator('.row-text')).toHaveText('Earlier polished output.');
  await expect(rows(page).first().locator('.row-meta')).toContainText('Last attempt failed');
  const pane = reader(page);
  await expect(pane.getByRole('alert')).toContainText('This dictation couldn’t be transcribed');
  await expect(pane.getByRole('group', { name: 'Polished transcript', exact: true })).toHaveCount(0);
  await expect(pane.getByRole('button', { name: 'Transcribe again' })).toHaveClass(/primary/);
  // The earlier attempt's text is shown, not hidden behind Details.
  await expect(pane.locator('.attempt-list')).toContainText('The quick brown fox');
});

test('history and reader keep readable insets, and narrow windows get a way back', async ({ page }) => {
  await open(page);
  for (const width of [900, 1024, 1440, 1920]) {
    await page.setViewportSize({ width, height: 800 });
    await expect(rows(page).first()).toBeVisible();
    const geometry = await page.evaluate(() => {
      const row = document.querySelector('.session-row')!.getBoundingClientRect();
      const text = document.querySelector('.session-row .row-text')!.getBoundingClientRect();
      const column = document.querySelector('.session-column')!.getBoundingClientRect();
      const paper = document.querySelector('.paper')!.getBoundingClientRect();
      const stage = document.querySelector('.reader-stage')!.getBoundingClientRect();
      return { rowInset: row.left - column.left, textInset: text.left - row.left, paperLeft: paper.left - stage.left, paperRight: stage.right - paper.right, paperWidth: paper.width };
    });
    expect(geometry.rowInset).toBeGreaterThanOrEqual(8);
    expect(geometry.textInset).toBeGreaterThanOrEqual(16);
    expect(geometry.paperLeft).toBeGreaterThanOrEqual(24);
    expect(geometry.paperRight).toBeGreaterThanOrEqual(24);
    // A very wide window does not stretch the page into one long line.
    expect(geometry.paperWidth).toBeLessThanOrEqual(960);
  }
  await page.setViewportSize({ width: 600, height: 700 });
  await expect(page.getByRole('region', { name: 'Sessions', exact: true })).toBeVisible();
  await rows(page).nth(1).locator('.row-hit').click();
  await expect(reader(page)).toBeVisible();
  await page.getByRole('button', { name: 'Back to history' }).click();
  await expect(rows(page).nth(1)).toBeVisible();
  await expect(rows(page).nth(1)).toHaveClass(/current/);
});

test('the first run teaches the shortcut, and the first dictation keeps Home stable', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'Win32', configurable: true }));
  await open(page, '?frame&empty');
  const hero = page.locator('.empty-hero');
  await expect(hero.getByRole('heading', { name: 'Speak anywhere. It’s typed.' })).toBeVisible();
  await expect(hero.locator('.kbd')).toHaveText('CtrlShiftD');
  await hero.getByRole('button', { name: 'Try a dictation here' }).click();
  expect(await page.evaluate(() => window.started)).toBe(1);

  // The first dictation gets the stage; history says where it will appear.
  await page.evaluate(() => window.session({ state: 'capturing', id: 'a', text: 'My first dictation.' }));
  await expect(page.getByRole('region', { name: 'Dictation in progress' })).toContainText('My first dictation.');
  await expect(page.locator('.session-column')).toContainText('Your dictations will appear here');
  await page.evaluate(() => window.session({ state: 'success', empty: false }));
  await expect(rows(page).first()).toBeVisible();
  await expect(page.locator('.dictation-dashboard')).toHaveCount(0);
});

test('original recovery is selected for copy/export and polishing again uses current settings', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await open(page);
  const original = 'The quick brown fox · exact snippet';
  const auto = superResult(original);
  auto.attempt.diagnostics = { ...auto.attempt.diagnostics, original_used: true } as typeof auto.attempt.diagnostics;
  let selected: object | null = null;
  let posts = 0;
  const current = { ...auto.attempt.profile, revision: 8, context: { application: false, selected_text: false, clipboard: false } };
  await page.route('**/api/v1/modes', route => route.fulfill({ json: { api_version: '1', data: { revision: 9, active: 'super', profiles: [current] } } }));
  await page.route('**/api/v1/jobs/job-a/live-processing', route => route.fulfill({ json: { api_version: '1', data: { ...auto, selected_attempt: selected, delivery: { attempt_id: 'job-a', original_used: true, copied: true, pasted: true, at_ms: 1 } } } }));
  await page.route('**/api/v1/jobs/job-a/processing', route => {
    if (route.request().method() === 'POST') {
      const request = route.request().postDataJSON().data; expect(request.profile_revision).toBe(8); posts++;
      selected = { ...auto.attempt, id: request.operation_id, profile: current, request, state: { state: 'succeeded', text: 'A later polished result.' }, diagnostics: { processing_ms: 200, reused_draft: false } };
      return route.fulfill({ json: { api_version: '1', data: selected } });
    }
    return route.fulfill({ json: { api_version: '1', data: selected ? [auto.attempt, selected] : [auto.attempt] } });
  });
  await page.reload();
  const pane = reader(page);
  await expect(pane).toContainText('Polishing unavailable');
  await expect(pane.getByRole('group', { name: 'Original transcript', exact: true })).toContainText(original);
  await pane.getByRole('button', { name: 'Copy', exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(original);
  const downloaded = await saveTextFromMenu(page);
  expect(await readFile((await downloaded.path())!, 'utf8')).toBe(original);
  await pane.getByRole('button', { name: 'Polish again', exact: true }).click();
  await expect(pane.getByRole('group', { name: 'Polished transcript', exact: true })).toContainText('A later polished result.');
  await expect(pane).toContainText('The original was inserted. This later result is available here.');
  expect(posts).toBe(1);
});
