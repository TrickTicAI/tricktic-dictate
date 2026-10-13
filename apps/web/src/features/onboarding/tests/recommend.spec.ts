import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import type { ModelView, RuntimeView } from '../../transcription-setup/api';
import { recommendModel } from '../recommend';
import { compareSpoken, normalizeWords } from '../compare';

/**
 * Run against the catalog the backend actually ships, so a model removed,
 * renamed or re-sized upstream is caught here rather than by a user staring at
 * a guide that offers nothing.
 */
const catalog = JSON.parse(readFileSync(
  fileURLToPath(new URL('../../../../../api/src/desktop_settings/model-catalog.json', import.meta.url)),
  'utf8',
)) as { id: string; display_name: string; precision: string; bytes: number; input_languages: ModelView['input_languages'] }[];

const models: ModelView[] = catalog.map((entry) => ({
  id: entry.id, display_name: entry.display_name, file_name: `${entry.id}.bin`, bytes: entry.bytes,
  sha256: 'a'.repeat(64), source_url: 'https://example.invalid', source_name: 'catalog', license: 'MIT',
  notes: '', input_languages: entry.input_languages, weights: { state: 'not_installed', resumable_bytes: null },
  selected: false, loaded: false, compatibility_issue: null,
  metadata: { family: '', precision: entry.precision, runtime: 'whisper_cpp', backends: [], revision: '', qualification: '' },
}));

const runtime = (gpu: boolean, freeGib: number | null): RuntimeView => ({
  hardware: { architecture: 'x86_64', cpu_threads: 8, total_ram_bytes: null, available_ram_bytes: freeGib === null ? null : freeGib * 2 ** 30, parakeet_cpu_supported: false },
  execution: { requested: 'automatic', cpu_available: true, gpu_available: gpu, fallback_reason: null },
  worker: { state: 'idle' }, problem: null, packaged: true, worker_executable_present: true,
  vulkan_library_present: gpu, ffmpeg_present: true, external_server: false, models_dir: null,
});

test('every language and hardware profile gets a model it can actually run', () => {
  for (const machine of [runtime(true, 8), runtime(false, 4), runtime(false, 1), runtime(false, null)]) {
    const budget = machine.execution?.gpu_available ? 2 * 2 ** 30
      : machine.hardware?.available_ram_bytes ? Math.max(machine.hardware.available_ram_bytes * 0.4, 1e8) : 6e8;
    for (const language of ['en', 'he', 'mixed'] as const) {
      const choice = recommendModel(models, machine, language);
      expect(choice, language).not.toBeNull();
      // Declared support, and a download this machine can hold.
      expect(choice!.best.input_languages, `${language} support`).toContain(language);
      expect(choice!.best.bytes, `${language} within budget`).toBeLessThanOrEqual(budget);
      // The alternatives are real alternatives, never the same file again.
      if (choice!.lighter) expect(choice!.lighter.bytes).toBeLessThanOrEqual(choice!.best.bytes / 2);
      if (choice!.stronger) expect(choice!.stronger.bytes).toBeGreaterThan(choice!.best.bytes);
      for (const other of [choice!.lighter, choice!.stronger]) {
        if (other) expect(other.input_languages, `${language} alternative`).toContain(language);
      }
      expect(choice!.reason).not.toBe('');
    }
  }
});

test('Hebrew-only weights are only ever offered for Hebrew', () => {
  const hebrewOnly = models.filter((model) => model.input_languages.length === 1 && model.input_languages[0] === 'he');
  expect(hebrewOnly.length).toBeGreaterThan(0);
  for (const language of ['en', 'mixed'] as const) {
    const choice = recommendModel(models, runtime(true, 32), language)!;
    for (const offered of [choice.best, choice.lighter, choice.stronger]) {
      if (offered) expect(hebrewOnly.map((model) => model.id)).not.toContain(offered.id);
    }
  }
});

test('an installed model is preferred over any download', () => {
  const installed = models.map((model) => model.id === 'whisper-tiny-q5-1'
    ? { ...model, weights: { state: 'installed' as const, bytes: model.bytes, sha256: 'a', verified_at_ms: 1, origin: 'download' } }
    : model);
  const choice = recommendModel(installed, runtime(true, 32), 'en')!;
  expect(choice.best.id).toBe('whisper-tiny-q5-1');
  expect(choice.reason).toContain('Already downloaded');
});

test('an empty or wholly incompatible catalog recommends nothing rather than guessing', () => {
  expect(recommendModel([], runtime(true, 32), 'en')).toBeNull();
  const blocked = models.map((model) => ({ ...model, compatibility_issue: 'This runtime requires AVX2.' }));
  expect(recommendModel(blocked, runtime(true, 32), 'en')).toBeNull();
});

test('the word comparison ignores punctuation and case but not Hebrew vowel marks', () => {
  expect(normalizeWords('Send the notes, please!')).toEqual(['send', 'the', 'notes', 'please']);
  const exact = compareSpoken('Send the notes before four.', 'send the notes before four');
  expect(exact.matched).toBe(5);
  expect(exact.expected).toBe(5);
  expect(exact.cells.every((cell) => cell.status === 'match')).toBe(true);

  const drifted = compareSpoken('Send the notes before four.', 'Send the vote before five o’clock');
  expect(drifted.expected).toBe(5);
  expect(drifted.matched).toBe(3);
  expect(drifted.cells.filter((cell) => cell.status === 'missing').map((cell) => cell.text)).toEqual(['notes', 'four']);
  expect(drifted.cells.filter((cell) => cell.status === 'extra').map((cell) => cell.text)).toEqual(['vote', 'five', "o'clock"]);

  // A vowelled and an unvowelled spelling are different words, not a match.
  expect(compareSpoken('שָׁלוֹם', 'שלום').matched).toBe(0);
  expect(compareSpoken('שלום עולם', 'שלום עולם').matched).toBe(2);
});
