import { expect, test, type Page } from '@playwright/test';
import type { DesktopPreferences } from '@metrix-writer/contracts/desktop';

/*
 * The shortcut is set by pressing the combination, not by typing its name.
 *
 * The old shell asked for the string `Ctrl+Shift+F9` in a text box. That breaks
 * under a Hebrew layout, where the character a key produces is not the key the
 * shell has to register, so these tests press physical keys and check what is
 * stored.
 */

test.beforeEach(async ({ page }) => { await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'Win32', configurable: true })); });

declare global {
  interface Window {
    nativeTest: {
      events: Map<string, (event: { payload: unknown }) => void>;
      calls: { command: string; args?: { active?: boolean } }[];
      state: { trusted: boolean };
    };
  }
}

const saves = (page: Page) => page.evaluate(() => (window as unknown as { saves: DesktopPreferences[] }).saves);

async function openCapture(page: Page, which: 'Record and stop dictation' | 'Discard dictation') {
  const row = page.locator('.setting-row').filter({ hasText: which });
  await row.getByRole('button', { name: 'Change…' }).click();
  await expect(page.getByRole('dialog', { name: /Set the .* shortcut/ })).toBeVisible();
}

test('pressing a combination stores it; releasing commits it', async ({ page }) => {
  await page.goto('/src/features/settings/tests/shortcuts.html');
  await expect(page.locator('.setting-row').filter({ hasText: 'Record and stop dictation' }).locator('kbd').first()).toHaveText('Ctrl');

  await openCapture(page, 'Record and stop dictation');
  await page.keyboard.down('Control');
  await page.keyboard.down('Alt');
  // Held but incomplete: the field shows progress and waits for a real key.
  await expect(page.getByText('waiting for a key…')).toBeVisible();
  await page.keyboard.down('KeyR');
  await page.keyboard.up('KeyR');
  await page.keyboard.up('Alt');
  await page.keyboard.up('Control');

  await expect(page.getByRole('dialog', { name: /Set the .* shortcut/ })).toBeHidden();
  expect((await saves(page)).at(-1)?.recording).toBe('Ctrl+Alt+R');
  await expect(page.locator('.setting-row').filter({ hasText: 'Record and stop dictation' }).locator('kbd').nth(2)).toHaveText('R');
});

test('a key with no modifier and F12 are refused with a reason', async ({ page }) => {
  await page.goto('/src/features/settings/tests/shortcuts.html');
  await openCapture(page, 'Record and stop dictation');

  await page.keyboard.down('KeyG');
  await expect(page.getByRole('alert')).toContainText('Add at least one of Ctrl, Alt, Shift or Win');
  await page.keyboard.up('KeyG');

  await page.keyboard.down('Control');
  await page.keyboard.down('F12');
  await expect(page.getByRole('alert')).toContainText('F12 is reserved for debuggers');
  await page.keyboard.up('F12');
  await page.keyboard.up('Control');

  // Nothing invalid was ever committed, and the dialog is still open.
  expect(await saves(page)).toEqual([]);
  await expect(page.getByRole('dialog', { name: /Set the .* shortcut/ })).toBeVisible();
});

test('a chord that collides with the other shortcut names it and offers the fix', async ({ page }) => {
  await page.goto('/src/features/settings/tests/shortcuts.html');
  await openCapture(page, 'Record and stop dictation');

  // Only the exact complete combination collides.
  await page.keyboard.down('Control');
  await page.keyboard.down('Shift');
  await page.keyboard.down('F10');
  await expect(page.getByRole('alert')).toContainText('Ctrl+Shift+F10 is already used to discard dictation');
  await expect(page.getByRole('alert')).toContainText('change that shortcut first');
  await page.keyboard.up('F10');
  await page.keyboard.up('Shift');
  await page.keyboard.up('Control');

  expect(await saves(page)).toEqual([]);
  await expect(page.getByRole('dialog', { name: /Set the .* shortcut/ })).toBeVisible();
});

test('reset restores the default and cancel changes nothing', async ({ page }) => {
  await page.goto('/src/features/settings/tests/shortcuts.html');

  await openCapture(page, 'Record and stop dictation');
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByRole('dialog', { name: /Set the .* shortcut/ })).toBeHidden();
  expect(await saves(page)).toEqual([]);

  await openCapture(page, 'Discard dictation');
  await page.getByRole('button', { name: 'Reset to default' }).click();
  expect((await saves(page)).at(-1)?.cancel).toBe('Ctrl+Shift+F10');
});

test('hold and toggle are a two-option choice, each with its own explanation', async ({ page }) => {
  await page.goto('/src/features/settings/tests/shortcuts.html');
  const row = page.locator('.setting-row').filter({ hasText: 'How the shortcut behaves' });
  await expect(row).toContainText('recording lasts exactly as long as you hold the keys');

  await row.getByRole('button', { name: 'Press to start and stop' }).click();
  expect((await saves(page)).at(-1)?.toggle).toBe(true);
  await expect(row).toContainText('one press starts recording, the next stops it');
});

for (const order of [['Meta', 'Alt'], ['Alt', 'Meta']]) {
  test(`Win+Alt modifier-only capture supports ${order.join(' then ')}`, async ({ page }) => {
    await page.goto('/src/features/settings/tests/shortcuts.html');
    await openCapture(page, 'Record and stop dictation');
    await page.keyboard.down(order[0]!); await page.keyboard.down(order[1]!);
    await expect(page.getByRole('alert')).toHaveCount(0);
    await page.keyboard.up(order[1]!); await page.keyboard.up(order[0]!);
    expect((await saves(page)).at(-1)?.recording).toBe('Win+Alt');
  });
}
test('Windows text goes where the dictation stops unless the window it started in is chosen', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'Win32' }));
  await page.goto('/src/features/settings/tests/shortcuts.html');
  const row = page.locator('.setting-row').filter({ hasText: 'Where the text goes' });
  // A document from before the choice existed means "where I stop".
  await expect(row.getByRole('button', { name: 'Where I stop' })).toHaveAttribute('aria-pressed', 'true');
  await expect(row).toContainText('in front of you when you stop');
  await row.getByRole('button', { name: 'Where I started' }).click();
  expect((await saves(page)).at(-1)?.paste_into).toBe('start');
  await expect(row).toContainText('the window you started dictating in');
  await row.getByRole('button', { name: 'Where I stop' }).click();
  expect((await saves(page)).at(-1)?.paste_into).toBe('stop');
});
test('Win plus a physical key can be saved for native registration', async ({ page }) => {
  await page.goto('/src/features/settings/tests/shortcuts.html');
  await openCapture(page, 'Record and stop dictation');
  await page.keyboard.down('Meta'); await page.keyboard.down('KeyD');
  await page.keyboard.up('KeyD'); await page.keyboard.up('Meta');
  expect((await saves(page)).at(-1)?.recording).toBe('Win+D');
});


test('Mac settings save paste targets and explain Accessibility fallback', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'MacIntel' }));
  await page.goto('/src/features/settings/tests/shortcuts.html');
  const row = page.locator('.setting-row').filter({ hasText: 'Where the text goes' });
  await expect(row.getByRole('button', { name: 'Where I stop', exact: true })).toBeEnabled();
  await expect(page.getByText(/Without it, completed text stays on the clipboard and in history/)).toBeVisible();
  await expect(page.getByText(/The fn shortcut is unavailable without Accessibility/)).toBeVisible();
  await row.getByRole('button', { name: 'Where I started' }).click();
  expect((await saves(page)).at(-1)?.paste_into).toBe('start');
});

for (const key of ['Meta', 'Alt']) {
  test(`Mac rejects bare ${key} and captures fn through the explicit choice`, async ({ page }) => {
    await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'MacIntel' }));
    await page.goto('/src/features/settings/tests/shortcuts.html');
    await openCapture(page, 'Record and stop dictation');
    await page.keyboard.down(key);
    await expect(page.getByRole('alert')).toContainText('Use fn alone, or at least two modifiers');
    await page.keyboard.up(key);
    expect(await saves(page)).toEqual([]);
    await page.getByRole('button', { name: 'Use fn', exact: true }).click();
    expect((await saves(page)).at(-1)?.recording).toBe('Fn');
    await expect(page.locator('.setting-row').filter({ hasText: 'Record and stop dictation' }).locator('kbd')).toHaveText('fn');
  });
}
test('Mac physical Hebrew letter and modifier glyphs round trip', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'MacIntel' }));
  await page.goto('/src/features/settings/tests/shortcuts.html');
  await openCapture(page, 'Record and stop dictation');
  await page.evaluate(() => {
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyE', key: 'ק', metaKey: true, bubbles: true }));
    window.dispatchEvent(new KeyboardEvent('keyup', { code: 'KeyE', key: 'ק', metaKey: true, bubbles: true }));
  });
  expect((await saves(page)).at(-1)?.recording).toBe('Win+E');
  await expect(page.locator('.setting-row').filter({ hasText: 'Record and stop dictation' }).locator('kbd').first()).toHaveText('⌘');
});

test('native fn edges capture and release without starting a dictation; access rechecks on focus', async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'platform', { value: 'MacIntel' });
    const events = new Map<string, (event: { payload: unknown }) => void>();
    const calls: { command: string; args: unknown }[] = [];
    const state = { trusted: false };
    Object.assign(window, {
      nativeTest: { events, calls, state },
      __TAURI__: {
        core: { invoke: async (command: string, args: unknown) => {
          calls.push({ command, args });
          if (command === 'accessibility_status') return { supported: true, trusted: state.trusted, tap_active: state.trusted, globe_action: null };
          return null;
        } },
        event: { listen: async (name: string, callback: (event: { payload: unknown }) => void) => {
          events.set(name, callback); return () => events.delete(name);
        } },
      },
    });
  });
  await page.route('http://tauri.localhost/**', async route => {
    const url = new URL(route.request().url());
    const response = await route.fetch({ url: `http://127.0.0.1:15191${url.pathname}${url.search}` });
    await route.fulfill({ response });
  });
  await page.goto('http://tauri.localhost/src/features/settings/tests/shortcuts.html');
  await expect(page.getByText(/The fn shortcut is unavailable without Accessibility/)).toBeVisible();
  await page.getByRole('button', { name: 'Open Accessibility settings' }).click();
  await page.evaluate(() => {
    const native = window.nativeTest;
    native.state.trusted = true;
    window.dispatchEvent(new Event('focus'));
  });
  await expect(page.getByText('Accessibility enabled. The fn shortcut is listening.')).toBeVisible();
  await openCapture(page, 'Record and stop dictation');
  await expect.poll(() => page.evaluate(() => window.nativeTest.calls.some((call) => call.command === 'shortcut_capture' && call.args?.active))).toBe(true);
  await page.evaluate(() => window.nativeTest.events.get('writer-fn-capture')!({ payload: { pressed: true, ctrl: false, alt: false, shift: false, meta: false } }));
  await expect(page.locator('.capture-slot kbd')).toHaveText('fn');
  expect(await saves(page)).toEqual([]);
  await page.evaluate(() => window.nativeTest.events.get('writer-fn-capture')!({ payload: { pressed: false, ctrl: false, alt: false, shift: false, meta: false } }));
  expect((await saves(page)).at(-1)?.recording).toBe('Fn');
  await expect.poll(() => page.evaluate(() => window.nativeTest.calls.some((call) => call.command === 'shortcut_capture' && !call.args?.active))).toBe(true);
  expect(await page.evaluate(() => window.nativeTest.calls.some((call) => call.command === 'capture_start'))).toBe(false);
});

test('Windows clipboard preservation opts in without changing paste target or shortcuts', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'Win32' }));
  await page.goto('/src/features/settings/tests/shortcuts.html');
  const toggle = page.getByRole('switch', { name: 'Preserve clipboard after dictation' });
  await expect(toggle).toHaveAttribute('aria-checked', 'false');
  await toggle.click();
  expect((await saves(page)).at(-1)).toMatchObject({ preserve_clipboard: true, recording: 'Ctrl+Shift+F9', cancel: 'Ctrl+Shift+F10' });
  await page.getByRole('button', { name: 'Where I started', exact: true }).click();
  expect((await saves(page)).at(-1)).toMatchObject({ preserve_clipboard: true, paste_into: 'start' });
  await toggle.click();
  expect((await saves(page)).at(-1)).toMatchObject({ preserve_clipboard: false, paste_into: 'start' });
});

test('Mac clipboard preservation opts in with Mac wording', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'MacIntel' }));
  await page.goto('/src/features/settings/tests/shortcuts.html');
  await expect(page.getByText(/once the paste has read the dictation/)).toBeVisible();
  const toggle = page.getByRole('switch', { name: 'Preserve clipboard after dictation' });
  await toggle.click();
  expect((await saves(page)).at(-1)).toMatchObject({ preserve_clipboard: true, recording: 'Ctrl+Shift+F9' });
});

test('private clipboard is opt-in and independent of preservation and paste target', async ({ page }) => {
  await page.goto('/src/features/settings/tests/shortcuts.html');
  const privacy = page.getByRole('switch', { name: 'Private clipboard delivery', exact: true });
  await expect(privacy).not.toBeChecked();
  await expect(page.getByText(/Exclude automatic dictation from Windows/)).toBeVisible();
  await page.getByRole('switch', { name: 'Preserve clipboard after dictation', exact: true }).click();
  await privacy.click();
  expect((await saves(page)).at(-1)).toMatchObject({ private_clipboard: true, preserve_clipboard: true });
  await expect(page.getByText(/Restoration does not prevent other apps/)).toBeVisible();
  await page.getByRole('button', { name: 'Where I started', exact: true }).click();
  expect((await saves(page)).at(-1)).toMatchObject({ private_clipboard: true, preserve_clipboard: true, paste_into: 'start' });
  await privacy.click();
  expect((await saves(page)).at(-1)).toMatchObject({ private_clipboard: false, preserve_clipboard: true });
  await expect(page.getByText(/ahead of dictation in Win\+V/)).toBeVisible();
});

test('Mac clipboard privacy discloses cooperative markers and sync limits', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'MacIntel', configurable: true }));
  await page.goto('/src/features/settings/tests/shortcuts.html');
  await expect(page.getByText(/managers such as CopyQ may ignore them/)).toBeVisible();
  await page.getByRole('switch', { name: 'Private clipboard delivery', exact: true }).click();
  expect((await saves(page)).at(-1)).toMatchObject({ private_clipboard: true });
  await expect(page.getByText(/cross-device sync is not guaranteed to be blocked/)).toBeVisible();
});
