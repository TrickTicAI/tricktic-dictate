import { expect, test } from '@playwright/test';
import type { Job } from '@metrix-writer/contracts';
import type { Recording } from '../../../recorder';
import { countWords, sessionStatistics } from '../statistics';

const recording = (id: string, duration = 60_000) => ({ id, duration } as Recording);
const success = (text: string, created = 1) => ({ created_at_ms: created, updated_at_ms: created, status: {
  state: 'succeeded', result: { transcript: { raw_text: text }, cleanup: { state: 'succeeded', cleaned_text: 'Extra cleanup words must not count' } },
} } as Job);
const failure = { status: { state: 'failed' } } as Job;

test('counts Hebrew, English and mixed speech without punctuation-only tokens', () => {
  expect(countWords('שלום עולם. Hello, world!')).toBe(4);
  expect(countWords('... — !')).toBe(0);
  expect(countWords("don't צה״ל 123")).toBe(3);
});

test('counts one newest success per recording, preserves success through failed retry, ignores duplicates and cleanup', () => {
  const rows = [recording('one'), recording('two'), recording('one')];
  const stats = sessionStatistics(rows, {
    one: { job: failure, history: [success('older text'), success('newest successful raw transcript', 2)] },
    two: { job: success('שלום עולם') },
  });
  expect(stats).toEqual({ recordings: 2, transcribed: 2, words: 6, durationMs: 120_000, wordsPerMinute: 3 });
});

test('pace weights actual successful audio duration and excludes failed/untranscribed/unknown duration', () => {
  const stats = sessionStatistics([recording('a', 30_000), recording('b', 90_000), recording('c', 60_000), recording('d', 0)], {
    a: { job: success('one two three') }, b: { job: success('four five six') },
    c: { job: failure }, d: { job: success('unknown duration words') },
  });
  expect(stats.words).toBe(9);
  expect(stats.wordsPerMinute).toBe(3);
  expect(stats.durationMs).toBe(120_000);
  expect(sessionStatistics([], {}).wordsPerMinute).toBeNull();
});
