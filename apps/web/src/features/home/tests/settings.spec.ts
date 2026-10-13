import { expect, test } from '@playwright/test';
test.beforeEach(async ({ page }) => {
  await page.route('http://tauri.localhost/**', async route => {
    const response = await route.fetch({ url: route.request().url().replace('http://tauri.localhost', 'http://127.0.0.1:15193') });
    await route.fulfill({ response });
  });
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'platform', { value: 'Win32', configurable: true });
    const calls: { command: string; args: unknown }[] = [];
    Object.assign(window, { writerCanDeleteHistory: () => !sessionStorage.getItem('busyOwner'), nativeFixture: calls, __TAURI__: { core: { invoke: async (command: string, args: { settings?: unknown; changed?: string }) => {
      calls.push({ command, args });
      if (command === 'data_location') return { root: 'C:\\WriterData' };
      if (command === 'storage_status') return { data_bytes: 3 * 1024 ** 3, data_limit_bytes: 5 * 1024 ** 3, audio_bytes: 3 * 1024 ** 3 - 1024 ** 2, state_bytes: 1024 ** 2, models_bytes: 2 * 1024 ** 3 };
      if (command === 'storage_preview') return { id: 'confirmed-preview', recordings: 2, jobs: 3, history: (args as unknown as { history: boolean }).history, before_ms: (args as unknown as { beforeMs: number | null }).beforeMs };
      if (command === 'storage_clear') {
        if (!sessionStorage.getItem('allowDelete')) throw 'Wait for transcription to finish before clearing storage.';
        return { recording_ids: ['old-recording'], history: sessionStorage.getItem('deleteText') === 'yes' };
      }
      if (command === 'history_delete') {
        if (sessionStorage.getItem('allowDelete')) return { deleted_recordings: 1, deleted_jobs: 1 };
        throw 'Stop recording before deleting history.';
      }
      if (command === 'history_resume' && sessionStorage.getItem('failResume')) throw 'Resume unavailable';
      if (command === 'system_settings_get') return JSON.parse(sessionStorage.getItem('system') || '{"launch_at_login":false,"sounds":false,"duck_audio":false}');
      if (command === 'system_settings_save') { if (sessionStorage.getItem('refuseLogin')) throw 'Approve TrickTic Dictate in Login Items.'; sessionStorage.setItem('system', JSON.stringify(args.settings)); return; }
    } } } });
  });
});
const nativeCalls = (page: import('@playwright/test').Page) =>
  page.evaluate(() => (window as unknown as { nativeFixture: { command: string; args: { beforeMs: number | null; history: boolean } }[] }).nativeFixture);
/** Picks what to delete and from when in Free up space, then asks to review it. */
async function choose(page: import('@playwright/test').Page, what: 'Audio only' | 'Audio and transcripts', from: 'Before today' | 'Every dictation') {
  await page.getByRole('group', { name: 'What to delete' }).getByRole('button', { name: what, exact: true }).click();
  await page.getByRole('group', { name: 'From' }).getByRole('button', { name: from, exact: true }).click();
  await page.getByRole('button', { name: 'Review what will be deleted…' }).click();
}

test('storage shows what takes the space and reviews a deletion from local midnight first', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html');
  const usage = page.getByRole('img', { name: /^Recordings/ });
  await expect(usage).toHaveAttribute('aria-label', 'Recordings 3.0 GB, Transcripts and settings 2.0 MB, Downloaded models 2.0 GB');
  await expect(page.getByText('used by TrickTic Dictate on this PC', { exact: false })).toBeVisible();
  await expect(page.getByText('uses 3.0 GB of its 5.0 GB limit', { exact: false })).toBeVisible();

  // Audio only, before today: the defaults.
  await page.getByRole('button', { name: 'Review what will be deleted…' }).click();
  await expect(page.getByText(/^Delete the audio of 2 dictations from before /)).toBeVisible();
  const preview = (await nativeCalls(page)).find((call) => call.command === 'storage_preview')!;
  expect(preview.args.history).toBe(false);
  expect(await page.evaluate((ms) => { const date = new Date(ms!); return [date.getHours(), date.getMinutes(), date.getSeconds(), date.getMilliseconds()]; }, preview.args.beforeMs)).toEqual([0, 0, 0, 0]);
  expect((await nativeCalls(page)).some((call) => call.command === 'storage_clear')).toBe(false);
  await page.getByRole('button', { name: 'Keep them' }).click();
  await expect(page.getByRole('button', { name: 'Delete permanently' })).toHaveCount(0);

  // Each choice says what it deletes and keeps before anything is reviewed.
  await choose(page, 'Audio and transcripts', 'Before today');
  await expect(page.getByText('history sync stops on this device', { exact: false })).toBeVisible();
  await expect(page.getByText(/^Delete 2 dictations and 3 transcription attempts from before /)).toBeVisible();
  await page.getByRole('button', { name: 'Delete permanently' }).click();
  await expect(page.getByText('Wait for transcription to finish before clearing storage.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Delete permanently' })).toBeEnabled();
});

test('deleting all audio keeps cached transcripts, and deleting older history removes only those', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html');
  await page.evaluate(async () => {
    sessionStorage.setItem('allowDelete', 'yes');
    const request = indexedDB.open('metrix-writer-transcription', 1);
    await new Promise<void>((resolve, reject) => {
      request.onupgradeneeded = () => request.result.createObjectStore('attempts', { keyPath: 'id' });
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        const db = request.result; const tx = db.transaction('attempts', 'readwrite');
        tx.objectStore('attempts').put({ id: 'old-recording', transcript: 'yesterday' });
        tx.objectStore('attempts').put({ id: 'today-recording', transcript: 'today' });
        tx.oncomplete = () => { db.close(); resolve(); };
      };
    });
  });
  const ids = () => page.evaluate(() => new Promise<string[]>(resolve => {
    const request = indexedDB.open('metrix-writer-transcription', 1);
    request.onsuccess = () => { const db = request.result; const tx = db.transaction('attempts'); const keys = tx.objectStore('attempts').getAllKeys(); tx.oncomplete = () => { db.close(); resolve(keys.result as string[]); }; };
  }));
  await choose(page, 'Audio only', 'Every dictation');
  expect((await nativeCalls(page)).find((call) => call.command === 'storage_preview')!.args.beforeMs).toBeNull();
  await Promise.all([page.waitForEvent('load'), page.getByRole('button', { name: 'Delete permanently' }).click()]);
  expect(await ids()).toEqual(['old-recording', 'today-recording']);
  await page.evaluate(() => sessionStorage.setItem('deleteText', 'yes'));
  await choose(page, 'Audio and transcripts', 'Before today');
  await Promise.all([page.waitForEvent('load'), page.getByRole('button', { name: 'Delete permanently' }).click()]);
  expect(await ids()).toEqual(['today-recording']);
});

test('deleting every recording and transcript is confirmed and surfaces the native busy refusal', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html');
  await expect(page.getByText('C:\\WriterData', { exact: true })).toBeVisible();
  await choose(page, 'Audio and transcripts', 'Every dictation');
  await expect(page.getByText('Delete every recording and transcript?')).toBeVisible();
  await page.getByRole('button', { name: 'Keep history' }).click();
  expect((await nativeCalls(page)).some((call) => call.command === 'history_delete' || call.command === 'storage_preview')).toBe(false);
  await page.getByRole('button', { name: 'Review what will be deleted…' }).click();
  await page.getByRole('button', { name: 'Delete permanently' }).click();
  await expect(page.getByText('Stop recording before deleting history.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Delete permanently' })).toBeEnabled();
  expect((await nativeCalls(page)).filter((call) => call.command.startsWith('history_')).map((call) => call.command)).toEqual(['history_delete', 'history_resume']);
});
test('login and sound settings read and persist through native commands', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html?section=system');
  await page.getByRole('switch', { name: 'Launch TrickTic Dictate at login' }).click();
  await expect(page.getByRole('switch', { name: 'Launch TrickTic Dictate at login' })).toHaveAttribute('aria-checked', 'true');
  await page.getByRole('switch', { name: 'Dictation and notification sounds' }).click();
  const saves = await page.evaluate(() => (window as unknown as { nativeFixture: { command: string; args: unknown }[] }).nativeFixture.filter(call => call.command === 'system_settings_save'));
  expect(saves).toHaveLength(2);
  expect(saves[1].args).toEqual({ settings: { launch_at_login: true, sounds: true, duck_audio: false }, changed: 'sounds' });
  await page.getByRole('switch', { name: 'Reduce other audio while dictating' }).click();
  await expect(page.getByRole('switch', { name: 'Reduce other audio while dictating' })).toHaveAttribute('aria-checked', 'true');
});

test('panel visibility and size are three explicit choices and one settled slider', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html?section=system');
  const saved = () => page.evaluate(() => (window as unknown as { preferenceSaves: { visibility: string; scale: number }[] }).preferenceSaves);

  await page.getByRole('button', { name: 'Always on screen' }).click();
  expect((await saved()).at(-1)).toMatchObject({ visibility: 'always' });
  await page.getByRole('button', { name: 'Never' }).click();
  expect((await saved()).at(-1)).toMatchObject({ visibility: 'off' });
  // The harness keeps what was saved, as the shell does, and the size of a
  // panel that is never shown is not editable; turn it back on first.
  await page.getByRole('button', { name: 'While dictating' }).click();
  expect((await saved()).at(-1)).toMatchObject({ visibility: 'recording' });

  const size = page.getByRole('slider', { name: 'Panel size' });
  await size.fill('2');
  // The reading follows the drag immediately; the write is settled, so a drag
  // across the range is not dozens of native saves.
  await expect(page.getByText('200%', { exact: true })).toBeVisible();
  await expect.poll(async () => (await saved()).at(-1)?.scale).toBe(2);
  await size.fill('0.75');
  await expect.poll(async () => (await saved()).at(-1)?.scale).toBe(0.75);
});

test('the panel preview is the real panel at its actual size and follows the slider as it moves', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html?section=system');
  const preview = page.locator('iframe[title="Floating panel preview"]');
  const frame = page.frameLocator('iframe[title="Floating panel preview"]');
  const width = async () => (await preview.boundingBox())?.width ?? 0;
  // A demonstration runs in it: listening, then processing.
  await expect(frame.locator('body')).toHaveAttribute('data-state', 'capturing');
  await expect(frame.locator('body')).toHaveAttribute('data-state', 'transcribing', { timeout: 6000 });
  expect(await width()).toBeCloseTo(112, 0);

  await page.getByRole('slider', { name: 'Panel size' }).fill('2');
  await expect.poll(width).toBeCloseTo(224, 0);
  await expect.poll(() => frame.locator('html').evaluate((root) => getComputedStyle(root).getPropertyValue('--scale').trim())).toBe('2');

  await page.getByRole('button', { name: 'Top centre' }).click();
  await expect(page.locator('.panel-stage')).toHaveAttribute('data-placement', 'top');
  await page.getByRole('button', { name: 'Never' }).click();
  await expect(page.getByText('The panel is turned off.', { exact: true })).toBeVisible();
  await expect(preview).toBeHidden();
});

test('panel colour presets and the custom pickers save one colour preference and repaint the preview', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html?section=system');
  const saves = () => page.evaluate(() => (window as unknown as { preferenceSaves: { panel_colors: unknown }[] }).preferenceSaves);
  const saved = async () => (await saves()).at(-1)?.panel_colors;
  const frame = page.frameLocator('iframe[title="Floating panel preview"]');
  const fill = () => frame.locator('section').evaluate((pill) => getComputedStyle(pill).backgroundColor);
  await expect(page.getByRole('button', { name: 'Match theme' })).toHaveAttribute('aria-pressed', 'true');
  // Paper's elevated surface, which is what the panel paints under that theme.
  await expect.poll(fill).toBe('rgb(253, 251, 247)');

  await page.getByRole('button', { name: 'Dracula' }).click();
  expect(await saved()).toEqual({ background: '#282a36', wave: '#bd93f9' });
  await expect.poll(fill).toBe('rgb(40, 42, 54)');
  await page.getByRole('button', { name: 'Black on white' }).click();
  expect(await saved()).toEqual({ background: '#ffffff', wave: '#000000' });
  await expect(page.getByRole('button', { name: 'Black on white' })).toHaveAttribute('aria-pressed', 'true');

  // Custom opens on the colours in use and writes nothing until one is picked.
  const count = (await saves()).length;
  await page.getByRole('button', { name: 'Custom' }).click();
  await expect(page.getByLabel('Panel background')).toHaveValue('#ffffff');
  expect((await saves()).length).toBe(count);
  await page.getByLabel('Panel background').fill('#123456');
  await expect.poll(fill).toBe('rgb(18, 52, 86)');
  await expect.poll(saved).toEqual({ background: '#123456', wave: '#000000' });
  await page.getByLabel('Voice animation').fill('#ffcc00');
  await expect.poll(saved).toEqual({ background: '#123456', wave: '#ffcc00' });
  await expect.poll(() => frame.locator('section').evaluate((pill) => getComputedStyle(pill).borderTopColor)).toBe('rgba(255, 204, 0, 0.2)');

  await page.getByRole('button', { name: 'Match theme' }).click();
  expect(await saved()).toBeNull();
  await expect.poll(fill).toBe('rgb(253, 251, 247)');
});
test('successful deletion clears cached transcript attempts before reloading', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html');
  await page.evaluate(async () => {
    sessionStorage.setItem('allowDelete', 'yes');
    await new Promise<void>((resolve, reject) => {
      const request = indexedDB.open('metrix-writer-transcription', 1);
      request.onupgradeneeded = () => request.result.createObjectStore('attempts', { keyPath: 'id' });
      request.onsuccess = () => {
        const db = request.result;
        const tx = db.transaction('attempts', 'readwrite');
        tx.objectStore('attempts').put({ id: 'old-recording', transcript: 'retained text' });
        tx.oncomplete = () => { db.close(); resolve(); };
        tx.onerror = () => reject(tx.error);
      };
    });
  });
  await choose(page, 'Audio and transcripts', 'Every dictation');
  await Promise.all([
    page.waitForEvent('load'),
    page.getByRole('button', { name: 'Delete permanently' }).click(),
  ]);
  expect(await page.evaluate(() => new Promise<number>((resolve, reject) => {
    const request = indexedDB.open('metrix-writer-transcription', 1);
    request.onsuccess = () => {
      const db = request.result;
      const tx = db.transaction('attempts');
      const count = tx.objectStore('attempts').count();
      tx.oncomplete = () => { db.close(); resolve(count.result); };
      tx.onerror = () => reject(tx.error);
    };
  }))).toBe(0);
});

test('pending owner work prevents native history deletion', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html');
  await page.evaluate(() => sessionStorage.setItem('busyOwner', 'yes'));
  // Refused before anything is reviewed, whichever deletion is chosen.
  await choose(page, 'Audio and transcripts', 'Every dictation');
  await expect(page.getByText('Finish recording and transcription before freeing space.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Delete permanently' })).toHaveCount(0);
  await choose(page, 'Audio only', 'Before today');
  await expect(page.getByText('Finish recording and transcription before freeing space.', { exact: true })).toBeVisible();
  expect((await nativeCalls(page)).some((call) => call.command === 'history_delete' || call.command === 'storage_preview')).toBe(false);
});

test('cache failure requests native resumption and preserves the original error', async ({ page }) => {
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html');
  await page.evaluate(() => {
    sessionStorage.setItem('allowDelete', 'yes');
    sessionStorage.setItem('failResume', 'yes');
    Object.defineProperty(indexedDB, 'open', { value: () => { throw new Error('Fixture cache failure'); } });
  });
  await choose(page, 'Audio and transcripts', 'Every dictation');
  await page.getByRole('button', { name: 'Delete permanently' }).click();
  await expect(page.getByText('Fixture cache failure', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Delete permanently' })).toBeEnabled();
  expect(await page.evaluate(() => (window as unknown as { nativeFixture: { command: string }[] }).nativeFixture.filter(call => call.command.startsWith('history_')).map(call => call.command))).toEqual(['history_delete', 'history_resume']);
});

test('Mac system settings disable ducking and re-read external Login Items changes', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'MacIntel', configurable: true }));
  await page.goto('http://tauri.localhost/src/features/home/tests/settings.html?section=system');
  await expect(page.getByRole('switch', { name: 'Reduce other audio while dictating' })).toBeDisabled();
  await expect(page.getByText(/other apps do not expose restorable per-app session volumes/)).toBeVisible();
  const login = page.getByRole('switch', { name: 'Launch TrickTic Dictate at login' });
  await login.click();
  await expect(login).toHaveAttribute('aria-checked', 'true');
  await page.evaluate(() => {
    sessionStorage.setItem('system', JSON.stringify({ launch_at_login: false, sounds: false, duck_audio: false }));
    window.dispatchEvent(new Event('focus'));
  });
  await expect(login).toHaveAttribute('aria-checked', 'false');
  await page.getByRole('switch', { name: 'Dictation and notification sounds' }).click();
  await expect(login).toHaveAttribute('aria-checked', 'false');
  await page.evaluate(() => sessionStorage.setItem('refuseLogin', 'true'));
  await login.click();
  await expect(page.getByText('Approve TrickTic Dictate in Login Items.')).toBeVisible();
  await expect(login).toHaveAttribute('aria-checked', 'false');
});
