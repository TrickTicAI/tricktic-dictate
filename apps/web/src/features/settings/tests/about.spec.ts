import { expect, test, type Page } from '@playwright/test';
import { LEGAL_LINKS } from '../../../product';

/*
 * About's legal links and the offline licenses view (MW-RELEASE-4). The desktop
 * window may not open other windows, so it names a link and the shell opens a
 * fixed address; the licenses view reads only files the shell lists.
 */

const INDEX = [
  { path: 'notices/MODELS-NOTICES.txt', title: 'Downloadable models', group: 'Models', kind: 'text' },
  { path: 'notices/WEB-NOTICES.txt', title: 'Interface libraries and fonts', group: 'Application', kind: 'text' },
  { path: 'notices/RUST-NOTICES.txt', title: 'Application libraries (Rust)', group: 'Application', kind: 'text' },
  { path: 'notices/ffmpeg-LICENSE.md', title: 'ffmpeg-LICENSE.md', group: 'Speech and text runtime', kind: 'text' },
  { path: 'notices/source/README.txt', title: 'FFmpeg source code', group: 'Speech and text runtime', kind: 'text' },
  { path: 'notices/source/ffmpeg-8.0.3.tar.xz', title: 'ffmpeg-8.0.3.tar.xz', group: 'Speech and text runtime', kind: 'source' },
];

type Call = { command: string; args?: Record<string, unknown> };
const calls = (page: Page) => page.evaluate(() => (window as unknown as { nativeCalls: Call[] }).nativeCalls);

test.beforeEach(async ({ page }) => {
  await page.route('http://tauri.localhost/**', async (route) => {
    const response = await route.fetch({ url: route.request().url().replace('http://tauri.localhost', 'http://127.0.0.1:15191') });
    await route.fulfill({ response });
  });
  await page.addInitScript((index) => {
    const nativeCalls: Call[] = [];
    Object.assign(window, { nativeCalls, __TAURI__: { event: { listen: async () => () => {} }, core: { invoke: async (command: string, args?: Record<string, unknown>) => {
      nativeCalls.push({ command, args });
      if (command === 'notices_index') return sessionStorage.getItem('noIndex') ? [] : index;
      if (command === 'notice_text') {
        if (args?.path === 'notices/RUST-NOTICES.txt') throw 'This notice is missing. Repair the installation.';
        return `Full text of ${String(args?.path)}\nParakeet TDT 0.6B v3 by NVIDIA Corporation, licensed under CC BY 4.0`;
      }
      if (command === 'legal_open') { if (args?.kind === 'terms') throw 'The page could not be opened. It is at https://example.invalid/terms'; return; }
      if (command === 'notices_folder') return;
    } } } });
  }, INDEX);
});

test('the desktop About opens each legal page through the shell by name', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/settings/tests/about.html');
  for (const [row, kind] of [['Privacy policy', 'privacy'], ['Support', 'support'], ['Report a security issue', 'security']] as const) {
    await page.locator('.setting-row').filter({ hasText: row }).getByRole('button').click();
    expect((await calls(page)).at(-1)).toEqual({ command: 'legal_open', args: { kind } });
  }
  await expect(page.getByText('contact@tricktic.com')).toBeVisible();
  // No link in the desktop window navigates it away.
  await expect(page.locator('a[href]')).toHaveCount(0);
  await page.locator('.setting-row').filter({ hasText: 'Terms of use' }).getByRole('button').click();
  await expect(page.getByRole('alert')).toContainText('It is at https://example.invalid/terms');
});

test('the licenses view lists every installed notice and reads one at a time', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/settings/tests/about.html');
  await page.locator('.setting-row').filter({ hasText: 'Open-source licenses' }).getByRole('button', { name: 'View' }).click();
  await expect(page.getByRole('heading', { name: 'Open-source licenses' })).toBeVisible();
  for (const group of ['Models', 'Application', 'Speech and text runtime']) await expect(page.getByRole('heading', { name: group })).toBeVisible();
  for (const notice of INDEX.filter((n) => n.kind === 'text')) await expect(page.getByRole('button', { name: notice.title, exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Downloadable models', exact: true }).click();
  await expect(page.getByLabel('Downloadable models text')).toContainText('licensed under CC BY 4.0');
  expect((await calls(page)).filter((call) => call.command === 'notice_text')).toEqual([{ command: 'notice_text', args: { path: 'notices/MODELS-NOTICES.txt' } }]);

  await page.getByRole('button', { name: 'Application libraries (Rust)', exact: true }).click();
  await expect(page.getByRole('alert')).toHaveText('This notice is missing. Repair the installation.');

  // The source archive is not text; the view shows where it is installed.
  await expect(page.getByText('ffmpeg-8.0.3.tar.xz · source code, installed in the notices folder')).toBeVisible();
  await page.getByRole('button', { name: 'Show folder' }).click();
  expect((await calls(page)).at(-1)).toEqual({ command: 'notices_folder', args: undefined });

  await page.getByRole('button', { name: 'About' }).click();
  await expect(page.getByText('Version 0.22.0')).toBeVisible();
});

test('a build without installed notices says so', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/settings/tests/about.html');
  await page.evaluate(() => sessionStorage.setItem('noIndex', '1'));
  await page.reload();
  await page.getByRole('button', { name: 'View' }).click();
  await expect(page.getByText('This build has no installed notices. Installed releases include them.')).toBeVisible();
});

test('the browser client links to the same pages directly', async ({ page }) => {
  await page.goto('/src/features/settings/tests/about.html?browser');
  await expect(page.getByRole('link', { name: 'Open' }).first()).toHaveAttribute('href', LEGAL_LINKS.privacy);
  await expect(page.getByRole('link', { name: 'Email' })).toHaveAttribute('href', 'mailto:contact@tricktic.com');
  await expect(page.getByRole('link', { name: 'Open' }).last()).toHaveAttribute('href', LEGAL_LINKS.security);
  await expect(page.getByRole('button', { name: 'View' })).toHaveCount(0);
});
