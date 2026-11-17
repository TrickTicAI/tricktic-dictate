import { expect, test, type Page } from '@playwright/test';

/*
 * Home warns before saved data reaches its storage limit and says so plainly
 * when a dictation was refused for it. The desktop shell's `storage_budget`
 * answers from its last measurement; here it is a fixture.
 */
const GB = 1024 ** 3;
async function open(page: Page, used: number, limit = 5 * GB) {
  await page.route('http://tauri.localhost/**', async route => {
    const response = await route.fetch({ url: route.request().url().replace('http://tauri.localhost', 'http://127.0.0.1:15193') });
    await route.fulfill({ response });
  });
  await page.route('**/api/v1/jobs/*/live-processing', route => route.fulfill({ json: { api_version: '1', data: { bound: false, attempt: null } } }));
  await page.route('**/api/v1/jobs/*/processed', route => route.fulfill({ json: { api_version: '1', data: { text: '' } } }));
  await page.route('**/api/v1/jobs/*/processing', route => route.fulfill({ json: { api_version: '1', data: [] } }));
  await page.route('**/api/v1/cleanup/providers', route => route.fulfill({ json: { api_version: '1', data: [] } }));
  await page.route('**/api/v1/modes', route => route.fulfill({ json: { api_version: '1', data: { revision: 1, active: 'regular', profiles: [
    { id: 'regular', revision: 1, name: 'Regular', kind: 'regular', instructions: '', text_model: null, context: { application: false, selected_text: false, clipboard: false } }] } } }));
  await page.route('**/api/v1/modes/providers', route => route.fulfill({ json: { api_version: '1', data: [] } }));
  await page.addInitScript(({ used, limit }) => {
    const w = window as unknown as Record<string, unknown>;
    w.budgetReads = 0;
    w.__TAURI__ = {
      core: { invoke: async (command: string) => {
        if (command === 'storage_budget') { (w.budgetReads as number)++; return { used_bytes: used, limit_bytes: limit }; }
        return undefined;
      } },
      event: { listen: async () => () => {} },
    };
  }, { used, limit });
  await page.goto('http://tauri.localhost/src/features/home/tests/home.html');
  await expect(page.locator('.session-row').first()).toBeVisible();
}
const notice = (page: Page) => page.locator('.home-problem', { hasText: 'Storage is' });
const toast = (page: Page) => page.locator('.toast');

test('below 90% of the limit nothing is said', async ({ page }) => {
  await open(page, 4.4 * GB);
  await expect.poll(() => page.evaluate(() => (window as unknown as { budgetReads: number }).budgetReads)).toBeGreaterThan(0);
  await expect(notice(page)).toHaveCount(0);
  await expect(toast(page)).toHaveCount(0);
});

test('past 90% Home warns, announces it once, and points to where space is freed', async ({ page }) => {
  await open(page, 4.6 * GB);
  await expect(notice(page)).toHaveText('Storage is almost full: 4.6 GB of 5.0 GB used. At the limit, new dictations can’t be transcribed. Free space in Settings › Privacy & data › Storage. Free up space');
  await expect(notice(page)).toHaveAttribute('role', 'status');
  await expect(toast(page)).toContainText('Storage is almost full');
  await notice(page).getByRole('button', { name: 'Free up space' }).click();
  expect(await page.evaluate(() => window.freeSpace)).toBe(1);

  // The notice stays; the announcement does not repeat for the same level.
  await page.reload();
  await expect(notice(page)).toBeVisible();
  await page.waitForTimeout(300);
  await expect(toast(page)).toHaveCount(0);
});

test('at the limit the warning is an alert, and a refused dictation says storage is full', async ({ page }) => {
  await open(page, 5 * GB);
  await expect(notice(page)).toContainText('Storage is full: 5.0 GB of 5.0 GB used. New dictations can’t be transcribed until you free space in Settings › Privacy & data › Storage.');
  await expect(notice(page)).toHaveAttribute('role', 'alert');
  await expect(toast(page)).toContainText('Storage is full');

  const reads = await page.evaluate(() => (window as unknown as { budgetReads: number }).budgetReads);
  await page.evaluate(() => window.session({ state: 'capturing', id: 'live', text: '' }));
  await page.evaluate(() => window.session({ state: 'saving' }));
  // Never read while a dictation records or saves.
  expect(await page.evaluate(() => (window as unknown as { budgetReads: number }).budgetReads)).toBe(reads);
  await page.evaluate(() => window.session({ state: 'error', errorCode: 'storage_full' }));
  await expect(page.getByRole('region', { name: 'Dictation', exact: true })).toContainText('Storage is full — your audio is saved');
  // A settled dictation reads the budget again.
  await expect.poll(() => page.evaluate(() => (window as unknown as { budgetReads: number }).budgetReads)).toBeGreaterThan(reads);
});

test('a new limit chosen in Settings is read at once, between dictations', async ({ page }) => {
  await open(page, 4.4 * GB);
  await expect.poll(() => page.evaluate(() => (window as unknown as { budgetReads: number }).budgetReads)).toBeGreaterThan(0);
  const reads = await page.evaluate(() => (window as unknown as { budgetReads: number }).budgetReads);
  await page.evaluate(() => window.dispatchEvent(new Event('writer-storage-limit-changed')));
  await expect.poll(() => page.evaluate(() => (window as unknown as { budgetReads: number }).budgetReads)).toBe(reads + 1);
});
