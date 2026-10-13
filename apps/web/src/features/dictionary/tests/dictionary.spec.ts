import { expect, test, type Page } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import type { VocabularyDocument } from '@metrix-writer/contracts';

const empty = (): VocabularyDocument => ({ hints: [], corrections: [], snippets: [] });

/**
 * Stubs the saved dictionary and records every write the page makes. Like the
 * real API it tags each stored version and refuses a write whose If-Match names
 * an older one; `version` advances whenever `stored` is replaced.
 */
async function serve(page: Page, options: { failWrites?: boolean; failReads?: boolean; stored?: unknown; identify?: boolean; delayMs?: number } = {}) {
  const state = { stored: empty() as unknown, writes: [] as VocabularyDocument[], version: 1, refused: 0, identify: false, delayMs: 0, ...options };
  const identify = () => {
    let changed = false;
    for (const entries of Object.values(state.stored as VocabularyDocument)) for (const entry of entries) {
      if (!entry.id) { entry.id = randomUUID(); changed = true; }
    }
    if (changed) state.version += 1;
  };
  if (state.identify) identify();
  await page.route('**/api/v1/vocabulary', async (route) => {
    const write = route.request().method() === 'PUT';
    if (write ? state.failWrites : state.failReads) return route.fulfill({ status: 500, json: {} });
    if (write) {
      const expected = await route.request().headerValue('if-match');
      if (expected && expected !== `"v${state.version}"`) {
        state.refused += 1;
        return route.fulfill({ status: 412, json: { api_version: '1', error: { code: 'idempotency_conflict', message: 'changed', retryable: false } } });
      }
      const body = route.request().postDataJSON().data as VocabularyDocument;
      state.writes.push(body);
      state.stored = body;
      state.version += 1;
      if (state.identify) identify();
      if (state.delayMs) await new Promise((resolve) => setTimeout(resolve, state.delayMs));
    }
    await route.fulfill({ json: { api_version: '1', data: state.stored }, headers: { ETag: `"v${state.version}"` } });
  });
  return state as { stored: VocabularyDocument; writes: VocabularyDocument[]; version: number; refused: number; failWrites?: boolean; failReads?: boolean; delayMs: number; identify: boolean };
}

const open = (page: Page) => page.goto('/src/features/dictionary/tests/dictionary.html');
/** A word chip shows its Edit and Delete on hover, as a pointer user would see them. */
async function word(page: Page, action: 'Edit' | 'Delete' | 'Finish editing', n: number) {
  await page.locator('.entry-row.token').nth(n - 1).hover();
  await page.getByRole('button', { name: `${action} word ${n}` }).click();
}
const leave = (page: Page) => page.evaluate(() => window.leaveDictionary());
const reopen = (page: Page) => page.evaluate(() => window.openDictionary());

/** The add dialog: press the action, fill the fields it asks for, confirm. */
async function add(page: Page, kind: string, values: string[]) {
  await page.getByRole('button', { name: `Add ${kind}`, exact: true }).click();
  const dialog = page.getByRole('dialog');
  const fields = await dialog.locator('input, textarea').all();
  for (const [index, value] of values.entries()) await fields[index].fill(value);
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
}

test('an added word is stored, survives leaving the section, and needs no Save button', async ({ page }) => {
  const state = await serve(page);
  await open(page);

  // Adding is a dialog, not a permanent band of fields under the list.
  await expect(page.getByLabel('Word or phrase')).toHaveCount(0);
  await add(page, 'word', ['Metrix']);

  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  expect(state.stored.hints).toEqual([{ text: 'Metrix', enabled: true }]);
  // The section reports a real save, not an intention to save later.
  await expect(page.getByRole('button', { name: /^Save/ })).toHaveCount(0);
  // The saved document reaches the app so dictation uses it without a restart.
  await expect(page.getByTestId('saves')).toHaveText('1');
  await expect(page.getByTestId('active-words')).toHaveText('Metrix');

  await leave(page);
  await reopen(page);
  // A stored entry comes back as text. It becomes editable only when asked,
  // so a stray click in a long list cannot rewrite a rule.
  await expect(page.locator('.entry-value')).toHaveText('Metrix');
  await expect(page.getByLabel('Word 1', { exact: true })).toHaveCount(0);
  await word(page, 'Edit', 1);
  await expect(page.getByLabel('Word 1', { exact: true })).toHaveValue('Metrix');
  await word(page, 'Finish editing', 1);

  await word(page, 'Delete', 1);
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  expect(state.stored.hints).toEqual([]);
});

test('an entry is switched off in place, keeps its position, and stops reaching dictation', async ({ page }) => {
  const state = await serve(page);
  await open(page);
  await add(page, 'word', ['Metrix']);
  await add(page, 'word', ['ivrit']);
  await expect(page.getByTestId('active-words')).toHaveText('Metrix, ivrit');

  const first = page.locator('.entry-row').first();
  await page.getByRole('switch', { name: 'Use word 1' }).click();

  // Kept, in the order precedence depends on, and no longer applied.
  await expect.poll(() => state.stored.hints).toEqual([{ text: 'Metrix', enabled: false }, { text: 'ivrit', enabled: true }]);
  await expect(page.getByTestId('active-words')).toHaveText('ivrit');
  await expect(first).toHaveClass(/off/);
  await expect(first.locator('.entry-value')).toHaveText('Metrix');
  await expect(page.getByRole('switch', { name: 'Use word 1' })).toHaveAttribute('aria-checked', 'false');

  // And switching it back on needs nothing retyped.
  await page.getByRole('switch', { name: 'Use word 1' }).click();
  await expect.poll(() => state.stored.hints[0].enabled).toBe(true);
  await expect(page.getByTestId('active-words')).toHaveText('Metrix, ivrit');

  // A switched-off correction is the same: stored, listed, not applied.
  await page.getByRole('tab', { name: /Corrections/ }).click();
  await add(page, 'correction', ['free', 'three']);
  await expect(page.getByTestId('active-corrections')).toHaveText('free→three');
  await page.getByRole('switch', { name: 'Use correction 1' }).click();
  await expect(page.getByTestId('active-corrections')).toHaveText('');
  await expect.poll(() => state.stored.corrections).toEqual([{ from: 'free', to: 'three', enabled: false }]);
});

test('a dictionary saved before entries could be switched off loads with every one on', async ({ page }) => {
  // The shape a backend that predates the switch returns.
  const state = await serve(page, { stored: { hints: ['Metrix'], corrections: [{ from: 'met ricks', to: 'Metrix' }], snippets: [] } });
  await open(page);

  await expect(page.locator('.entry-value')).toHaveText('Metrix');
  await expect(page.getByRole('switch', { name: 'Use word 1' })).toHaveAttribute('aria-checked', 'true');
  // Nothing has been saved yet: reading an old document must not rewrite it.
  await expect(page.getByTestId('saves')).toHaveText('0');

  // The next write is in the shape that can carry a switch.
  await page.getByRole('switch', { name: 'Use word 1' }).click();
  await expect.poll(() => state.stored.hints).toEqual([{ text: 'Metrix', enabled: false }]);
  await expect.poll(() => state.stored.corrections).toEqual([{ from: 'met ricks', to: 'Metrix', enabled: true }]);
});

test('a correction still being typed is saved when the section is left', async ({ page }) => {
  const state = await serve(page);
  await open(page);
  await page.getByRole('tab', { name: /Corrections/ }).click();

  await add(page, 'correction', ['met ricks', 'Metrics']);
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();

  // Typed, then the section is left before the debounce elapses. Nothing here
  // moves focus, so this exercises the unmount flush rather than blur.
  await page.getByRole('button', { name: 'Edit correction 1' }).click();
  await page.getByLabel('Correction 1 replace with').fill('Metrix');
  await leave(page);
  await expect.poll(() => state.stored.corrections).toEqual([{ from: 'met ricks', to: 'Metrix', enabled: true }]);

  await reopen(page);
  await expect(page.locator('.entry-row .entry-value')).toHaveText(['met ricks', 'Metrix']);
});

// MW-OUATH-11 live: sync applied another device's edit while this one was
// being typed, and the whole-document save overwrote it.
test('an edit made on an older copy never overwrites a dictionary changed meanwhile', async ({ page }) => {
  const state = await serve(page, { stored: { hints: [{ text: 'שלום', enabled: true }], corrections: [], snippets: [] } });
  await open(page);
  await expect(page.locator('.entry-value')).toHaveText('שלום');

  // Another device's change lands in the stored dictionary; this page has not reloaded.
  state.stored = { hints: [{ text: 'שלוםא', enabled: true }], corrections: [], snippets: [] };
  state.version += 1;

  await word(page, 'Edit', 1);
  await page.getByLabel('Word 1', { exact: true }).fill('שלוםב');
  await word(page, 'Finish editing', 1);

  await expect(page.getByRole('alert')).toContainText('saved dictionary changed');
  expect(state.refused).toBe(1);
  expect(state.writes).toEqual([]);
  expect(state.stored.hints).toEqual([{ text: 'שלוםא', enabled: true }]);
  // What is stored now is shown; the refused draft is kept until explicitly discarded.
  await expect(page.locator('.entry-value')).toHaveText('שלוםא');
  await page.getByRole('button', { name: 'Discard unsaved changes' }).click();
  await word(page, 'Edit', 1);
  await page.getByLabel('Word 1', { exact: true }).fill('שלוםאב');
  await word(page, 'Finish editing', 1);
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
  expect(state.stored.hints).toEqual([{ text: 'שלוםאב', enabled: true }]);
});

test('a failed save keeps the entry on screen and retry sends exactly what was typed', async ({ page }) => {
  const state = await serve(page, { failWrites: true });
  await open(page);
  await page.getByRole('tab', { name: /Snippets/ }).click();

  await add(page, 'snippet', ['my email', 'writer@example.com']);

  await expect(page.getByRole('alert')).toContainText('your edits are still here', { ignoreCase: true });
  await expect(page.getByRole('status').filter({ hasText: 'Not saved' })).toBeVisible();
  await expect(page.locator('.entry-row .entry-value')).toHaveText(['“my email”', 'writer@example.com']);
  expect(state.stored.snippets).toEqual([]);

  state.failWrites = false;
  await page.getByRole('button', { name: 'Retry saving' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  expect(state.stored.snippets).toEqual([{ phrase: 'my email', text: 'writer@example.com', enabled: true }]);
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('a load failure asks for a retry instead of showing an empty dictionary', async ({ page }) => {
  const state = await serve(page, { failReads: true });
  await open(page);

  await expect(page.getByRole('alert')).toContainText('could not be loaded', { ignoreCase: true });
  await expect(page.getByRole('button', { name: 'Add word', exact: true })).toHaveCount(0);

  state.failReads = false;
  await page.getByRole('button', { name: 'Retry' }).click();
  await expect(page.getByRole('button', { name: 'Add word', exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('a correction reads as a rule, and right-click edits, switches off or deletes it', async ({ page }) => {
  const state = await serve(page);
  await open(page);
  await page.getByRole('tab', { name: /Corrections/ }).click();

  await add(page, 'correction', ['herdes', 'Hermes']);
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();

  // What was heard, an arrow, what it becomes — not two open input boxes.
  await expect(page.locator('.entry-row .entry-value')).toHaveText(['herdes', 'Hermes']);
  await expect(page.locator('.entry-row input, .entry-row textarea')).toHaveCount(0);
  await expect(page.locator('.entry-row .entry-arrow')).toBeVisible();

  // Writer's own menu, on the rule the pointer is actually over.
  await page.locator('.entry-row').click({ button: 'right' });
  await expect(page.getByRole('menu')).toBeVisible();
  await page.getByRole('menuitem', { name: 'Edit', exact: true }).click();
  await expect(page.getByLabel('Correction 1 heard as')).toHaveValue('herdes');

  // Inside the field the four editing commands are what the menu is for.
  await page.getByLabel('Correction 1 heard as').click({ button: 'right' });
  await expect(page.getByRole('menuitem')).toHaveText([/^Cut/, /^Copy/, /^Paste/, /^Select all/]);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Finish editing correction 1' }).click();
  await page.locator('.entry-row').click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Turn off' }).click();
  await expect.poll(() => state.stored.corrections[0].enabled).toBe(false);

  await page.locator('.entry-row').click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Delete' }).click();
  await expect.poll(() => state.stored.corrections).toEqual([]);
  await expect(page.getByText('Nothing here yet')).toBeVisible();
});

test('snippets are visible first, searchable, previewable and edited with explicit Save', async ({ page }) => {
  const state = await serve(page, { stored: { hints: [], corrections: [], snippets: [
    {phrase:'my email',text:'hello@example.com',enabled:true},
    {phrase:'my signature',text:'Kind regards,\nAlex\nProduct & Design',enabled:false},
  ]}});
  await open(page);
  await page.getByRole('tab', {name:/Snippets/}).click();
  await expect(page.locator('.snippet-preview')).toHaveCount(0);
  await expect(page.locator('.compact-snippet-row').first()).toBeInViewport();
  await expect(page.locator('video')).toHaveCount(0);
  await page.emulateMedia({reducedMotion:'reduce'});
  const row = page.locator('.compact-snippet-row').first();
  const before = await row.boundingBox();
  await page.getByRole('button',{name:'Preview snippet 1',exact:true}).click();
  await expect(row.getByRole('status')).toHaveText('Preview: hello@example.com');
  expect(await row.boundingBox()).toEqual(before);
  await page.getByRole('searchbox', { name: 'Search dictionary' }).fill('Kind regards');
  await expect(page.locator('.compact-snippet-row')).toHaveCount(1);
  await page.getByRole('button',{name:'Edit snippet 2',exact:true}).click();
  await page.getByLabel('Snippet 2 inserts exactly').fill('Unsaved text');
  await page.getByRole('button',{name:'Cancel',exact:true}).click();
  expect(state.writes).toHaveLength(0);
  await page.getByRole('button',{name:'Edit snippet 2',exact:true}).click();
  await expect(page.getByLabel('Snippet 2 inserts exactly')).toHaveValue('Kind regards,\nAlex\nProduct & Design');
  await page.getByLabel('Snippet 2 inserts exactly').fill('Updated signature');
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await expect.poll(()=>state.stored.snippets[1].text).toBe('Updated signature');
  await page.getByRole('searchbox', { name: 'Search dictionary' }).fill('');
  await page.getByRole('switch',{name:'Use snippet 2'}).click();
  await expect.poll(()=>state.stored.snippets[1].enabled).toBe(true);
  await leave(page);await reopen(page);
  await expect(page.locator('.compact-snippet-row').nth(1)).toContainText('Updated signature');
});

test('the cleaned video plays in a dismissible looping player and respects reduced motion', async ({ page }) => {
  await serve(page);
  await open(page);
  await page.getByRole('tab',{name:/Snippets/}).click();
  const trigger=page.getByRole('button',{name:'See how snippets work'});
  await page.emulateMedia({reducedMotion:'reduce'});
  await trigger.click();
  const video=page.locator('video');
  await expect(video).toBeVisible();
  await expect.poll(()=>video.evaluate((v: HTMLVideoElement)=>v.readyState)).toBeGreaterThanOrEqual(2);
  expect(await video.evaluate((v: HTMLVideoElement)=>v.paused)).toBe(true);
  expect(await video.evaluate((v: HTMLVideoElement)=>v.loop && v.muted && v.controls)).toBe(true);
  await video.evaluate((v: HTMLVideoElement)=>v.play());
  await expect.poll(()=>video.evaluate((v: HTMLVideoElement)=>v.currentTime)).toBeGreaterThan(.1);
  await video.evaluate((v: HTMLVideoElement)=>{v.currentTime=v.duration-.15;});
  await expect.poll(()=>video.evaluate((v: HTMLVideoElement)=>v.currentTime)).toBeLessThan(2);
  await page.keyboard.press('Escape');
  await expect(video).toHaveCount(0);await expect(trigger).toBeFocused();
  await page.emulateMedia({reducedMotion:'no-preference'});
  await trigger.click();
  await expect.poll(()=>page.locator('video').evaluate((v: HTMLVideoElement)=>v.currentTime)).toBeGreaterThan(.1);
  await page.getByRole('button',{name:'Close',exact:true}).click();
  await expect(page.locator('video')).toHaveCount(0);
});

test('words wrap as chips and rules keep both gutters and reachable actions at every size', async ({ page }) => {
  await serve(page, { stored: {
    hints: Array.from({length: 20}, (_, i) => ({ text: i === 0 ? 'LongWord'.repeat(35) : `Example word ${i}`, enabled: true })),
    corrections: [{ from: 'A long spoken correction '.repeat(8), to: 'A corrected phrase '.repeat(8), enabled: true }],
    snippets: Array.from({length: 8}, (_, i) => ({ phrase: i === 0 ? 'המייל שלי' : `My phrase ${i}`, text: 'Multiple lines\n'.repeat(5), enabled: true })),
  } });
  await open(page);
  for (const size of [{width: 600, height: 480}, {width: 760, height: 600}, {width: 1280, height: 800}, {width: 1920, height: 1080}]) {
    await page.setViewportSize(size);
    for (const name of ['Words', 'Corrections', 'Snippets']) {
      await page.getByRole('tab', { name: new RegExp(name) }).click();
      const first = page.locator('.entry-row').first();
      await first.scrollIntoViewIfNeeded();
      const geometry = await first.evaluate(row => {
        const box = row.getBoundingClientRect();
        const actions = row.querySelector('.row-actions')!.getBoundingClientRect();
        const cell = row.querySelector('.entry-cell')!.getBoundingClientRect();
        const content = document.querySelector('.dictionary-content')!;
        return { left: box.left, right: innerWidth - box.right, inset: cell.left - box.left, actionInset: box.right - actions.right,
          overflow: content.scrollWidth - content.clientWidth };
      });
      expect(geometry.left).toBeGreaterThanOrEqual(12);
      expect(geometry.right).toBeGreaterThanOrEqual(12);
      expect(geometry.overflow).toBe(0);
      if (name !== 'Words') {
        expect(geometry.inset).toBeGreaterThanOrEqual(16);
        expect(geometry.actionInset).toBeGreaterThanOrEqual(16);
      }
      const last = page.locator('.entry-row').last();
      await last.hover();
      await last.getByRole('button', { name: /^Edit / }).click();
      if (name === 'Snippets') await expect(page.getByRole('dialog', {name:'Edit snippet'})).toBeVisible();
      else await expect(last.locator('input')).toBeVisible();
      await page.keyboard.press('Escape');
    }
  }
});

test('deleting is instant and Undo puts the entry back where it was', async ({ page }) => {
  const state = await serve(page, { stored: { hints: ['Alpha', 'Bravo', 'Charlie'].map(text => ({ text, enabled: true })), corrections: [], snippets: [] } });
  await open(page);
  await word(page, 'Delete', 2);
  await expect.poll(() => state.stored.hints.map(h => h.text)).toEqual(['Alpha', 'Charlie']);
  const toast = page.locator('.dictionary-toast');
  await expect(toast).toContainText('Deleted “Bravo”');
  await toast.getByRole('button', { name: 'Undo' }).click();
  await expect.poll(() => state.stored.hints.map(h => h.text)).toEqual(['Alpha', 'Bravo', 'Charlie']);
  await expect(page.locator('.entry-row .entry-value')).toHaveText(['Alpha', 'Bravo', 'Charlie']);

  // Ctrl/⌘ Z does the same while the deletion can still be undone.
  await word(page, 'Delete', 1);
  await expect.poll(() => state.stored.hints.map(h => h.text)).toEqual(['Bravo', 'Charlie']);
  await page.locator('body').click({ position: { x: 5, y: 5 } });
  await page.keyboard.press('Control+z');
  await expect.poll(() => state.stored.hints.map(h => h.text)).toEqual(['Alpha', 'Bravo', 'Charlie']);
  await expect(toast).toHaveCount(0);
});

test('the add dialog catches blanks, duplicates and unusable words as you type', async ({ page }) => {
  await serve(page, { stored: { hints: [{ text: 'Tailscale', enabled: true }], corrections: [{ from: 'free', to: 'three', enabled: true }], snippets: [] } });
  await open(page);
  await page.getByRole('button', { name: 'Add word', exact: true }).click();
  const dialog = page.getByRole('dialog');
  const add = dialog.getByRole('button', { name: 'Add', exact: true });
  await expect(add).toBeDisabled();
  await dialog.getByLabel('Word or phrase').fill('tailscale');
  await expect(dialog.getByRole('status')).toHaveText('“tailscale” is already in your words.');
  await expect(add).toBeDisabled();
  await dialog.getByLabel('Word or phrase').fill('<b>');
  await expect(dialog.getByRole('status')).toContainText('can’t contain < or >');
  await dialog.getByLabel('Word or phrase').fill('Kubernetes');
  await expect(dialog.getByRole('status')).toHaveCount(0);
  await expect(add).toBeEnabled();
  await page.keyboard.press('Escape');

  await page.getByRole('tab', { name: /Corrections/ }).click();
  await page.getByRole('button', { name: 'Add correction', exact: true }).click();
  await dialog.getByLabel('When I hear').fill('free');
  await dialog.getByLabel('Write').fill('3');
  await expect(dialog.getByRole('status')).toHaveText('There is already a correction for “free”.');
  await expect(add).toBeDisabled();
});

test('one search covers all three tabs and says where the matches are', async ({ page }) => {
  await serve(page, { stored: {
    hints: [{ text: 'Kubernetes', enabled: true }],
    corrections: [{ from: 'tail scale', to: 'Tailscale', enabled: true }, { from: 'free', to: 'three', enabled: true }],
    snippets: [{ phrase: 'my vpn', text: 'Use Tailscale to connect.', enabled: true }],
  } });
  await open(page);
  const search = page.getByRole('searchbox', { name: 'Search dictionary' });
  await expect(search).toBeVisible();
  await page.keyboard.press('/');
  await expect(search).toBeFocused();
  await search.fill('tailscale');
  await expect(page.getByRole('tab', { name: /Corrections/ }).locator('.count')).toHaveText('1');
  await expect(page.getByRole('tab', { name: /Snippets/ }).locator('.count')).toHaveText('1');
  await page.getByRole('button', { name: '1 in Corrections' }).click();
  await expect(page.getByRole('tab', { name: /Corrections/ })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('.entry-row')).toHaveCount(1);
  await expect(page.locator('.entry-row .entry-value').first()).toHaveText('tail scale');
});

test('the words meter counts active words and warns before transcription would refuse them', async ({ page }) => {
  await serve(page, { stored: { hints: Array.from({ length: 130 }, (_, i) => ({ text: `term${i}`, enabled: i !== 0 })), corrections: [], snippets: [] } });
  await open(page);
  const meter = page.locator('.capacity');
  await expect(meter).toContainText('129 of 128 active');
  await expect(meter.getByRole('alert')).toContainText('every transcription fails');
  await expect(page.getByRole('tab', { name: /Words/ })).toContainText('1 off');
  await page.getByRole('switch', { name: 'Use word 2', exact: true }).click();
  await expect(meter).toContainText('128 of 128 active');
  await expect(meter.getByRole('alert')).toHaveCount(0);
});

test('N opens the add dialog for the open tab', async ({ page }) => {
  await serve(page);
  await open(page);
  await page.getByRole('tab', { name: /Snippets/ }).click();
  await page.locator('body').click({ position: { x: 5, y: 5 } });
  await page.keyboard.press('n');
  await expect(page.getByRole('dialog', { name: 'Add a snippet' })).toBeVisible();
});

const identifiedFixture = (): VocabularyDocument => ({
  hints: [{ id: randomUUID(), text: 'שלום Writer', enabled: true }],
  corrections: [{ id: randomUUID(), from: 'met ricks', to: 'Metrix', enabled: true }],
  snippets: [{ id: randomUUID(), phrase: 'my signature', text: 'שלום\nWriter ✓', enabled: true }],
});

test('sync identities survive edits, toggles, additions, deletion, undo and reopening', async ({ page }) => {
  const fixture = identifiedFixture();
  const state = await serve(page, { stored: fixture, identify: true });
  await open(page);
  await word(page, 'Edit', 1);
  await page.getByLabel('Word 1', { exact: true }).fill('שלום Writer updated');
  await word(page, 'Finish editing', 1);
  await expect.poll(() => state.writes.length).toBe(1);
  await page.getByRole('tab', { name: /Corrections/ }).click();
  await page.getByRole('button', { name: 'Edit correction 1' }).click();
  await page.getByLabel('Correction 1 replace with').fill('Metrix מעודכן');
  await page.getByRole('button', { name: 'Finish editing correction 1' }).click();
  await expect.poll(() => state.writes.length).toBe(2);
  await page.getByRole('tab', { name: /Snippets/ }).click();
  await page.getByRole('button', { name: 'Edit snippet 1' }).click();
  await page.getByLabel('Snippet 1 inserts exactly').fill('Updated שלום\nWriter ✓');
  await page.getByRole('dialog').getByRole('button', { name: 'Save', exact: true }).click();
  await expect.poll(() => state.writes.length).toBe(3);
  await page.getByRole('switch', { name: 'Use snippet 1' }).click();
  await expect.poll(() => state.writes.length).toBe(4);
  await page.getByRole('tab', { name: /Words/ }).click();
  await add(page, 'word', ['New word']);
  await expect.poll(() => state.stored.hints.length).toBe(2);
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  const newId = state.stored.hints[1].id;
  expect(newId).toBeTruthy();
  await word(page, 'Delete', 2);
  await expect.poll(() => state.stored.hints.length).toBe(1);
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect.poll(() => state.stored.hints.length).toBe(2);
  expect(state.stored.hints[1].id).toBe(newId);
  await leave(page); await reopen(page);
  await page.getByRole('switch', { name: 'Use word 1', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  expect(state.refused).toBe(0);
  expect(state.stored.hints[0].id).toBe(fixture.hints[0].id);
  expect(state.stored.corrections[0].id).toBe(fixture.corrections[0].id);
  expect(state.stored.snippets[0].id).toBe(fixture.snippets[0].id);
});

test('a delayed save response adopts IDs without replacing newer edits or deletions', async ({ page }) => {
  const state = await serve(page, { identify: true, delayMs: 600 });
  await open(page);
  await add(page, 'word', ['First']);
  await expect.poll(() => state.writes.length).toBe(1);
  // The first response is held back while the same row is edited.
  await word(page, 'Edit', 1);
  await page.getByLabel('Word 1', { exact: true }).fill('First updated');
  await word(page, 'Finish editing', 1);
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  const firstId = state.stored.hints[0].id;
  expect(state.stored.hints[0].text).toBe('First updated');
  expect(firstId).toBeTruthy();
  await add(page, 'word', ['Second']);
  await expect.poll(() => state.writes.length).toBe(3);
  // Deleting the first row shifts the newly added row while its response is pending.
  await word(page, 'Delete', 1);
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  expect(state.stored.hints.map((entry) => entry.text)).toEqual(['Second']);
  expect(state.stored.hints[0].id).toBeTruthy();
  expect(state.stored.hints[0].id).not.toBe(firstId);
  expect(state.refused).toBe(0);
});

test('a refused draft survives navigation and recovery keeps unrelated synced entries', async ({ page }) => {
  const fixture = identifiedFixture();
  const state = await serve(page, { stored: structuredClone(fixture), identify: true });
  await open(page);
  await expect(page.locator('.entry-value')).toHaveText('שלום Writer');
  // The other device changes the same word and adds an unrelated snippet.
  state.stored.hints[0].text = 'Saved remotely';
  const remoteSnippet = { id: randomUUID(), phrase: 'remote only', text: 'Kept remotely', enabled: true };
  state.stored.snippets.push(remoteSnippet); state.version += 1;
  await word(page, 'Edit', 1);
  await page.getByLabel('Word 1', { exact: true }).fill('My unsaved שלום');
  await word(page, 'Finish editing', 1);
  await expect(page.getByRole('alert')).toContainText('unsaved changes are kept');
  await leave(page); await reopen(page);
  await page.getByRole('button', { name: 'Review unsaved changes' }).click();
  const dialog = page.getByRole('dialog', { name: 'Review unsaved dictionary changes' });
  await expect(dialog).toContainText('My unsaved שלום');
  await expect(dialog).toContainText('Saved remotely');
  await expect(dialog.getByRole('button', { name: 'Apply reviewed changes' })).toBeDisabled();
  await dialog.getByLabel('Resolve My unsaved שלום').selectOption('mine');
  await dialog.getByRole('button', { name: 'Apply reviewed changes' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  expect(state.stored.hints[0]).toEqual({ ...fixture.hints[0], text: 'My unsaved שלום' });
  expect(state.stored.snippets).toEqual([...fixture.snippets, remoteSnippet]);
  expect(state.refused).toBe(1);
});

test('reviewed deletion preserves unrelated remote edits without demanding a conflict choice', async ({ page }) => {
  const fixture = identifiedFixture();
  const state = await serve(page, { stored: structuredClone(fixture), identify: true });
  await open(page);
  await expect(page.locator('.entry-value')).toHaveText('שלום Writer');
  state.stored.snippets[0].text = 'Remote exact text'; state.version += 1;
  await word(page, 'Delete', 1);
  await page.getByRole('button', { name: 'Review unsaved changes' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('Your change: Deleted');
  await expect(dialog.locator('select')).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Apply reviewed changes' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  expect(state.stored.hints).toEqual([]);
  expect(state.stored.snippets[0].text).toBe('Remote exact text');
  expect(state.stored.snippets[0].id).toBe(fixture.snippets[0].id);
});

test('explicitly recreating an entry deleted remotely gives it a new sync identity', async ({ page }) => {
  const fixture = identifiedFixture();
  const state = await serve(page, { stored: structuredClone(fixture), identify: true });
  await open(page);
  await expect(page.locator('.entry-value')).toHaveText('שלום Writer');
  state.stored.hints = []; state.version += 1;
  await word(page, 'Edit', 1);
  await page.getByLabel('Word 1', { exact: true }).fill('Recreated שלום');
  await word(page, 'Finish editing', 1);
  await page.getByRole('button', { name: 'Review unsaved changes' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('Saved: Deleted');
  await dialog.getByLabel('Resolve Recreated שלום').selectOption('mine');
  await dialog.getByRole('button', { name: 'Apply reviewed changes' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  expect(state.stored.hints[0].text).toBe('Recreated שלום');
  expect(state.stored.hints[0].id).not.toBe(fixture.hints[0].id);
});

test('a desktop sync event cannot retarget a snippet dialog that is still being edited', async ({ page }) => {
  await page.addInitScript(() => {
    let listener: ((event: { payload: { kind: string } }) => void) | undefined;
    window.__TAURI__ = {
      core: { invoke: async () => { throw new Error('No native command expected'); } },
      event: { listen: async (name, callback) => {
        if (name === 'writer-sync') listener = callback as typeof listener;
        return () => { listener = undefined; };
      } },
    };
    (window as unknown as { emitTestSync: () => void }).emitTestSync = () => listener?.({ payload: { kind: 'applied' } });
  });
  await page.route('http://tauri.localhost/**', async (route) => {
    const url = new URL(route.request().url());
    await route.fulfill({ response: await page.request.get(`http://127.0.0.1:15188${url.pathname}${url.search}`) });
  });
  const fixture = identifiedFixture();
  const state = await serve(page, { stored: structuredClone(fixture), identify: true });
  await page.goto('http://tauri.localhost/src/features/dictionary/tests/dictionary.html?sub=snippets');
  await page.getByRole('button', { name: 'Edit snippet 1' }).click();
  await page.getByLabel('Snippet 1 inserts exactly').fill('My exact draft שלום');
  const remoteSnippet = { id: randomUUID(), phrase: 'new remote snippet', text: 'Do not replace this', enabled: true };
  state.stored.snippets.unshift(remoteSnippet); state.version += 1;
  await page.evaluate(() => (window as unknown as { emitTestSync: () => void }).emitTestSync());
  await page.getByRole('dialog').getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('unsaved changes are kept');
  expect(state.refused).toBe(1);
  expect(state.stored.snippets[0]).toEqual(remoteSnippet);
  await page.getByRole('button', { name: 'Review unsaved changes' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Apply reviewed changes' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  expect(state.stored.snippets[0]).toEqual(remoteSnippet);
  expect(state.stored.snippets[1]).toEqual({ ...fixture.snippets[0], text: 'My exact draft שלום' });
});

test('a failed conflict reload keeps the draft and blocks review until the saved version is readable', async ({ page }) => {
  const state = await serve(page, { stored: identifiedFixture(), identify: true });
  await open(page);
  await expect(page.locator('.entry-value')).toHaveText('שלום Writer');
  state.stored.hints[0].text = 'Current saved word'; state.version += 1;
  state.failReads = true;
  await word(page, 'Delete', 1);
  const review = page.getByRole('button', { name: 'Review unsaved changes' });
  await expect(page.getByRole('alert').filter({ hasText: 'could not be loaded' })).toBeVisible();
  await expect(review).toBeDisabled();
  state.failReads = false;
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await review.click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('Your change: Deleted');
  await dialog.getByLabel('Resolve שלום Writer').selectOption('current');
  await dialog.getByRole('button', { name: 'Apply reviewed changes' }).click();
  await expect(page.getByRole('alert')).toHaveCount(0);
  expect(state.stored.hints[0].text).toBe('Current saved word');
});
