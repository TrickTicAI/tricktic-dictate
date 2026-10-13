import { expect, test } from '@playwright/test';
import { languageCatalog, languageIssue } from '../index';
import type { InputLanguage, Settings } from '@metrix-writer/contracts';
const pagePath = '/src/features/language/tests/settings.html';
test('language mismatch, explicit translation and readiness independently gate submission', async ({ page }) => {
  await page.goto(pagePath);
  await expect(page.getByRole('button', { name: 'Transcribe', exact: true })).toBeEnabled();
  await page.getByLabel('Speech language').selectOption('mixed');
  await expect(page.getByRole('alert')).toContainText('Hebrew only');
  await expect(page.getByRole('button', { name: 'Transcribe', exact: true })).toBeDisabled();
  await page.getByLabel('Speech language').selectOption('he');
  await page.getByLabel('Output language').selectOption('en');
  await expect(page.getByRole('alert')).toContainText('Translation is explicitly selected');
  await page.getByLabel('Speech language').selectOption('he');
  await expect(page.getByLabel('Output language')).toHaveValue('preserve');
  await page.getByRole('button', { name: 'Mark unavailable' }).click();
  await expect(page.getByRole('status')).toContainText('unavailable or unverified');
  await expect(page.getByRole('button', { name: 'Transcribe', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Select unverified cloud' }).click();
  await expect(page.getByRole('status')).toContainText('unavailable or unverified');
  await expect(page.getByRole('button', { name: 'Transcribe', exact: true })).toBeEnabled();
});
test('UI preference and speech presets preserve unrelated settings', async ({ page }) => {
  await page.goto(pagePath);
  await page.getByLabel('Interface language').selectOption('he');
  await page.getByLabel('Speech language').selectOption('en');
  const value = JSON.parse((await page.getByTestId('settings').textContent())!) as Settings;
  expect(value.ui_language).toBe('he');
  expect(value.transcription.input_language).toBe('en');
  expect(value.transcription.output_language).toEqual({ mode: 'preserve' });
  expect(value.transcription.selection.model).toBe('ivrit-turbo');
  expect(value.transcription.vocabulary.hints).toEqual(['Metrix']);
});
test('all catalog capability combinations and unknown model fail closed', () => {
  for (const model of languageCatalog.models) {
    for (const input_language of ['en', 'he', 'mixed'] as InputLanguage[]) {
      const options = { selection: model.selection, input_language, output_language: { mode: 'preserve' as const }, cleanup: { mode: 'off' as const }, vocabulary: { hints: [], corrections: [], snippets: [] } };
      expect(languageIssue(options, { available: true }) === null).toBe(model.input_languages.includes(input_language));
      expect(languageIssue(options)).not.toBeNull();
      expect(languageIssue({ ...options, output_language: { mode: 'translate', language: 'en' } }, { available: true })).not.toBeNull();
      expect(languageIssue({ ...options, selection: { provider: 'local', model: 'unknown' } }, { available: true })).toContain('no registered');
    }
  }
});
