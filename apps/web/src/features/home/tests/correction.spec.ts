import { expect, test, type Page } from '@playwright/test';

/**
 * Correct transcript: a reference kept beside what was heard, and the drafts
 * queued for a check. The server is routed, and every write it receives is
 * kept so the test can see exactly what was asked of it.
 */
const MODES = { revision: 1, active: 'regular', profiles: [{ id: 'regular', revision: 1, name: 'Regular', kind: 'regular', instructions: '', text_model: null, context: { application: false, selected_text: false, clipboard: false } }] };
const ENGLISH = 'The quick brown fox jumps over the lazy dog, and then keeps going for a while longer so the row has something to clamp.';
const DRAFT = { recording_id: 'b', job_id: 'job-b', text: 'שלום, זאת הקלטה בעברית', gold: true, source: 'draft', needs_check: true, alternative: 'שלום, זו הקלטה בעברית', updated_at_ms: 1 };

async function open(page: Page) {
  const writes: { method: string; url: string; body: unknown }[] = [];
  await page.route('**/api/v1/jobs/*/live-processing', route => route.fulfill({ json: { api_version: '1', data: { bound: false, attempt: null } } }));
  await page.route('**/api/v1/jobs/*/processed', (route) => route.fulfill({ json: { api_version: '1', data: { text: '' } } }));
  await page.route('**/api/v1/jobs/*/processing', (route) => route.fulfill({ json: { api_version: '1', data: [] } }));
  await page.route('**/api/v1/cleanup/providers', (route) => route.fulfill({ json: { api_version: '1', data: [] } }));
  await page.route('**/api/v1/modes', (route) => route.fulfill({ json: { api_version: '1', data: MODES } }));
  await page.route('**/api/v1/modes/providers', (route) => route.fulfill({ json: { api_version: '1', data: [] } }));
  await page.route('**/api/v1/references', (route) => route.fulfill({ json: { api_version: '1', data: [DRAFT] } }));
  await page.route('**/api/v1/recordings/*/reference', async (route) => {
    const request = route.request();
    const recording = request.url().split('/').at(-2)!;
    if (request.method() === 'DELETE') {
      writes.push({ method: 'DELETE', url: request.url(), body: null });
      return route.fulfill({ status: 204 });
    }
    const body = request.postDataJSON() as { data: { job_id: string; text: string; gold: boolean } };
    writes.push({ method: request.method(), url: request.url(), body });
    return route.fulfill({ json: { api_version: '1', data: { recording_id: recording, ...body.data, source: 'user', needs_check: false, updated_at_ms: 2 } } });
  });
  await page.goto('/src/features/home/tests/home.html');
  await expect(page.locator('.session-row').first()).toBeVisible();
  return writes;
}

const reader = (page: Page) => page.getByRole('region', { name: 'Transcript', exact: true });

test('a past transcript is corrected beside what was heard and can be marked gold', async ({ page }) => {
  const writes = await open(page);
  await page.locator('.session-row').first().locator('.row-hit').click();
  await reader(page).getByRole('button', { name: 'More actions' }).click();
  await page.getByRole('menuitem', { name: 'Correct transcript' }).click();

  const form = page.getByRole('form', { name: 'Correct transcript' });
  const field = form.getByRole('textbox', { name: 'What you said' });
  // It starts from what was heard, and says the original stays.
  await expect(field).toHaveValue(ENGLISH);
  await expect(form).toContainText('What was heard stays saved as it was');
  await field.fill('The quick brown fox jumps over the lazy dog and merges the worktrees.');
  await form.getByRole('checkbox').uncheck();
  await form.getByRole('button', { name: 'Save correction' }).click();

  expect(writes).toEqual([{ method: 'PUT', url: expect.stringContaining('/api/v1/recordings/a/reference'), body: { api_version: '1', data: { job_id: 'job-a', text: 'The quick brown fox jumps over the lazy dog and merges the worktrees.', gold: false } } }]);
  const saved = page.getByRole('region', { name: 'Corrected transcript' });
  await expect(saved).toContainText('Your correction');
  await expect(saved).toContainText('merges the worktrees');
  // What was heard is still the page.
  await expect(reader(page).locator('.transcript-block .transcript-text')).toHaveText(ENGLISH);
  await expect(page.locator('.session-row').first().locator('.row-meta')).toContainText('Corrected');

  await saved.getByRole('button', { name: 'Mark gold' }).click();
  expect((writes[1].body as { data: { gold: boolean } }).data.gold).toBe(true);
  await expect(saved).toContainText('Gold');
  await expect(page.locator('.session-row').first().locator('.row-meta')).toContainText('Gold');

  await saved.getByRole('button', { name: 'Remove', exact: true }).click();
  expect(writes[2].method).toBe('DELETE');
  await expect(saved).toHaveCount(0);
});

test('drafts that disagree are queued to check, and checking one saves it as yours', async ({ page }) => {
  const writes = await open(page);
  const filter = page.getByRole('group', { name: 'Show' }).getByRole('button', { name: /To check/ });
  await expect(filter).toContainText('1');
  await filter.click();
  await expect(page.locator('.session-row')).toHaveCount(1);
  const row = page.locator('.session-row').first();
  await expect(row.locator('.row-meta')).toContainText('To check');
  await row.locator('.row-hit').click();

  const draft = page.getByRole('region', { name: 'Corrected transcript' });
  await expect(draft).toContainText('Draft to check');
  await draft.getByRole('button', { name: 'Check now' }).click();
  const form = page.getByRole('form', { name: 'Correct transcript' });
  const field = form.getByRole('textbox', { name: 'What you said' });
  await expect(field).toHaveValue(DRAFT.text);
  await expect(field).toHaveAttribute('dir', 'rtl');
  await form.getByRole('button', { name: 'Use this draft' }).click();
  await expect(field).toHaveValue(DRAFT.alternative);
  await form.getByRole('button', { name: 'Save correction' }).click();

  expect(writes[0].body).toEqual({ api_version: '1', data: { job_id: 'job-b', text: DRAFT.alternative, gold: true } });
  await expect(draft).toContainText('Your correction');
  await expect(page.getByRole('group', { name: 'Show' }).getByRole('button', { name: /To check/ })).not.toContainText('1');
});

test('a correction proposes words and mishearings to learn, each accepted or declined', async ({ page }) => {
  await open(page);
  const asked: unknown[] = [];
  const decided: unknown[] = [];
  await page.route('**/api/v1/vocabulary/suggestions', async (route) => {
    asked.push(route.request().postDataJSON());
    await route.fulfill({ json: { api_version: '1', data: [{ kind: 'mishearing', heard: 'lazy dog', meant: 'Claude' }, { kind: 'term', text: 'Claude' }] } });
  });
  await page.route('**/api/v1/vocabulary/suggestions/decision', async (route) => {
    decided.push(route.request().postDataJSON());
    await route.fulfill({ json: { api_version: '1', data: { mishearings: [], declined: [] } } });
  });
  await page.locator('.session-row').first().locator('.row-hit').click();
  await reader(page).getByRole('button', { name: 'More actions' }).click();
  await page.getByRole('menuitem', { name: 'Correct transcript' }).click();
  const form = page.getByRole('form', { name: 'Correct transcript' });
  const corrected = ENGLISH.replace('lazy dog', 'Claude');
  await form.getByRole('textbox', { name: 'What you said' }).fill(corrected);
  await form.getByRole('button', { name: 'Save correction' }).click();

  const learn = page.getByRole('region', { name: 'Learn from this correction' });
  await expect(learn).toContainText('“lazy dog” is often “Claude”');
  await expect(learn).toContainText('Add “Claude” to your words');
  expect(asked).toEqual([{ api_version: '1', data: { heard: ENGLISH, corrected } }]);

  await learn.getByRole('button', { name: 'Learn' }).click();
  await expect(learn).not.toContainText('lazy dog');
  await learn.getByRole('button', { name: 'Not this: Claude' }).click();
  await expect(learn).toHaveCount(0);
  expect(decided).toEqual([
    { api_version: '1', data: { suggestion: { kind: 'mishearing', heard: 'lazy dog', meant: 'Claude' }, accept: true } },
    { api_version: '1', data: { suggestion: { kind: 'term', text: 'Claude' }, accept: false } },
  ]);
  // The correction itself was saved before anything was asked.
  await expect(page.getByRole('region', { name: 'Corrected transcript' })).toContainText('Your correction');
});
