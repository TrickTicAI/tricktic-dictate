import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { SYNONYMS } from '../search';
import { PRODUCT_NAME } from '../../../product';

/*
 * Search reads the rendered sections, so these run against the real dialog
 * (the desktop harness, on a non-Mac platform) rather than a fixture index.
 */

async function search(page: Page, query: string) {
  await page.getByRole('searchbox', { name: 'Search settings' }).fill(query);
  return page.getByRole('list', { name: 'Matching settings' });
}
const result = (list: ReturnType<Page['getByRole']>, name: RegExp) => list.getByRole('button', { name });

test.beforeEach(async ({ page }) => {
  await page.goto('/src/features/settings/tests/modal-focus.html');
});

test('a misspelled word still finds every setting about it, titles first', async ({ page }) => {
  const list = await search(page, 'Cleeboard');
  await expect(result(list, /^Preserve clipboard after dictation/)).toBeVisible();
  await expect(result(list, /^Private clipboard delivery/)).toBeVisible();
  await expect(result(list, /^Clipboard, exports and backups/)).toBeVisible();
  await expect(list.getByRole('button').first()).toHaveAccessibleName(/clipboard/i);
});

test('options, descriptions and names inside a setting are searchable', async ({ page }) => {
  // An option of a segmented control.
  let list = await search(page, 'hold to dictate');
  await expect(result(list, /^How the shortcut behaves/)).toBeVisible();
  // A word only in a description, shown with the text that matched.
  list = await search(page, 'DPAPI');
  await expect(result(list, /^Provider keys.*DPAPI/)).toBeVisible();
  // A placement choice, and a name used by both a theme and a panel colour preset.
  list = await search(page, 'top centre');
  await expect(result(list, /^Placement/)).toBeVisible();
  list = await search(page, 'dracula');
  await expect(result(list, /Appearance/).first()).toBeVisible();
  await expect(result(list, /^Colours/)).toBeVisible();
});

test('words the interface does not use still find their setting', async ({ page }) => {
  let list = await search(page, 'hotkey');
  await expect(result(list, /^Record and stop dictation/)).toBeVisible();
  list = await search(page, 'startup');
  await expect(result(list, /^Launch TrickTic Dictate at login/)).toBeVisible();
  list = await search(page, 'dark mode');
  await expect(list.getByRole('button').first()).toHaveAccessibleName(/^Theme/);
});

test('every typed word has to match, and nothing matches nonsense', async ({ page }) => {
  await search(page, 'clipboard qwxzv');
  await expect(page.getByRole('status').filter({ hasText: 'No settings match' })).toBeVisible();
});

test('settings this platform does not show are not found', async ({ page }) => {
  // The harness is not a Mac, so the Mac-only Accessibility card is absent.
  await search(page, 'Accessibility for dictation and paste');
  await expect(page.getByRole('button', { name: /^Accessibility for dictation and paste/ })).toHaveCount(0);
});

test('every synonym still names a title its section renders', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const sources: Record<string, string[]> = {
    general: ['General.tsx', 'System.tsx'],
    appearance: ['Appearance.tsx'],
    shortcuts: ['Shortcuts.tsx', 'Accessibility.tsx'],
    microphone: ['Microphone.tsx', 'System.tsx'],
    panel: ['System.tsx'],
    engine: ['Transcription.tsx', '../transcription-setup/TranscriptionSetup.tsx'],
    text: ['../modes/TextModels.tsx'],
    modes: ['../modes/ModeSettings.tsx'],
    keys: ['Keys.tsx'],
    account: ['DataPrivacy.tsx'],
    data: ['DataPrivacy.tsx', 'StorageControls.tsx'],
    about: ['About.tsx'],
  };
  for (const entry of SYNONYMS) {
    // Titles that name the product render PRODUCT_NAME, so read the sources with it substituted.
    const text = sources[entry.section].map((file) => readFileSync(join(here, '..', file), 'utf8')).join('\n')
      .replaceAll('${PRODUCT_NAME}', PRODUCT_NAME).replaceAll('{PRODUCT_NAME}', PRODUCT_NAME);
    expect(text, `${entry.section}: ${entry.title}`).toContain(entry.title);
  }
});

test('choosing a result opens its section and marks the setting', async ({ page }) => {
  const list = await search(page, 'Cleeboard');
  await result(list, /^Private clipboard delivery/).click();
  await expect(page.locator('.modal-head h2')).toHaveText('Shortcuts & pasting');
  await expect(page.getByRole('searchbox', { name: 'Search settings' })).toHaveValue('');
  await expect(page.locator('.setting-row.search-hit')).toContainText('Private clipboard delivery');
});

test('the keyboard reaches a result, and Escape clears the search before closing', async ({ page }) => {
  const box = page.getByRole('searchbox', { name: 'Search settings' });
  await search(page, 'launch at login');
  await expect(result(page.getByRole('list', { name: 'Matching settings' }), /^Launch TrickTic Dictate at login/)).toBeVisible();
  await box.press('Enter');
  await expect(page.locator('.modal-head h2')).toHaveText('General');

  await box.fill('nothing like this exists');
  await expect(page.getByRole('status').filter({ hasText: 'No settings match' })).toBeVisible();
  await box.press('Escape');
  await expect(box).toHaveValue('');
  expect(await page.evaluate(() => (window as unknown as { closeCalls: number }).closeCalls)).toBe(0);
  await box.press('Escape');
  expect(await page.evaluate(() => (window as unknown as { closeCalls: number }).closeCalls)).toBe(1);
});
