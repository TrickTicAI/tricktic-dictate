import { expect, type Page } from '@playwright/test';

/** Only for UI suites that already stub the API. Real auth is tested separately. */
export async function stubOwnerSession(page: Page) {
  await page.route('**/api/v1/session', route => route.fulfill({ json: {
    api_version: '1', data: { token: 'a'.repeat(64), expires_in_seconds: 43200 },
  } }));
}
export async function loginFixture(page: Page) {
  await page.getByLabel('Owner key').fill('b'.repeat(64));
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Sign out', exact: true })).toBeVisible();
}
