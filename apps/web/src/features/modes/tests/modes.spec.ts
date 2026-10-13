import { stubOwnerSession, loginFixture } from '../../auth/tests/fixture';
import { expect, test, type Page } from '@playwright/test';
import type { ModeProfile, ModeSettings, ProcessRequest, ProcessingAttempt } from '../api';
function settings(): ModeSettings {
  return { revision: 1, active: 'regular', profiles: [
    { id: 'regular', name: 'Regular', revision: 1, kind: 'regular', instructions: '', text_model: null, context: { application: false, selected_text: false, clipboard: false } },
    { id: 'super', name: 'Super', revision: 1, kind: 'super', instructions: 'Format paragraphs', text_model: { provider: 'openai', model: 'gpt-4.1-mini' }, context: { application: false, selected_text: false, clipboard: false } },
    { id: 'command', name: 'Command', revision: 1, kind: 'command', instructions: 'Apply instruction to selected text', text_model: { provider: 'openai', model: 'gpt-4.1-mini' }, context: { application: false, selected_text: true, clipboard: false } },
  ] };
}
async function routes(page: Page, drop = false, extra: object[] = []) {
  const state = { settings: settings(), requests: [] as ProcessRequest[], attempts: [] as ProcessingAttempt[], saves: 0, custom: [] as { provider: string; model: string; fast: string; balanced: string; only_hosts: string[]; ignore_hosts: string[] }[], customSaves: 0, hostReads: [] as string[], previews: [] as { job: string; instructions: string }[] };
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/v1/modes/providers') return route.fulfill({ json: { api_version: '1', data: [...['gpt-4.1-mini', 'gpt-5.6-luna'].map(model => ({ selection: { provider: 'openai', model }, configured: true, execution_location: 'cloud' })), ...extra, ...state.custom.map(({ provider, model, fast, balanced }) => ({ selection: { provider, model }, configured: provider === 'openai', execution_location: 'cloud', custom: { fast, balanced } }))] } });
    if (path === '/api/v1/modes/custom-models/hosts') {
      expect(route.request().method()).toBe('POST');
      state.hostReads.push(route.request().postDataJSON().data.model);
      return route.fulfill({ json: { api_version: '1', data: [
        { tag: 'deepinfra/fp8', name: 'DeepInfra', uptime: 99.9, quantization: 'fp8' },
        { tag: 'relace/fp4', name: 'Relace', uptime: 97.5, quantization: 'fp4' },
      ] } });
    }
    if (path === '/api/v1/modes/custom-models') {
      if (route.request().method() === 'PUT') {
        const next = route.request().postDataJSON().data as typeof state.custom;
        if (next.some((m, i) => next.findIndex(o => o.provider === m.provider && o.model === m.model) !== i)) return route.fulfill({ status: 400, json: { api_version: '1', error: { code: 'invalid_request', message: 'That custom model is already added.', retryable: false } } });
        state.custom = next; state.customSaves++;
      }
      return route.fulfill({ json: { api_version: '1', data: state.custom } });
    }
    if (path === '/api/v1/modes') {
      if (route.request().method() === 'PUT') { state.saves++; state.settings = route.request().postDataJSON().data; state.settings.revision++; }
      return route.fulfill({ json: { api_version: '1', data: state.settings } });
    }
    if (path === '/api/v1/modes/preview') {
      const body = route.request().postDataJSON().data as { profile: ModeProfile; job_id?: string };
      state.previews.push({ job: body.job_id ?? '', instructions: body.profile.instructions });
      // The shorter prompt keeps the question; the factory one drops it.
      const input = `Ship it tomorrow please. Does that work for you?`;
      const keeps = body.profile.instructions.includes('every question');
      return route.fulfill({ json: { api_version: '1', data: { text: keeps ? input : 'Ship it tomorrow, please.', input_text: input, diagnostics: { processing_ms: keeps ? 900 : 700, reused_draft: false } } } });
    }
    if (path.endsWith('/processing')) {
      if (route.request().method() === 'POST') {
        const req: ProcessRequest = route.request().postDataJSON().data;
        state.requests.push(req);
        if (drop && state.requests.length === 1) return route.abort('failed');
        const attempt: ProcessingAttempt = { id: req.operation_id, job_id: '11111111-1111-4111-8111-111111111111', created_at_ms: 1, profile: state.settings.profiles.find(p => p.id === req.profile_id)!, request: req, raw_text: 'שלום hello', input_text: 'שלום hello', prompt_version: 1, state: { state: 'succeeded', text: req.profile_id === 'command' ? 'Updated target' : 'שלום hello.' } };
        state.attempts.push(attempt);
        return route.fulfill({ json: { api_version: '1', data: attempt } });
      }
      return route.fulfill({ json: { api_version: '1', data: state.attempts } });
    }
    return route.fulfill({ status: 404, json: {} });
  });
  await stubOwnerSession(page);
  await page.goto('/src/features/modes/tests/panel.html');
  return state;
}
test('mode shortcuts use key capture and are saved with the profile', async ({ page }) => {
  const state = await routes(page);
  await page.getByText('Advanced settings', { exact: false }).click();
  await page.getByRole('button', { name: 'Set mode shortcut…' }).click();
  await expect(page.getByRole('dialog', { name: 'Set the mode shortcut' })).toBeVisible();
  await page.keyboard.press('Control+Alt+1');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.getByRole('button', { name: 'Save changes' }).click();
  await expect.poll(() => state.settings.profiles[0].shortcut).toBe('Ctrl+Alt+1');
});
test('settings separate text model and context; editing requires save and duplicate preserves original', async ({ page }) => {
  const state = await routes(page);
  const settings = page.getByRole('region', { name: 'Dictation modes' });
  await settings.getByLabel('Default recording mode', { exact: true }).selectOption('super');
  await settings.getByText('Advanced settings', { exact: false }).click();
  await settings.getByLabel('Instructions').fill('Format as clear paragraphs; preserve Hebrew and English.');
  await settings.getByLabel('Clipboard text').check();
  expect(state.saves).toBe(0);
  await settings.getByRole('button', { name: 'Save changes' }).click();
  await expect(settings.getByRole('status')).toContainText('Saved. Super is your default recording mode.');
  expect(state.settings.profiles[1].context.clipboard).toBe(true);
  await settings.getByText('Manage profiles', { exact: false }).click();
  await settings.getByRole('button', { name: 'Duplicate mode' }).click();
  await expect(settings.getByLabel('Name', { exact: true })).toHaveValue('Super copy');
  expect(state.settings.profiles).toHaveLength(3);
});
test('command requires a target; results and originals remain inspectable without requests on reload', async ({ page }) => {
  const state = await routes(page);
  await page.getByText('Process with a mode', { exact: true }).click();
  await page.getByLabel('Processing mode').selectOption('command');
  await expect(page.getByRole('button', { name: 'Process saved transcript' })).toBeDisabled();
  await page.getByLabel('Selected text to transform').fill('Old target');
  await page.getByRole('button', { name: 'Process saved transcript' }).click();
  await expect(page.getByText('Updated target', { exact: true })).toBeVisible();
  expect(state.requests[0].context).toEqual({ application: '', clipboard: '', selected_text: 'Old target' });
  await page.reload();
  expect(state.requests).toHaveLength(1);
  await page.getByText('Process with a mode', { exact: true }).click();
  await expect(page.getByText('Updated target', { exact: true })).toBeVisible();
  await page.getByText('Processing details', { exact: true }).click();
  await expect(page.getByText('שלום hello', { exact: true })).toBeVisible();
});
test('uncertain processing submission retries the exact operation ID', async ({ page }) => {
  const state = await routes(page, true);
  await page.getByText('Process with a mode', { exact: true }).click();
  await page.getByLabel('Processing mode').selectOption('super');
  await page.getByRole('button', { name: 'Process saved transcript' }).click();
  await expect(page.getByRole('alert')).toBeVisible();
  await page.getByRole('button', { name: 'Retry same request' }).click();
  await expect(page.getByText('שלום hello.', { exact: true })).toBeVisible();
  expect(state.requests).toHaveLength(2);
  expect(state.requests[0]).toEqual(state.requests[1]);
});

test('modes fit the settings window and malformed imports do not replace the current profile', async ({ page }) => {
  await routes(page);
  await page.goto('/?settings=modes'); await loginFixture(page);
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel('Default recording mode', { exact: true }).selectOption('super');
  for (const viewport of [{ width: 1024, height: 700 }, { width: 1280, height: 800 }]) {
    await page.setViewportSize(viewport);
    expect(await page.evaluate(() => document.documentElement.scrollHeight - document.documentElement.clientHeight)).toBe(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBe(0);
  }
  await dialog.getByText('Manage profiles', { exact: false }).click();
  await dialog.getByLabel('Import modes').setInputFiles({ name: 'bad.json', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify({ api_version: '1', data: { profiles: [{}] } })) });
  await expect(dialog.getByRole('alert')).toContainText('invalid or excessive');
  await dialog.getByText('Advanced settings', { exact: false }).click();
  await expect(dialog.getByLabel('Name', { exact: true })).toHaveValue('Super');
});

test('default selection is explicit and save/discard remain visible without scrolling', async ({ page }) => {
  const state = await routes(page);
  await page.goto('/?settings=modes'); await loginFixture(page);
  await page.setViewportSize({ width: 1024, height: 700 });
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByLabel('Default recording mode').selectOption('super');
  await expect(dialog.getByText('Will switch to Super when you save.')).toBeVisible();
  const save = dialog.getByRole('button', { name: 'Save changes' });
  await expect(save).toBeInViewport();
  await expect(dialog.getByLabel('Text-processing model')).toBeInViewport();
  await dialog.getByText('Advanced settings', { exact: false }).click();
  await dialog.getByLabel('Instructions').fill('My new prompt');
  await expect(save).toBeInViewport();
  await dialog.getByRole('button', { name: 'Discard changes' }).click();
  await expect(dialog.getByLabel('Instructions')).toHaveValue('Format paragraphs');
  await expect(dialog.getByLabel('Default recording mode')).toHaveValue('regular');
  expect(state.saves).toBe(0);
});

test('draft survives model setup and closing settings requires a deliberate discard', async ({ page }) => {
  await routes(page);
  await page.goto('/?settings=modes'); await loginFixture(page);
  const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  await dialog.getByLabel('Default recording mode').selectOption('super');
  await dialog.getByRole('button', { name: 'Manage models' }).click();
  await page.getByRole('button', { name: 'Return to Modes' }).click();
  await expect(dialog.getByLabel('Default recording mode')).toHaveValue('super');
  await dialog.getByRole('button', { name: 'Close settings' }).click();
  const guard = page.getByRole('dialog', { name: 'Unsaved mode changes', exact: true });
  await expect(guard).toBeVisible();
  await guard.getByRole('button', { name: 'Keep editing' }).click();
  await expect(dialog.getByLabel('Default recording mode')).toHaveValue('super');
  await dialog.getByRole('button', { name: 'Save changes' }).click();
  await dialog.getByRole('button', { name: 'Close settings' }).click();
  await expect(dialog).toHaveCount(0);
});

test('preview uses unsaved formatting without saving or activating and shows stale results', async ({ page }) => {
  const state = await routes(page);
  let body: { profile: { formatting: { rewrite: string } } } | undefined;
  await page.route('**/api/v1/modes/preview', async r => {
    body = r.request().postDataJSON().data;
    await r.fulfill({ json: { api_version: '1', data: { input_text: 'buy apples milk', text: 'Buy:\n- Apples\n- Milk', diagnostics: { processing_ms: 250, reused_draft: false } } } });
  });
  const panel = page.getByRole('region', { name: 'Dictation modes' });
  await panel.getByText('Manage profiles', { exact: false }).click();
  await panel.getByLabel('Editing profile').selectOption('super');
  await panel.getByRole('button', { name: 'Concise', exact: true }).click();
  await panel.getByText('Try this mode', { exact: false }).click();
  await panel.getByRole('button', { name: 'Preview result' }).click();
  await expect(panel.getByText('Result · 0.25 s')).toBeVisible();
  expect(body?.profile.formatting.rewrite).toBe('concise');
  expect(state.saves).toBe(0);
  expect(state.settings.active).toBe('regular');
  await panel.getByRole('button', { name: 'Polished', exact: true }).click();
  await expect(panel.getByText('Settings or sample changed.', { exact: false })).toBeVisible();
});

test('failed save keeps draft and successful retry clears the save bar', async ({ page }) => {
  await routes(page);
  let fail = true;
  await page.route('**/api/v1/modes', async r => {
    if (r.request().method() !== 'PUT') return r.fallback();
    if (fail) { fail = false; return r.fulfill({ status: 503, json: { api_version: '1', error: { message: 'Storage unavailable' } } }); }
    return r.fulfill({ json: { api_version: '1', data: r.request().postDataJSON().data } });
  });
  const panel = page.getByRole('region', { name: 'Dictation modes' });
  await panel.getByLabel('Default recording mode').selectOption('super');
  await panel.getByRole('button', { name: 'Save changes' }).click();
  await expect(panel.getByRole('alert')).toContainText('Storage unavailable');
  await expect(panel.getByLabel('Default recording mode')).toHaveValue('super');
  await panel.getByRole('button', { name: 'Save changes' }).click();
  await expect(panel.getByRole('region', { name: 'Unsaved mode changes' })).toHaveCount(0);
});


test('Mac modes save each opt-in context choice independently', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'platform', { value: 'MacIntel' }));
  const state = await routes(page);
  const section = page.getByRole('region', { name: 'Dictation modes' });
  await section.getByLabel('Default recording mode', { exact: true }).selectOption('super');
  await section.getByText('Advanced settings', { exact: false }).click();
  await section.getByLabel('Application context', { exact: true }).check();
  await section.getByLabel('Selected text', { exact: true }).check();
  await expect(section.getByLabel('Clipboard text', { exact: true })).not.toBeChecked();
  await section.getByRole('button', { name: 'Save changes' }).click();
  await expect.poll(() => state.settings.profiles[1].context).toEqual({ application: true, selected_text: true, clipboard: false });
});


test('Luna can be selected and saved with its explicit speed setting', async ({ page }) => {
  const state = await routes(page);
  const panel = page.getByRole('region', { name: 'Dictation modes' });
  await panel.getByLabel('Default recording mode', { exact: true }).selectOption('super');
  await panel.getByLabel('Text-processing model').selectOption('openai/gpt-5.6-luna');
  await panel.getByText('Advanced settings', { exact: false }).click();
  await expect(panel).toContainText('Luna, Grok 4.3 and TrickTic Dictate’s OpenRouter models disable reasoning for Fast');
  await panel.getByRole('button', { name: 'Save changes', exact: true }).click();
  await expect.poll(() => state.settings.profiles[1].text_model?.model).toBe('gpt-5.6-luna');
  expect(state.settings.profiles[1].fast_processing).toBe(true);
  await page.reload();
  await expect(panel.getByLabel('Text-processing model')).toHaveValue('openai/gpt-5.6-luna');
});

test('a custom model is chosen like any other and says what each speed sends', async ({ page }) => {
  const state = await routes(page, false, [
    { selection: { provider: 'openrouter', model: 'meta-llama/llama-4-maverick' }, configured: true, execution_location: 'cloud', custom: { fast: 'off', balanced: 'provider_default' } },
  ]);
  const panel = page.getByRole('region', { name: 'Dictation modes' });
  await panel.getByLabel('Default recording mode', { exact: true }).selectOption('super');
  const select = panel.getByLabel('Text-processing model');
  await expect(select.locator('option', { hasText: 'meta-llama/llama-4-maverick' })).toHaveText('meta-llama/llama-4-maverick · OpenRouter · custom');
  await select.selectOption('openrouter/meta-llama/llama-4-maverick');
  await expect(panel).toContainText('Cloud processing · Key saved');
  await panel.getByText('Advanced settings', { exact: false }).click();
  await expect(panel).toContainText('meta-llama/llama-4-maverick is a custom model: Fast sends reasoning off and Balanced sends no reasoning setting. Change this in Manage custom models.');
  await panel.getByRole('button', { name: 'Save changes', exact: true }).click();
  // The slash inside an OpenRouter ID stays part of the model.
  await expect.poll(() => state.settings.profiles[1].text_model).toEqual({ provider: 'openrouter', model: 'meta-llama/llama-4-maverick' });
});

test('custom models are added and managed in a popup from the mode’s text model', async ({ page }) => {
  const state = await routes(page);
  const panel = page.getByRole('region', { name: 'Dictation modes' });
  await panel.getByLabel('Default recording mode', { exact: true }).selectOption('super');
  const opener = panel.getByRole('button', { name: 'Manage custom models' });
  await opener.click();
  const dialog = page.getByRole('dialog', { name: 'Custom text models' });
  await expect(dialog).toContainText('No custom models yet.');
  const form = dialog.getByRole('form', { name: 'Add a custom model' });
  // The form starts on the provider of the mode's current model, at a setting every model accepts.
  await expect(form.getByLabel('Provider')).toHaveValue('openai');
  await expect(form.getByLabel('Reasoning when Fast')).toHaveValue('provider_default');
  await expect(form.getByRole('button', { name: 'Add model' })).toBeDisabled();

  await form.getByLabel('Provider').selectOption('openrouter');
  await expect(form.getByLabel('Model ID')).toHaveAttribute('placeholder', 'publisher/model');
  await expect(form.getByRole('status')).toContainText('No OpenRouter key is stored yet');
  await form.getByLabel('Model ID').fill(' meta-llama/llama-4-maverick ');
  await form.getByLabel('Reasoning when Fast').selectOption('off');
  await form.getByRole('button', { name: 'Add model' }).click();
  await expect.poll(() => state.custom).toEqual([{ provider: 'openrouter', model: 'meta-llama/llama-4-maverick', fast: 'off', balanced: 'provider_default', only_hosts: [], ignore_hosts: [] }]);
  await expect(dialog.getByRole('status').filter({ hasText: 'meta-llama/llama-4-maverick added.' })).toBeVisible();
  const maverick = dialog.getByRole('listitem', { name: 'meta-llama/llama-4-maverick' });
  await expect(maverick).toContainText('OpenRouter · Fast: Off · Balanced: Provider default · key needed');
  // The form is ready for the next one and keeps the provider.
  await expect(form.getByLabel('Model ID')).toHaveValue('');
  await expect(form.getByLabel('Provider')).toHaveValue('openrouter');

  await form.getByLabel('Provider').selectOption('openai');
  await form.getByLabel('Model ID').fill('gpt-5.6-terra');
  await form.getByRole('button', { name: 'Add model' }).click();
  await expect(dialog.getByRole('listitem', { name: 'gpt-5.6-terra' })).toContainText('OpenAI · Fast: Provider default · Balanced: Provider default');
  await expect(dialog.getByRole('listitem', { name: 'gpt-5.6-terra' })).not.toContainText('key needed');

  // A refused save is explained and changes nothing.
  await form.getByLabel('Model ID').fill('gpt-5.6-terra');
  await form.getByRole('button', { name: 'Add model' }).click();
  await expect(dialog.getByRole('alert')).toHaveText('That custom model is already added.');
  expect(state.customSaves).toBe(2);

  await dialog.getByRole('listitem', { name: 'gpt-5.6-terra' }).getByRole('button', { name: 'Edit' }).click();
  const edit = dialog.getByRole('form', { name: 'Edit gpt-5.6-terra' });
  await edit.getByLabel('Reasoning when Balanced').selectOption('medium');
  await edit.getByRole('button', { name: 'Save model' }).click();
  await expect(dialog.getByRole('listitem', { name: 'gpt-5.6-terra' })).toContainText('Balanced: Medium');
  expect(state.custom).toHaveLength(2);

  await maverick.getByRole('button', { name: 'Remove' }).click();
  await expect(dialog.getByRole('group', { name: 'Confirm removing meta-llama/llama-4-maverick' })).toContainText('Modes that use it keep it selected');
  await dialog.getByRole('button', { name: 'Remove model' }).click();
  await expect.poll(() => state.custom.map(m => m.model)).toEqual(['gpt-5.6-terra']);

  await dialog.getByRole('button', { name: 'Done' }).click();
  await expect(dialog).toHaveCount(0);
  await expect(opener).toBeFocused();
  // The mode's list has the new model straight away, not at the next poll.
  const select = panel.getByLabel('Text-processing model');
  await expect(select.locator('option', { hasText: 'gpt-5.6-terra' })).toHaveText('gpt-5.6-terra · cloud · custom');
  await select.selectOption('openai/gpt-5.6-terra');
  await panel.getByRole('button', { name: 'Save changes', exact: true }).click();
  await expect.poll(() => state.settings.profiles[1].text_model).toEqual({ provider: 'openai', model: 'gpt-5.6-terra' });
  // Adding a model never changed the mode by itself.
  expect(state.saves).toBe(1);
});

test('a mode takes a fallback model and its own wait, and only cloud models are offered', async ({ page }) => {
  const state = await routes(page, false, [
    { selection: { provider: 'local', model: 'qwen3-4b-instruct-2507-q4' }, configured: true, execution_location: 'self_hosted_server' },
  ]);
  const panel = page.getByRole('region', { name: 'Dictation modes' });
  await panel.getByLabel('Default recording mode', { exact: true }).selectOption('super');
  // Nothing is waiting for a fallback until one is chosen.
  await expect(panel.getByLabel('Wait before the fallback')).toHaveCount(0);
  const fallback = panel.getByLabel('Fallback model');
  await expect(fallback.locator('option')).toHaveText(['No fallback', 'gpt-4.1-mini · cloud', 'gpt-5.6-luna · cloud']);
  await fallback.selectOption('openai/gpt-5.6-luna');
  await expect(panel).toContainText('It starts immediately if the text model fails');
  await expect(panel).toContainText('Only then are both requests charged.');
  await expect(panel).toContainText('The first valid answer is used.');
  const wait = panel.getByLabel('Wait before the fallback');
  await expect(wait).toHaveValue('1.5');
  await wait.fill('0.75');
  await panel.getByRole('button', { name: 'Save changes', exact: true }).click();
  await expect.poll(() => state.settings.profiles[1].fallback_model).toEqual({ provider: 'openai', model: 'gpt-5.6-luna' });
  expect(state.settings.profiles[1].fallback_after_ms).toBe(750);

  await fallback.selectOption('');
  await expect(panel.getByLabel('Wait before the fallback')).toHaveCount(0);
  await panel.getByRole('button', { name: 'Save changes', exact: true }).click();
  await expect.poll(() => state.settings.profiles[1].fallback_model).toBe(null);
});

test('an OpenRouter model can pin or avoid hosts, read from OpenRouter when asked for', async ({ page }) => {
  const state = await routes(page);
  const panel = page.getByRole('region', { name: 'Dictation modes' });
  await panel.getByLabel('Default recording mode', { exact: true }).selectOption('super');
  await panel.getByRole('button', { name: 'Manage custom models' }).click();
  const dialog = page.getByRole('dialog', { name: 'Custom text models' });
  const form = dialog.getByRole('form', { name: 'Add a custom model' });
  await form.getByLabel('Provider').selectOption('openai');
  // Only OpenRouter serves one model from several hosts.
  await expect(dialog.locator('details.custom-models-hosts')).toHaveCount(0);
  await form.getByLabel('Provider').selectOption('openrouter');
  await form.getByLabel('Model ID').fill('deepseek/deepseek-v4.1-flash');
  // The list is read only when the section is opened.
  expect(state.hostReads).toEqual([]);
  await dialog.locator('details.custom-models-hosts summary').click();
  await dialog.getByLabel('Which hosts may serve it').selectOption('avoid');
  await expect.poll(() => state.hostReads).toEqual(['deepseek/deepseek-v4.1-flash']);
  const hosts = dialog.getByRole('group', { name: 'Hosts for deepseek/deepseek-v4.1-flash' });
  await expect(hosts).toContainText('Relace relace/fp4 · fp4 · 97.5% up');
  await hosts.getByRole('checkbox', { name: /Relace/ }).check();
  await form.getByRole('button', { name: 'Add model' }).click();
  await expect.poll(() => state.custom).toEqual([{ provider: 'openrouter', model: 'deepseek/deepseek-v4.1-flash', fast: 'provider_default', balanced: 'provider_default', only_hosts: [], ignore_hosts: ['relace/fp4'] }]);
  await expect(dialog.getByRole('listitem', { name: 'deepseek/deepseek-v4.1-flash' })).toContainText('avoiding relace/fp4');
});

test('prompt versions are compared on saved dictations before one is adopted', async ({ page }) => {
  const state = await routes(page);
  // Two saved dictations of this person's own, which is what a prompt is judged on.
  await page.addInitScript(() => {
    const options = { selection: { provider: 'local', model: 'whisper' }, input_language: 'en', output_language: { mode: 'preserve' }, vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' } };
    const attempt = (id: string, text: string) => ({ id, request: { recording_id: id, idempotency_key: id, options }, job: { id, created_at_ms: 1_800_000_000_000, updated_at_ms: 1_800_000_000_100, request: { recording_id: id, idempotency_key: id, options }, status: { state: 'succeeded', result: { transcript: { raw_text: text, selection: options.selection, input_language: 'en', output_language: { mode: 'preserve' }, execution_location: 'self_hosted_server' }, cleanup: { state: 'off' } } } } });
    const open = indexedDB.open('metrix-writer-transcription', 1);
    open.onupgradeneeded = () => open.result.createObjectStore('attempts', { keyPath: 'id' });
    open.onsuccess = () => {
      const store = open.result.transaction('attempts', 'readwrite').objectStore('attempts');
      store.put(attempt('job-1', 'Ship it tomorrow please. Does that work for you?'));
      store.put(attempt('job-2', 'Second one please.'));
    };
  });
  await page.goto('/src/features/modes/tests/panel.html');
  const panel = page.getByRole('region', { name: 'Dictation modes' });
  await panel.getByLabel('Default recording mode', { exact: true }).selectOption('super');
  await panel.getByText('Advanced settings', { exact: false }).click();
  await panel.getByLabel('Instructions').fill('Format the dictation and keep every question the speaker asked.');
  await panel.getByRole('button', { name: /Compare prompt versions/ }).click();

  const dialog = page.getByRole('dialog', { name: 'Compare prompt versions' });
  await dialog.getByRole('textbox', { name: 'Version name' }).fill('Keeps every question');
  await dialog.getByRole('button', { name: 'Save version' }).click();
  await expect(dialog.getByRole('checkbox', { name: /Keeps every question/ })).toBeChecked();
  await panel.getByLabel('Instructions').fill('Format the dictation.');
  await dialog.getByLabel('Dictations to test').selectOption('3');
  await dialog.getByRole('button', { name: 'Run comparison' }).click();

  const rows = dialog.locator('.compare-results tbody tr');
  await expect(rows).toHaveCount(2);
  // Each version ran against each saved dictation, nothing invented.
  await expect.poll(() => state.previews.map(p => p.job).sort()).toEqual(['job-1', 'job-1', 'job-2', 'job-2']);
  await expect(rows.nth(0)).toContainText('Current draft');
  await expect(rows.nth(0)).toContainText('0.70 s');
  await expect(rows.nth(1)).toContainText('Keeps every question');
  await expect(rows.nth(1)).toContainText('0.90 s');
  // The draft's wording loses the question in both dictations; the saved version does not.
  await expect(rows.nth(0).locator('td').nth(3)).toHaveText('2');
  await expect(rows.nth(1).locator('td').nth(3)).toHaveText('0');

  await rows.nth(1).getByRole('button', { name: 'Use this version' }).click();
  await expect(dialog).toHaveCount(0);
  await expect(panel.getByLabel('Instructions')).toHaveValue('Format the dictation and keep every question the speaker asked.');
  await panel.getByRole('button', { name: 'Save changes', exact: true }).click();
  await expect.poll(() => state.settings.profiles[1].instructions).toBe('Format the dictation and keep every question the speaker asked.');
  expect(state.settings.profiles[1].prompt_versions).toHaveLength(1);
});

test('context opt-in explains persistence and destinations before saving, including default Command', async ({ page }) => {
  const state = await routes(page, false, [
    { selection: { provider: 'local', model: 'local-text' }, configured: true, execution_location: 'self_hosted' },
    { selection: { provider: 'openrouter', model: 'openai/gpt-5.4' }, configured: true, execution_location: 'cloud' },
  ]);
  const modes = page.getByRole('region', { name: 'Dictation modes' });
  await modes.getByLabel('Default recording mode', { exact: true }).selectOption('command');
  const consent = modes.getByTestId('context-consent');
  await expect(consent).toBeVisible();
  await expect(consent).toContainText('before the microphone starts');
  await expect(consent).toContainText('openai cloud text processing');
  await expect(consent).toContainText('failed or interrupted attempts keep it for retry');
  await expect(consent).toContainText('Turning context off affects future captures');
  await expect(consent).toContainText('Delete audio removes only audio');
  await modes.getByLabel('Text-processing model').selectOption('local/local-text');
  await expect(consent).toContainText('your configured local text server');
  await expect(consent).not.toContainText('openai cloud');
  await modes.getByLabel('Text-processing model').selectOption('openrouter/openai/gpt-5.4');
  await expect(consent).toContainText('OpenRouter and the provider serving your selected model');
  expect(state.saves).toBe(0);
});

test('context survives uncertain submission but clears from entry fields once accepted', async ({ page }) => {
  const state = await routes(page, true);
  await page.getByText('Process with a mode', { exact: true }).click();
  await page.getByLabel('Processing mode').selectOption('command');
  const selection = page.getByLabel('Selected text to transform');
  await selection.fill('SYNTHETIC-UI-CONTEXT-SEC14');
  await page.getByRole('button', { name: 'Process saved transcript' }).click();
  await expect(selection).toHaveValue('SYNTHETIC-UI-CONTEXT-SEC14');
  await page.getByRole('button', { name: 'Retry same request' }).click();
  await expect(selection).toHaveValue('');
  expect(state.requests[0]).toEqual(state.requests[1]);
});

 test('polishing controls show a bounded original recovery and check the backup independently', async ({ page }) => {
  const state = await routes(page);
  const panel = page.getByRole('region', { name: 'Dictation modes' });
  await panel.getByLabel('Default recording mode', { exact: true }).selectOption('super');
  await expect(panel.getByLabel('Maximum polishing wait')).toHaveValue('3');
  await panel.getByLabel('Fallback model', { exact: true }).selectOption('openai/gpt-5.6-luna');
  await expect(panel).toContainText('Both models share a provider');
  await panel.getByRole('button', { name: 'Check fallback', exact: true }).click();
  await expect(panel).toContainText('Fallback sample succeeded with gpt-5.6-luna');
  expect(state.previews).toHaveLength(1);
  await panel.getByLabel('Wait before the fallback').fill('5');
  await expect(panel.getByLabel('Maximum polishing wait')).toHaveValue('7');
  await panel.getByRole('button', { name: 'Save changes', exact: true }).click();
  expect(state.settings.profiles[1].fallback_after_ms).toBe(5000);
  await page.reload();
  await expect(panel.getByLabel('Maximum polishing wait')).toHaveValue('7');
 });
