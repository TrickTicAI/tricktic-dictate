import { stubOwnerSession, loginFixture } from '../../auth/tests/fixture';
import { expect, test, type Page } from '@playwright/test';

/*
 * The application frame. The no-scroll rule is a layout invariant, not a
 * cosmetic one: it holds only while every flex ancestor between the viewport
 * and a scroller can shrink, and the default `min-height: auto` on flex items
 * is the thing most likely to break it silently. So it is asserted, not
 * reviewed.
 */

const VIEWPORTS = [
  { width: 760, height: 600 },
  { width: 1024, height: 700 },
  { width: 1280, height: 800 },
  { width: 1920, height: 1080 },
];

const SECTIONS = ['engine', 'shortcuts', 'microphone', 'panel', 'modes', 'text', 'keys', 'general', 'appearance', 'account', 'data', 'about'];

async function stubApi(page: Page) {
  await stubOwnerSession(page);
  await page.route('**/api/v1/health', (route) => route.fulfill({ json: { status: 'ok' } }));
  await page.route('**/api/v1/providers', (route) => route.fulfill({
    json: {
      providers: [
        { id: 'openai', name: 'OpenAI', configured: true, available: true, status: 'ready', models: ['gpt-transcribe'] },
        { id: 'local', name: 'Local', configured: false, available: false, status: 'not configured', models: [] },
      ],
    },
  }));
  await page.route('**/api/v1/vocabulary', (route) => route.fulfill({
    json: {
      api_version: '1',
      data: {
        hints: ['TrickTic Dictate', 'ivrit-turbo'],
        corrections: [{ from: 'trick tick dictate', to: 'TrickTic Dictate' }],
        snippets: [{ phrase: 'המייל שלי', text: 'writer@example.com' }],
      },
    },
  }));
  await page.route('**/api/v1/cleanup/providers', (route) => route.fulfill({ json: { api_version: '1', data: [] } }));
  await page.route('**/api/v1/setup', (route) => route.fulfill({ status: 503, json: {} }));
}

/** The whole document must fit the window, with no page-level scrollbar. */
async function assertNoPageScroll(page: Page, where: string) {
  const overflow = await page.evaluate(() => ({
    root: document.documentElement.scrollHeight - document.documentElement.clientHeight,
    rootX: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    body: document.body.scrollHeight - document.body.clientHeight,
  }));
  expect(overflow.root, `vertical page scroll at ${where}`).toBe(0);
  expect(overflow.rootX, `horizontal page scroll at ${where}`).toBe(0);
  expect(overflow.body, `body scroll at ${where}`).toBe(0);
}

test('no page-level scrolling on any page or with settings open, at four window sizes', async ({ page }) => {
  await stubApi(page);
  for (const viewport of VIEWPORTS) {
    await page.setViewportSize(viewport);
    const size = `${viewport.width}x${viewport.height}`;

    for (const tab of ['home', 'statistics', 'dictionary']) {
      await page.goto(`/?tab=${tab}`); await loginFixture(page);
      await expect(page.getByRole('navigation', { name: 'Sections' })).toBeVisible();
      await assertNoPageScroll(page, `${tab} at ${size}`);
    }

    // The modal is the case most likely to regress: it is the tallest thing in
    // the tree and sits above a layout that already fills the window exactly.
    for (const section of SECTIONS) {
      await page.goto(`/?tab=home&settings=${section}`); await loginFixture(page);
      await expect(page.getByRole('dialog', { name: 'Settings' })).toBeVisible();
      await assertNoPageScroll(page, `settings/${section} at ${size}`);
    }
  }
});

test('tab, sub-tab and the open settings section are URL-backed and survive a reload', async ({ page }) => {
  await stubApi(page);
  await page.setViewportSize(VIEWPORTS[1]);
  await page.goto('/'); await loginFixture(page);

  await page.getByRole('button', { name: 'Dictionary' }).click();
  await expect(page).toHaveURL(/tab=dictionary/);
  await page.getByRole('tab', { name: /Corrections/ }).click();
  await expect(page).toHaveURL(/sub=corrections/);

  await page.reload(); await loginFixture(page);
  await expect(page.getByRole('tab', { name: /Corrections/ })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('heading', { name: 'Dictionary', level: 1 })).toBeVisible();

  // Exact: the status chip is also a button, and it now says what pressing it
  // does — which is to open the model settings.
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page).toHaveURL(/settings=general/);
  await page.getByRole('button', { name: 'Appearance' }).click();
  await expect(page).toHaveURL(/settings=appearance/);

  // Going back to the default section keeps the dialog open. The section used
  // to be dropped from the URL when it equalled the default, and an absent
  // section is exactly how the workspace knows Settings is closed — so
  // choosing General from the sidebar dismissed the whole dialog.
  await page.getByRole('button', { name: 'General' }).click();
  await expect(page).toHaveURL(/settings=general/);
  await expect(page.getByRole('dialog', { name: 'Settings' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'General', level: 2 })).toBeVisible();

  await page.getByRole('button', { name: 'Appearance' }).click();
  await page.reload(); await loginFixture(page);
  await expect(page.getByRole('dialog', { name: 'Settings' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Appearance', level: 2 })).toBeVisible();

  // Closing returns to the page underneath rather than navigating away.
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: 'Settings' })).toBeHidden();
  await expect(page).toHaveURL(/tab=dictionary/);
  await expect(page).not.toHaveURL(/settings=/);
});

test('the cleanup pass lists every text model distinctly without squeezing its description', async ({ page }) => {
  await stubApi(page);
  const choice = (provider: string, model: string, configured = true) => ({ selection: { provider, model }, configured, execution_location: 'cloud' });
  await page.route('**/api/v1/cleanup/providers', (route) => route.fulfill({
    json: {
      api_version: '1',
      data: [
        choice('openai', 'gpt-4.1-mini', false), choice('openai', 'gpt-5.6-luna', false), choice('gemini', 'gemini-3.8-flash'),
        choice('grok', 'grok-4.6', false), choice('grok', 'grok-4.3', false), choice('openrouter', 'anthropic/claude-sonnet-5'),
        choice('openrouter', 'google/gemini-3.8-pro'),
      ],
    },
  }));
  await page.setViewportSize(VIEWPORTS[2]);
  await page.goto('/?tab=home&settings=general'); await loginFixture(page);
  const picker = page.getByRole('combobox', { name: 'Cleanup model' });
  await expect(picker.locator('option')).toHaveText([
    'Off', 'gpt-4.1-mini · OpenAI · key needed', 'gpt-5.6-luna · OpenAI · key needed', 'gemini-3.8-flash · Gemini',
    'grok-4.6 · Grok · key needed', 'grok-4.3 · Grok · key needed', 'anthropic/claude-sonnet-5 · OpenRouter', 'google/gemini-3.8-pro · OpenRouter',
  ]);
  await picker.selectOption('openrouter/google/gemini-3.8-pro');
  await expect(picker).toHaveValue('openrouter/google/gemini-3.8-pro');
  // A control too wide for its row once crushed the description to one word per line.
  const detail = page.getByText(/Adjusts punctuation, capitalization/);
  expect((await detail.boundingBox())!.width).toBeGreaterThan(250);
  await assertNoPageScroll(page, 'settings/general with many cleanup models');
});

test('links to sections from before the regrouping open their new homes', async ({ page }) => {
  await stubApi(page);
  await page.setViewportSize(VIEWPORTS[1]);
  // The tray of an older shell, and any link written before the sections were regrouped.
  for (const [old, title, id] of [['models', 'Speech engine', 'engine'], ['transcription', 'Speech engine', 'engine'], ['system', 'Floating panel', 'panel']]) {
    await page.goto(`/?settings=${old}`); await loginFixture(page);
    await expect(page.getByRole('heading', { name: title, level: 2 })).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`settings=${id}`));
  }
});

test('paper is the default theme and the picker repaints the whole frame', async ({ page }) => {
  await stubApi(page);
  await page.setViewportSize(VIEWPORTS[1]);
  await page.goto('/?settings=appearance'); await loginFixture(page);

  const token = () => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--bg').trim());
  const kind = () => page.evaluate(() => document.documentElement.dataset.themeKind);
  expect(await page.evaluate(() => document.documentElement.dataset.theme)).toBe('paper');
  expect((await token()).toUpperCase()).toBe('#EBE5DC');
  expect(await kind()).toBe('light');

  // The title bar is ordinary page content, so it repaints with everything else.
  const titlebar = page.locator('.titlebar');
  const before = await titlebar.evaluate((element) => getComputedStyle(element).backgroundColor);

  await page.getByRole('button', { name: 'Dracula', exact: false }).first().click();
  expect(await page.evaluate(() => document.documentElement.dataset.theme)).toBe('dracula');
  expect(await kind()).toBe('dark');
  expect(await titlebar.evaluate((element) => getComputedStyle(element).backgroundColor)).not.toBe(before);

  await page.reload(); await loginFixture(page);
  expect(await page.evaluate(() => document.documentElement.dataset.theme)).toBe('dracula');
});

test('the settings dialog keeps the keyboard, and Home keeps statistics on their own page', async ({ page }) => {
  await stubApi(page);
  await page.setViewportSize(VIEWPORTS[1]);
  await page.goto('/?tab=home'); await loginFixture(page);

  await expect(page.locator('.statistics-totals')).toHaveCount(0);
  await page.getByRole('button', {name: 'Statistics', exact: true}).click();
  await expect(page.getByRole('heading', {name:'Statistics', exact:true})).toBeVisible();
  await page.reload(); await loginFixture(page);
  await expect(page.getByRole('heading', {name:'Statistics', exact:true})).toBeVisible();

  await page.goto('/?settings=general'); await loginFixture(page);
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(dialog).toBeVisible();

  // Tab has to stay inside the dialog. Escape and the backdrop already closed
  // it; the keyboard used to walk straight out into the page behind, where
  // every control is reachable and none of it is what the dialog asks about.
  const inside = () => page.evaluate(() => !!document.querySelector('.modal')?.contains(document.activeElement));
  for (let step = 0; step < 40; step++) {
    await page.keyboard.press('Tab');
    expect(await inside(), `focus left the dialog after ${step + 1} tabs`).toBe(true);
  }
  await page.keyboard.press('Shift+Tab');
  expect(await inside()).toBe(true);
});

/*
 * Half of what this product is for is Hebrew, and nothing rendered the
 * interface right-to-left. The layout is built from logical properties, which
 * means it mirrors for free or breaks silently — there is no third outcome, and
 * only one of the two is visible without looking.
 */
test('the interface mirrors for Hebrew without overflowing at any window size', async ({ page }) => {
  await stubApi(page);
  await page.setViewportSize(VIEWPORTS[1]);
  await page.goto('/?settings=general'); await loginFixture(page);

  await page.getByRole('group', { name: 'Interface language' }).getByRole('button', { name: 'עברית' }).click();
  expect(await page.evaluate(() => document.documentElement.dir)).toBe('rtl');
  expect(await page.evaluate(() => document.documentElement.lang)).toBe('he');

  // Top navigation keeps its full width and reverses the order of its items.
  const home = await page.locator('.sections .nav-item').first().boundingBox();
  const status = await page.locator('.status-chip').boundingBox();
  expect(home!.x).toBeGreaterThan(status!.x + status!.width);

  // The window-buttons group, when present, follows it to the other side.
  const titlebar = await page.locator('.titlebar').boundingBox();
  expect(titlebar!.width).toBeGreaterThan(0);

  for (const viewport of VIEWPORTS) {
    await page.setViewportSize(viewport);
    const size = `${viewport.width}x${viewport.height}`;
    for (const where of ['/?tab=home', '/?tab=dictionary', '/?tab=statistics', '/?settings=general', '/?settings=models', '/?settings=about']) {
      await page.goto(where); await loginFixture(page);
      // Exact: the settings dialog contributes a "Settings sections" landmark.
      await expect(page.getByRole('navigation', { name: 'Sections', exact: true })).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.dir), `direction at ${where}`).toBe('rtl');
      await assertNoPageScroll(page, `rtl ${where} at ${size}`);
    }
  }
});

test('hebrew, english and mixed dictionary entries keep their own direction', async ({ page }) => {
  await stubApi(page);
  await page.setViewportSize(VIEWPORTS[1]);
  await page.goto('/?tab=dictionary&sub=snippets'); await loginFixture(page);

  // `dir="auto"` lets the first strong character decide per entry, so a Hebrew
  // phrase and a Latin replacement can sit in one list without either flipping.
  // A saved entry reads as text until the pen opens it, so both states carry it.
  const values = page.locator('.entry-row .entry-value');
  await expect(values).toHaveText(['“המייל שלי”', 'writer@example.com']);
  await expect(values.first()).toHaveAttribute('dir', 'auto');
  await expect(values.last()).toHaveAttribute('dir', 'auto');

  await page.getByRole('button', { name: 'Edit snippet 1' }).click();
  const phrase = page.getByLabel('Snippet 1 spoken phrase');
  await expect(phrase).toHaveValue('המייל שלי');
  await expect(phrase).toHaveAttribute('dir', 'auto');
  await expect(page.getByLabel('Snippet 1 inserts exactly')).toHaveAttribute('dir', 'auto');
});

test('a Hebrew-capable face is bundled and used for speech text, not for chrome', async ({ page }) => {
  await stubApi(page);
  await page.setViewportSize(VIEWPORTS[1]);
  await page.goto('/?tab=dictionary&sub=snippets'); await loginFixture(page);

  // Bundled, not fetched: the desktop app runs against a private origin and has
  // to work offline, so a font CDN is not an option.
  await page.evaluate(() => document.fonts.ready);
  const loaded = await page.evaluate(() => [...document.fonts].map((face) => face.family));
  expect(loaded).toContain('Assistant');
  // Checked against Hebrew specifically: the subset that matters here is the one
  // Inter cannot cover, and it has to resolve from this origin rather than a CDN.
  const hebrew = await page.evaluate(async () => {
    const faces = await document.fonts.load('400 15px Assistant', 'שלום');
    return { count: faces.length, ready: document.fonts.check('400 15px Assistant', 'שלום') };
  });
  expect(hebrew.count).toBeGreaterThan(0);
  expect(hebrew.ready).toBe(true);

  const fonts = await page.evaluate(() => ({
    speech: getComputedStyle(document.documentElement).getPropertyValue('--text-font').trim(),
    chrome: getComputedStyle(document.body).fontFamily,
  }));
  // Inter keeps the chrome; its Hebrew coverage is not adequate for body text.
  expect(fonts.speech.startsWith('Assistant')).toBe(true);
  expect(fonts.chrome.startsWith('Inter')).toBe(true);
});

test('connection dot, label and detail share a vertical center', async ({ page }) => {
  await stubApi(page);
  await page.goto('/'); await loginFixture(page);
  await expect(page.locator('.status-chip')).toBeVisible();
  const centers = await page.locator('.status-chip').evaluate(chip =>
    ['.status-dot', 'strong', '.chip-detail'].map(selector => {
      const box = chip.querySelector(selector)!.getBoundingClientRect();
      return box.top + box.height / 2;
    }));
  expect(Math.max(...centers) - Math.min(...centers)).toBeLessThanOrEqual(1);
});
