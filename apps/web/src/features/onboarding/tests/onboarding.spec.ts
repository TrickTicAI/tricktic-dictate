import { expect, test, type Page } from '@playwright/test';
import type { ModelView, SetupView, WeightsState, WorkerState } from '../../transcription-setup/api';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'Win32', configurable: true }));
});

const pagePath = '/src/features/onboarding/tests/onboarding.html';

const installed: WeightsState = { state: 'installed', bytes: 574_000_000, sha256: 'aa', verified_at_ms: 1_800_000_000_000, origin: 'download' };

function model(id: string, name: string, bytes: number, languages: ModelView['input_languages'], weights: WeightsState = { state: 'not_installed', resumable_bytes: null }): ModelView {
  return {
    id, display_name: name, file_name: `${id}.bin`, bytes, sha256: 'a'.repeat(64),
    source_url: `https://huggingface.co/x/${id}`, source_name: 'ggerganov/whisper.cpp', license: 'MIT',
    notes: 'Multilingual weights.', input_languages: languages, weights, selected: false, loaded: false,
  };
}

function view(overrides: Partial<SetupView> = {}, worker: WorkerState = { state: 'idle' }): SetupView {
  return {
    mode: 'desktop',
    first_run: true,
    settings: {
      ui_language: 'en',
      transcription: {
        selection: { provider: 'openai', model: 'gpt-transcribe' }, input_language: 'en',
        output_language: { mode: 'preserve' }, vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' },
      },
    },
    runtime: {
      hardware: { architecture: 'x86_64', cpu_threads: 16, total_ram_bytes: 34_000_000_000, available_ram_bytes: 8 * 2 ** 30, parakeet_cpu_supported: false },
      execution: { requested: 'automatic', cpu_available: true, gpu_available: true, fallback_reason: null },
      worker, problem: null, packaged: true, worker_executable_present: true, vulkan_library_present: true,
      ffmpeg_present: true, external_server: false, models_dir: null,
    },
    models: [
      model('whisper-tiny-q5-1', 'Whisper tiny', 32_000_000, ['en', 'he', 'mixed']),
      model('whisper-large-v3-turbo-q5-0', 'Whisper large-v3-turbo', 574_000_000, ['en', 'he', 'mixed']),
      model('ivrit-turbo', 'ivrit.ai Whisper large-v3-turbo', 1_624_555_275, ['he']),
    ],
    cloud: [
      { provider: 'openai', models: ['gpt-transcribe', 'whisper-1'], credential: 'absent', check: null, verified: false, selected: false },
      { provider: 'gemini', models: ['gemini-3.8-flash'], credential: 'absent', check: null, verified: false, selected: false },
    ],
    ...overrides,
  };
}

interface Routed { current: SetupView; posts: string[]; reject?: boolean }

/** Serves the setup document and applies the same transitions the backend does. */
async function route(page: Page, initial: SetupView = view()): Promise<Routed> {
  const state: Routed = { current: initial, posts: [] };
  await page.route('**/api/v1/**', async (r) => {
    const url = new URL(r.request().url());
    if (url.pathname === '/api/v1/settings' && r.request().method() === 'PUT') {
      const body = r.request().postDataJSON() as { data: SetupView['settings'] };
      const selection = body.data.transcription.selection;
      state.current = {
        ...state.current,
        first_run: false,
        settings: body.data,
        models: state.current.models.map((m) => ({ ...m, selected: selection.provider === 'local' && selection.model === m.id, loaded: selection.provider === 'local' && selection.model === m.id })),
        runtime: selection.provider === 'local'
          ? { ...state.current.runtime, worker: { state: 'ready', model: selection.model, device: 'AMD GPU', backend: 'vulkan', started_at_ms: 1, restarts: 0 } }
          : state.current.runtime,
        cloud: state.current.cloud.map((c) => ({ ...c, selected: c.provider === selection.provider })),
      };
      return r.fulfill({ json: { api_version: '1', data: body.data } });
    }
    if (r.request().method() === 'POST') {
      state.posts.push(url.pathname);
      const download = /\/setup\/models\/([a-z0-9-]+)\/download$/.exec(url.pathname);
      if (download) {
        state.current = { ...state.current, models: state.current.models.map((m) => m.id === download[1] ? { ...m, weights: { state: 'downloading', downloaded_bytes: 200_000_000, total_bytes: m.bytes } } : m) };
      }
      const checked = /\/setup\/cloud\/([a-z]+)\/test$/.exec(url.pathname);
      if (checked) {
        state.current = { ...state.current, cloud: state.current.cloud.map((c) => c.provider === checked[1] ? { ...c, check: { outcome: state.reject ? 'rejected' : 'accepted', checked_at_ms: 1_800_000_000_000 } } : c) };
      }
    }
    return r.fulfill({ json: { api_version: '1', data: state.current } });
  });
  return state;
}

const HEADINGS = [
  'Talk anywhere in Windows. TrickTic Dictate types it.',
  'What will you dictate in?',
  'Choose what does the transcribing',
  'Your dictation shortcut',
  'Now say something',
  'You are set up',
];

/** Advances from screen `from` to screen `target`, asserting each destination so
 *  a stuck gate is reported as the screen it stuck on, not an anonymous timeout. */
async function toStep(page: Page, target: number, from = 0) {
  for (let i = from; i < target; i++) {
    const button = page.getByRole('button', { name: i === 0 ? 'Set up TrickTic Dictate' : 'Continue', exact: true });
    await expect(button).toBeEnabled();
    await button.click();
    await expect(page.getByRole('heading', { name: HEADINGS[i + 1], exact: true })).toBeVisible();
  }
}

test('a local model is suggested for the spoken language, downloaded, and gates the guide until its worker is ready', async ({ page }) => {
  const state = await route(page);
  await page.goto(pagePath);

  await expect(page.getByRole('heading', { name: 'Talk anywhere in Windows. TrickTic Dictate types it.' })).toBeVisible();
  await toStep(page, 2);

  await expect(page.getByRole('heading', { name: 'Choose what does the transcribing' })).toBeVisible();
  const forward = page.getByRole('button', { name: /^Continue/ });
  await expect(forward).toBeDisabled();

  await page.getByRole('button', { name: /Run on this PC/ }).click();
  // Suggested for English on a machine with a Vulkan runtime and 8 GiB free.
  await expect(page.getByRole('heading', { name: 'Whisper large-v3-turbo' })).toBeVisible();
  await expect(page.getByText('GPU (Vulkan) runtime installed')).toBeVisible();
  // Hebrew-only weights are never offered while English is the speech language.
  await expect(page.getByText('ivrit.ai')).toHaveCount(0);

  await page.getByRole('button', { name: /^Download 574/ }).click();
  await expect(page.getByText('Downloading 200.0 MB of 574.0 MB')).toBeVisible();
  await expect(forward).toBeDisabled();

  state.current = { ...state.current, models: state.current.models.map((m) => m.id === 'whisper-large-v3-turbo-q5-0' ? { ...m, weights: installed } : m) };
  await page.getByRole('button', { name: 'Use this model' }).click();

  await expect(page.getByText('whisper-large-v3-turbo-q5-0 is loaded and running on AMD GPU (vulkan).')).toBeVisible();
  await expect(forward).toBeEnabled();
  expect(state.posts).toContain('/api/v1/setup/models/whisper-large-v3-turbo-q5-0/download');
});

test('the Hebrew preset changes which model is suggested', async ({ page }) => {
  await route(page);
  await page.goto(pagePath);
  await toStep(page, 1);
  await page.getByRole('group', { name: 'Speech language' }).getByRole('button', { name: 'עברית · Hebrew', exact: true }).click();
  await toStep(page, 2, 1);
  await page.getByRole('button', { name: /Run on this PC/ }).click();
  await expect(page.getByRole('heading', { name: 'ivrit.ai Whisper large-v3-turbo' })).toBeVisible();
});

test('a cloud key opens the gate without any download, and is never typed into the page', async ({ page }) => {
  const state = await route(page);
  await page.goto(pagePath);
  await toStep(page, 2);
  await page.getByRole('button', { name: /Use a cloud provider/ }).click();

  await expect(page.getByLabel('OpenAI key')).toHaveText(/No key stored\./);
  await expect(page.getByRole('button', { name: 'Use OpenAI' })).toBeDisabled();
  await expect(page.locator('input[type="password"], input[name*="key" i]')).toHaveCount(0);

  await page.getByRole('button', { name: 'Add key…' }).first().click();
  expect(await page.evaluate(() => window.ob.credentialPrompts)).toEqual(['openai']);

  state.current = { ...state.current, cloud: state.current.cloud.map((c) => c.provider === 'openai' ? { ...c, credential: 'protected_store' } : c) };
  await expect(page.getByRole('button', { name: 'Use OpenAI' })).toBeEnabled();
  await page.getByRole('button', { name: 'Use OpenAI' }).click();
  await expect(page.getByRole('button', { name: /^Continue/ })).toBeEnabled();
});

test('a stored key is checked against its provider, automatically and on demand', async ({ page }) => {
  const state = await route(page, view({
    cloud: [
      { provider: 'openai', models: ['gpt-transcribe'], credential: 'absent', check: null, verified: false, selected: false },
      { provider: 'gemini', models: ['gemini-3.8-flash'], credential: 'protected_store', check: null, verified: false, selected: false },
    ],
  }));
  await page.goto(pagePath);
  await toStep(page, 2);
  await page.getByRole('button', { name: /Use a cloud provider/ }).click();

  // The stored key is asked about without being clicked on: after a restart the
  // backend remembers no check, and "I have not looked" is not an answer here.
  const gemini = page.locator('.ob-provider', { has: page.getByRole('button', { name: 'Use Gemini' }) });
  await expect(gemini.locator('.key-pill')).toHaveText('Connected');
  await expect(page.getByLabel('Gemini key')).toHaveText(/Gemini accepted it at .*No transcript has run yet\./);
  expect(state.posts).toContain('/api/v1/setup/cloud/gemini/test');
  // Never for a provider with no key to check.
  expect(state.posts).not.toContain('/api/v1/setup/cloud/openai/test');
  await expect(page.locator('.ob-provider', { has: page.getByRole('button', { name: 'Use OpenAI' }) }).locator('.key-pill')).toHaveText('No key');

  // A rejected key is reported as rejected, and still does not block recording.
  state.reject = true;
  await gemini.getByRole('button', { name: 'Check key' }).click();
  await expect(gemini.locator('.key-pill')).toHaveText('Error');
  await expect(page.getByLabel('Gemini key')).toHaveText(/Gemini rejected it\. Replace the key\./);
});

/** Walks to the practice step with a ready local model. */
async function toPractice(page: Page, path = pagePath) {
  const state = await route(page, view({
    first_run: false,
    settings: {
      ui_language: 'en',
      transcription: {
        selection: { provider: 'local', model: 'whisper-large-v3-turbo-q5-0' }, input_language: 'en',
        output_language: { mode: 'preserve' }, vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' },
      },
    },
  }, { state: 'ready', model: 'whisper-large-v3-turbo-q5-0', device: 'AMD GPU', backend: 'vulkan', started_at_ms: 1, restarts: 0 }));
  state.current = { ...state.current, models: state.current.models.map((m) => m.id === 'whisper-large-v3-turbo-q5-0' ? { ...m, weights: installed, selected: true, loaded: true } : m) };
  await page.goto(path);
  await toStep(page, 4);
  await expect(page.getByRole('heading', { name: 'Now say something' })).toBeVisible();
  return state;
}

test('holding the chord is shown, the spoken text replaces the prompt, and only then does the guide open the workspace', async ({ page }) => {
  await toPractice(page);
  const prompt = page.getByLabel('Sentence to read aloud');
  await expect(prompt).toHaveText('TrickTic Dictate keeps my audio even when the model fails.');
  const forward = page.getByRole('button', { name: /^Continue/ });
  await expect(forward).toBeDisabled();

  const keys = page.locator('.ob-stage .ob-keys').first();
  await expect(keys).not.toHaveClass(/held/);
  await page.evaluate(() => window.ob.hotkey('recording', true));
  await expect(keys).toHaveClass(/held/);
  await expect(page.getByText('Got it — keep talking')).toBeVisible();

  await page.evaluate(() => window.ob.dictate({ state: 'requesting', sessionId: 's1' }));
  await expect(page.getByText('Opening the microphone…')).toBeVisible();
  await page.evaluate(() => window.ob.dictate({ state: 'capturing', levels: [0.2, 0.6, 0.9] }));
  await expect(page.getByText(/Listening/)).toBeVisible();
  await page.evaluate(() => window.ob.hotkey('recording', false));
  await expect(keys).not.toHaveClass(/held/);

  await page.evaluate(() => window.ob.dictate({ state: 'saving' }));
  await expect(page.getByText('Saving the recording…')).toBeVisible();
  await page.evaluate(() => window.ob.dictate({ state: 'transcribing' }));
  await expect(page.locator('.ob-phase')).toContainText('Transcribing with whisper-large-v3-turbo-q5-0 · on this PC');
  await expect(forward).toBeDisabled();

  await page.evaluate(() => window.ob.dictate({ state: 'success', transcript: 'TrickTic Dictate keeps my audio even when the model failed' }));
  await expect(page.getByLabel('What you said')).toHaveText('TrickTic Dictate keeps my audio even when the model failed');
  await expect(page.getByLabel('Sentence to read aloud')).toHaveCount(0);
  await expect(page.getByText('9 of 10 words from the prompt came back', { exact: false })).toBeVisible();
  await expect(page.locator('.ob-word[data-status="missing"]')).toHaveText(['fails']);
  await expect(page.locator('.ob-word[data-status="extra"]')).toHaveText(['failed']);
  await expect(forward).toBeEnabled();
});

test('a failed dictation reports why, keeps the audio and offers an explicit way past', async ({ page }) => {
  await toPractice(page);
  await page.evaluate(() => window.ob.dictate({ state: 'requesting', sessionId: 's2' }));
  await expect(page.getByRole('button', { name: 'Stop now', exact: true })).toBeVisible();
  await expect(page.getByText('Opening the microphone…', { exact: true })).toBeVisible();
  await page.evaluate(() => window.ob.dictate({ state: 'error', problem: 'Session failed (worker_exited).' }));

  await expect(page.getByRole('alert').first()).toContainText('Session failed (worker_exited). Your audio was kept, so nothing was lost.');
  await expect(page.getByRole('button', { name: /^Continue$/ })).toBeDisabled();
  await page.getByRole('button', { name: 'Continue anyway' }).click();
  await expect(page.getByRole('heading', { name: 'You are set up' })).toBeVisible();
});

test('a restored session from before the guide is never presented as a fresh result', async ({ page }) => {
  await toPractice(page);
  await page.evaluate(() => window.ob.dictate({ state: 'success', sessionId: 'old', transcript: 'Something dictated last week' }));
  await expect(page.getByLabel('Sentence to read aloud')).toBeVisible();
  await expect(page.getByText('Something dictated last week')).toHaveCount(0);
  await expect(page.getByRole('button', { name: /^Continue$/ })).toBeDisabled();
});

test('the in-app record button covers a shortcut Windows refused to register', async ({ page }) => {
  await toPractice(page);
  await page.getByRole('button', { name: 'Record without the shortcut' }).click();
  expect(await page.evaluate(() => window.ob.starts)).toBe(1);
  await page.evaluate(() => window.ob.dictate({ state: 'capturing', sessionId: 's3' }));
  await page.getByRole('button', { name: 'Stop now' }).click();
  expect(await page.evaluate(() => window.ob.stops)).toBe(1);
});

test('the last screen recommends launching at login, maps the settings, and completion is recorded once', async ({ page }) => {
  await toPractice(page);
  await page.evaluate(() => window.ob.dictate({ state: 'requesting', sessionId: 's4' }));
  await expect(page.getByRole('button', { name: 'Stop now', exact: true })).toBeVisible();
  await page.evaluate(() => window.ob.dictate({ state: 'success', transcript: 'TrickTic Dictate keeps my audio even when the model fails.' }));
  await page.getByRole('button', { name: /^Continue$/ }).click();

  await expect(page.getByRole('heading', { name: 'You are set up' })).toBeVisible();
  await expect(page.getByText('Recommended')).toBeVisible();
  const toggle = page.getByRole('switch', { name: 'Start TrickTic Dictate when I sign in to Windows' });
  await expect(toggle).toHaveAttribute('aria-checked', 'false');
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-checked', 'true');
  expect(await page.evaluate(() => window.ob.system.launch_at_login)).toBe(true);

  await expect(page.getByText('Settings → Shortcuts')).toBeVisible();
  await expect(page.getByText('Settings → Speech engine')).toBeVisible();
  await expect(page.getByText('Settings → Modes')).toBeVisible();

  await page.getByRole('button', { name: 'Start using TrickTic Dictate' }).click();
  await expect(page.getByRole('heading', { name: 'Workspace' })).toBeVisible();
  expect(await page.evaluate(() => window.ob.completed)).toBe(true);

  await page.getByRole('button', { name: 'Run setup guide' }).click();
  await expect(page.getByRole('heading', { name: 'Talk anywhere in Windows. TrickTic Dictate types it.' })).toBeVisible();
  expect(await page.evaluate(() => window.ob.completed)).toBe(false);
});

test('the rail walks back to a finished screen and forward again, but never past the gate', async ({ page }) => {
  await toPractice(page);
  const rail = page.getByRole('navigation', { name: 'Setup progress' });

  // Everything reached is a real button; the screen the gate has not opened yet is not.
  await expect(rail.getByRole('button')).toHaveCount(5);
  await expect(rail.getByRole('button', { name: /Ready/ })).toHaveCount(0);

  await rail.getByRole('button', { name: /voice/i }).click();
  await expect(page.getByRole('heading', { name: 'What will you dictate in?' })).toBeVisible();

  // Stepping back to change an answer does not cost the screens already finished.
  await rail.getByRole('button', { name: /Try it/ }).click();
  await expect(page.getByRole('heading', { name: 'Now say something' })).toBeVisible();
});

test('the shortcut screen edits the real chord and never starts a recording', async ({ page }) => {
  await toPractice(page);
  await page.getByRole('button', { name: 'Back' }).click();
  await expect(page.getByRole('heading', { name: 'Your dictation shortcut' })).toBeVisible();
  await expect(page.getByLabel('Ctrl+Shift+F9')).toBeVisible();
  await expect(page.getByLabel('Ctrl+Shift+F10')).toBeVisible();

  await page.getByRole('group', { name: 'Shortcut mode' }).getByRole('button', { name: 'Press to start and stop' }).click();
  expect(await page.evaluate(() => window.ob.preferences.toggle)).toBe(true);

  await page.getByRole('button', { name: 'Change…' }).first().click();
  await page.keyboard.down('Control');
  await page.keyboard.down('Alt');
  await page.keyboard.press('KeyR');
  await page.keyboard.up('Alt');
  await page.keyboard.up('Control');
  expect(await page.evaluate(() => window.ob.preferences.recording)).toBe('Ctrl+Alt+R');
  expect(await page.evaluate(() => window.ob.starts)).toBe(0);
});

test('a dictation that cannot be transcribed keeps saying why and never opens the gate', async ({ page }) => {
  await toPractice(page);
  await page.evaluate(() => window.ob.dictate({
    state: 'requesting', sessionId: 's5',
    problem: 'The local model is not ready. Settings → Transcription shows the actual model and runtime state.',
  }));
  await expect(page.getByRole('button', { name: 'Stop now', exact: true })).toBeVisible();
  await page.evaluate(() => window.ob.dictate({ state: 'saved' }));

  await expect(page.locator('.ob-phase')).toContainText('Audio saved. Handing it to whisper-large-v3-turbo-q5-0');
  await expect(page.getByRole('alert').first()).toContainText('The local model is not ready.');
  await expect(page.getByRole('button', { name: 'Continue', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Continue anyway' })).toHaveCount(0);
});

test('Mac onboarding welcomes dictation without promising automatic insertion', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'MacIntel', configurable: true }));
  await route(page);
  await page.goto(pagePath);
  await expect(page.getByRole('heading', { name: 'Dictate on your Mac. Keep the text.' })).toBeVisible();
  await expect(page.getByText(/Talk anywhere in Windows/)).toHaveCount(0);
});

test('Mac Accessibility step is optional and fn shortcut guidance remains available', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'MacIntel' }));
  const initial = view({ first_run: false });
  initial.cloud[0] = { ...initial.cloud[0]!, credential: 'protected_store', verified: true, check: { outcome: 'accepted', checked_at_ms: 1_800_000_000_000 } };
  await route(page, initial);
  await page.goto(pagePath);
  await toStep(page, 2);
  await page.getByRole('button', { name: 'Continue', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Enable dictation and paste', exact: true })).toBeVisible();
  await expect(page.getByText(/You can skip this step/)).toBeVisible();
  await page.getByRole('button', { name: 'Continue', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Your dictation shortcut', exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.ob.starts)).toBe(0);
});

test('a build with sign-in offers Google once at the end, and continuing without an account finishes setup', async ({ page }) => {
  // Served from the desktop origin with a simulated shell that offers sign-in.
  await page.route('http://tauri.localhost/**', async (r) => {
    const response = await r.fetch({ url: r.request().url().replace('http://tauri.localhost', 'http://127.0.0.1:15193') });
    await r.fulfill({ response });
  });
  await page.addInitScript(() => {
    const calls: string[] = [];
    Object.assign(window, { accountCalls: calls, __TAURI__: {
      core: { invoke: async (command: string) => {
        calls.push(command);
        if (command === 'account_snapshot') return { state: 'guest', availability: 'ready', generation: 1, reauthenticating: false };
        // Everything else behaves as it does without a shell.
        throw new Error('Native host unavailable.');
      } },
      event: { listen: async () => () => {} },
    } });
  });
  await toPractice(page, `http://tauri.localhost${pagePath}`);
  await page.evaluate(() => window.ob.dictate({ state: 'requesting', sessionId: 's5' }));
  await expect(page.getByRole('button', { name: 'Stop now', exact: true })).toBeVisible();
  await page.evaluate(() => window.ob.dictate({ state: 'success', transcript: 'TrickTic Dictate keeps my audio even when the model fails.' }));
  await page.getByRole('button', { name: /^Continue$/ }).click();
  await expect(page.getByRole('heading', { name: 'You are set up' })).toBeVisible();
  await expect(page.getByText('works fully without an account', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sign in with Google' })).toBeVisible();
  await page.getByRole('button', { name: 'Continue without an account' }).click();
  expect(await page.evaluate(() => window.ob.completed)).toBe(true);
  expect(await page.evaluate(() => (window as unknown as { accountCalls: string[] }).accountCalls)).not.toContain('account_sign_in');
});

test('a public build shows no account choice at the end of setup', async ({ page }) => {
  await toPractice(page);
  await page.evaluate(() => window.ob.dictate({ state: 'requesting', sessionId: 's6' }));
  await expect(page.getByRole('button', { name: 'Stop now', exact: true })).toBeVisible();
  await page.evaluate(() => window.ob.dictate({ state: 'success', transcript: 'TrickTic Dictate keeps my audio even when the model fails.' }));
  await page.getByRole('button', { name: /^Continue$/ }).click();
  await expect(page.getByRole('heading', { name: 'You are set up' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Continue without an account' })).toHaveCount(0);
});

test('each step opens at its top with focus on its heading, and Ready shows the first dictation', async ({ page }) => {
  await toPractice(page);
  // The heading of the step just opened has focus, so its name is announced.
  await expect(page.getByRole('heading', { name: 'Now say something' })).toBeFocused();
  await page.evaluate(() => window.ob.dictate({ state: 'requesting', sessionId: 's9' }));
  await expect(page.getByRole('button', { name: 'Stop now', exact: true })).toBeVisible();
  await page.evaluate(() => window.ob.dictate({ state: 'capturing' }));
  await page.evaluate(() => window.ob.dictate({ state: 'transcribing' }));
  await page.waitForTimeout(150);
  await page.evaluate(() => window.ob.dictate({ state: 'success', transcript: 'My first words, typed for me.' }));
  await page.getByRole('button', { name: /^Continue$/ }).click();
  await expect(page.getByRole('heading', { name: 'You are set up' })).toBeFocused();
  await expect(page.locator('.ob-first')).toContainText('My first words, typed for me.');
  await expect(page.locator('.ob-first')).toContainText(/came back [\d.]+ s after you let go/);
  expect(await page.locator('.ob-body').evaluate((element) => element.scrollTop)).toBe(0);
});
