import { expect, test, type Page } from '@playwright/test';
// The rows the shell sends, in the shape tray.rs publishes them.
import { TRAY_THEMES, trayPayload } from '../../../../preview/tray-demo.mjs';

type Call = { command: string; args: Record<string, unknown> };

async function open(page: Page, state = 'ready', extra: Record<string, unknown> = {}, platform: 'windows' | 'macos' = 'windows') {
  const payload = { ...trayPayload(state, TRAY_THEMES.paper, { platform }), ...extra };
  await page.addInitScript((payload) => {
    const calls: Call[] = [];
    Object.assign(window, { trayCalls: calls, __TAURI__: {
      event: { listen: async () => () => {} },
      core: { invoke: async (command: string, args: Record<string, unknown> = {}) => {
        calls.push({ command, args });
        return command === 'tray_menu_ready' ? payload : null;
      } },
    } });
  }, payload);
  await page.setViewportSize({ width: 420, height: 900 });
  await page.goto('/desktop-tray.html');
  await expect(page.locator('#panel')).toBeVisible();
}
const calls = (page: Page) => page.evaluate(() => (window as unknown as { trayCalls: Call[] }).trayCalls);
const sent = async (page: Page) => (await calls(page)).filter((call) => call.command === 'tray_menu_command').map((call) => call.args.id);

test('opens on whether you can dictate, the shortcut as keycaps and the last dictation', async ({ page }) => {
  await open(page);
  const status = page.locator('.row.status');
  await expect(status).toHaveAttribute('data-tone', 'ready');
  await expect(status).toContainText('Ready to dictate');
  await expect(status).toContainText('Whisper large-v3-turbo · English · Super');
  const start = page.getByRole('menuitem', { name: /Start dictation/ });
  await expect(start.locator('kbd')).toHaveText(['Ctrl', 'Shift', 'F9']);
  await expect(page.locator('.heading').first()).toHaveText(/^Last dictation · \S/);
  // Each quick switch names its current value.
  await expect(page.getByRole('menuitem', { name: /Microphone/ })).toContainText('Shure MV7+');
  await expect(page.getByRole('menuitem', { name: /Mode/ }).filter({ hasText: 'Super' })).toBeVisible();
  // Discard appears only while there is a dictation to discard.
  await expect(page.getByRole('menuitem', { name: /Discard dictation/ })).toHaveCount(0);
  // The shell sizes the window from what the page measured.
  const measured = (await calls(page)).find((call) => call.command === 'tray_menu_measured');
  expect(Number(measured?.args.width)).toBeGreaterThanOrEqual(288 + 28);
  expect(Number(measured?.args.height)).toBeGreaterThan(400);
});

test('a recording counts up, and every row sends back the id the shell issued', async ({ page }) => {
  await open(page, 'recording');
  const title = page.locator('.row.status strong');
  await expect(title).toHaveText(/^Recording · 0:0[78]$/);
  const first = await title.textContent();
  await expect(title).not.toHaveText(first ?? '', { timeout: 2500 });
  await expect(page.getByRole('menuitem', { name: /Discard dictation/ })).toHaveClass(/danger/);
  await page.getByRole('menuitem', { name: 'Copy it again' }).click();
  await page.getByRole('menuitem', { name: /Recording/ }).click();
  expect(await sent(page)).toEqual(['copy-pasted', 'open']);
});

test('submenus drill in place, settings are switches, and the keyboard reaches every row', async ({ page }) => {
  await open(page);
  await page.getByRole('menuitem', { name: /Preferences/ }).click();
  await expect(page.locator('#back')).toHaveText('Preferences');
  const panel = page.getByRole('menuitemcheckbox', { name: /Floating panel/ });
  await expect(panel).toHaveAttribute('aria-checked', 'true');
  await expect(panel.locator('.switch')).toBeVisible();
  await expect(page.getByRole('menuitemcheckbox', { name: /Quiet other audio/ })).toHaveAttribute('aria-checked', 'false');
  await page.keyboard.press('Escape');
  await expect(page.locator('#back')).toBeHidden();

  // The first arrow press lands on the status row, the next on Start.
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('.row.status')).toHaveClass(/active/);
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  // Into Mode with the arrow that points into a submenu, then choose.
  await page.getByRole('menuitem', { name: /^Mode/ }).hover();
  await page.keyboard.press('ArrowRight');
  await expect(page.locator('#back')).toHaveText('Mode');
  await expect(page.getByRole('menuitemcheckbox', { name: /Command/ })).toBeDisabled();
  await expect(page.getByRole('menuitemcheckbox', { name: /Command/ })).toContainText('Needs a text model');
  await page.getByRole('menuitemcheckbox', { name: /Regular/ }).click();
  expect(await sent(page)).toEqual(['dictate', 'mode:regular']);
  // Escape at the root closes the menu.
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  expect((await calls(page)).some((call) => call.command === 'tray_menu_dismiss')).toBe(true);
});

test('a Mac draws its own key symbols; reduced animations stop the spinner', async ({ page }) => {
  await open(page, 'working', { reduced_motion: true }, 'macos');
  await expect(page.getByRole('menuitem', { name: /Start dictation/ }).locator('kbd')).toHaveText(['⌃', '⇧', 'Space']);
  await expect(page.getByRole('menuitem', { name: /Start with Windows|Launch at login/ })).toHaveCount(0);
  await page.getByRole('menuitem', { name: /Preferences/ }).click();
  await expect(page.getByRole('menuitemcheckbox', { name: 'Launch at login' })).toBeVisible();
  await expect(page.getByRole('menuitemcheckbox', { name: /Quiet other audio/ })).toHaveCount(0);
  await page.keyboard.press('Escape');
  expect(await page.locator('.row.status .spin').evaluate((spinner) => getComputedStyle(spinner).animationName)).toBe('none');
});

test('a Hebrew interface mirrors the menu but keeps a quotation in its own direction', async ({ page }) => {
  await open(page, 'ready', { direction: 'rtl' });
  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  const chevron = page.getByRole('menuitem', { name: /Language/ }).locator('.chevron');
  expect(await chevron.evaluate((icon) => getComputedStyle(icon).transform)).toContain('matrix(-1');
  await expect(page.locator('.note bdi')).toHaveCount(1);
});
