import { expect, test, type Page } from '@playwright/test';

/*
 * Every settings row keeps its title and description readable beside its
 * control. A control is sized by its content, so one long value (an OpenRouter
 * model name, a key state, a device name) used to push into the title column
 * and squeeze "Text model" into "Text / mo". This walks every section of the
 * real dialog at the window sizes people use, with the longest values we ship,
 * and checks the geometry of every row rather than one screen at a time.
 */

const LONG = ['mistralai/mistral-medium-3-5', 'meta-llama/llama-4-maverick-17b-128e-instruct'];

async function routes(page: Page) {
  const text_model = { provider: 'openrouter', model: LONG[1] };
  const context = { application: false, selected_text: false, clipboard: false };
  await page.route('**/api/v1/**', route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/v1/modes/providers') return route.fulfill({ json: { api_version: '1', data: LONG.map(model => ({ selection: { provider: 'openrouter', model }, configured: true, execution_location: 'cloud' })) } });
    if (path === '/api/v1/modes/custom-models') return route.fulfill({ json: { api_version: '1', data: [] } });
    if (path === '/api/v1/modes') return route.fulfill({ json: { api_version: '1', data: { revision: 1, active: 'super', profiles: [
      { id: 'regular', name: 'Regular', revision: 1, kind: 'regular', instructions: '', text_model: null, context },
      { id: 'super', name: 'Super', revision: 1, kind: 'super', instructions: 'Format paragraphs', text_model, fallback_model: { provider: 'openrouter', model: LONG[0] }, context },
    ] } } });
    return route.fulfill({ status: 404, json: {} });
  });
}

/** Rows whose title column is narrower than this, or that overlap their control, fail. */
const MIN_TEXT = 150;

async function brokenRows(page: Page) {
  return page.locator('.modal-panel .setting-row:not(.stacked)').evaluateAll((rows, min) => rows.flatMap(row => {
    const text = row.querySelector(':scope > .setting-text');
    const control = row.querySelector(':scope > .setting-control');
    if (!text || !control || !control.childElementCount) return [];
    const t = text.getBoundingClientRect(), c = control.getBoundingClientRect(), r = row.getBoundingClientRect();
    if (!r.width || !c.width) return [];
    const title = text.querySelector('strong')?.textContent ?? '';
    const problems = [
      t.width < min && `title column ${Math.round(t.width)}px`,
      t.right > c.left + 1 && c.top < t.bottom && `control overlaps the title by ${Math.round(t.right - c.left)}px`,
      c.right > r.right + 1 && `control overflows the row by ${Math.round(c.right - r.right)}px`,
      [...control.querySelectorAll('select, input, button')].some(el => el.getBoundingClientRect().right > r.right + 1) && 'a control is cut off',
    ].filter(Boolean);
    return problems.length ? [`${title}: ${problems.join(', ')}`] : [];
  }), MIN_TEXT);
}

for (const viewport of [{ width: 1137, height: 790 }, { width: 1024, height: 700 }, { width: 1440, height: 900 }]) {
  test(`every settings row stays readable at ${viewport.width}×${viewport.height}`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await routes(page);
    await page.goto('/src/features/settings/tests/modal-focus.html');
    const nav = page.getByRole('navigation', { name: 'Settings sections' });
    // Each section button is named by its title; its live state is a description.
    const sections = await nav.locator('button[aria-current]').evaluateAll(buttons => buttons.map(button => button.getAttribute('aria-label') ?? ''));
    expect(sections.length).toBeGreaterThan(5);
    const failures: string[] = [];
    for (const name of sections) {
      await nav.getByRole('button', { name, exact: true }).click();
      await expect(nav.getByRole('button', { name, exact: true })).toHaveAttribute('aria-current', 'true');
      if (name === 'Modes') await expect(page.getByLabel('Text-processing model')).toBeVisible();
      await page.waitForTimeout(50);
      failures.push(...(await brokenRows(page)).map(line => `${name} › ${line}`));
    }
    expect(failures).toEqual([]);
  });
}
