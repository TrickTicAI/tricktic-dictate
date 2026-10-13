import { expect, test, type Page } from '@playwright/test';

/*
 * A parent re-render must not pull focus back to the dialog.
 *
 * The desktop shell re-renders roughly once a second while the update poll is
 * alive. The dialog's focus call was keyed on `onClose`, a fresh arrow on every
 * one of those renders, so it re-ran and took focus off whatever the pointer
 * had just opened. A native `select` popup is dismissed the moment its element
 * is blurred, which is why every dropdown in Settings closed on its own, and
 * the same theft dropped keystrokes out of the panel's text fields.
 *
 * Playwright cannot see a native popup, so these assert the cause rather than
 * the symptom: where focus sits across several re-renders.
 */

const focused = (page: Page) => page.evaluate(() => {
  const element = document.activeElement;
  if (!element) return 'none';
  return [element.tagName.toLowerCase(), element.getAttribute('aria-label'), element.className]
    .filter(Boolean).join(' ');
});
const ticks = (page: Page) => page.evaluate(() => (window as unknown as { ticks: number }).ticks);
const closeCalls = (page: Page) => page.evaluate(() => (window as unknown as { closeCalls: number }).closeCalls);

async function pastSeveralRenders(page: Page) {
  const start = await ticks(page);
  await expect.poll(() => ticks(page)).toBeGreaterThan(start + 3);
}

test('the dialog takes focus when it opens', async ({ page }) => {
  await page.goto('/src/features/settings/tests/modal-focus.html');
  await expect(page.getByRole('dialog', { name: 'Settings' })).toBeVisible();
  expect(await focused(page)).toContain('Settings');
});

test('a focused control keeps focus while the parent re-renders', async ({ page }) => {
  await page.goto('/src/features/settings/tests/modal-focus.html');
  await page.getByLabel('Output language').focus();

  await pastSeveralRenders(page);
  expect(await focused(page)).toBe('select Output language');
});

test('clicking a control leaves focus on it, not on the dialog', async ({ page }) => {
  await page.goto('/src/features/settings/tests/modal-focus.html');
  await page.getByLabel('Output language').click();

  await pastSeveralRenders(page);
  expect(await focused(page)).toBe('select Output language');
});

test('Escape still closes after many re-renders', async ({ page }) => {
  await page.goto('/src/features/settings/tests/modal-focus.html');
  await pastSeveralRenders(page);
  await page.keyboard.press('Escape');
  expect(await closeCalls(page)).toBe(1);
});
