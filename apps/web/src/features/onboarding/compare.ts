/**
 * Word-level comparison between a prompt sentence and what was actually
 * transcribed, for the practice step.
 *
 * This exists to answer one question — "did the words I said come back?" — on
 * one sentence, once. It is deliberately not an accuracy metric: it ignores
 * punctuation and case, it has no notion of a substitution being close, and a
 * single reading proves nothing about a model's quality on real dictation. The
 * step says so next to the result.
 */

export type WordStatus = 'match' | 'missing' | 'extra';
export interface WordCell {
  /** Normalised token, which is what was actually compared. */
  text: string;
  status: WordStatus;
}
export interface Comparison {
  /** The alignment in reading order, so it renders as one sentence. */
  cells: WordCell[];
  matched: number;
  expected: number;
}

/**
 * Case, punctuation and spacing are removed; letters, digits, combining marks
 * and the apostrophe are kept. Hebrew niqqud are combining marks and survive,
 * so a vowelled and an unvowelled spelling are not silently equated.
 */
export function normalizeWords(value: string): string[] {
  return value
    .normalize('NFC')
    .toLowerCase()
    .replace(/[‘’]/gu, "'")
    .replace(/[^\p{L}\p{N}\p{M}']+/gu, ' ')
    .split(' ')
    .filter(Boolean);
}

/** Longest-common-subsequence alignment. Sentences are a dozen words, so the
 *  quadratic table costs nothing and keeps the result easy to reason about. */
export function compareSpoken(reference: string, spoken: string): Comparison {
  const expected = normalizeWords(reference);
  const said = normalizeWords(spoken);
  const table: number[][] = Array.from({ length: expected.length + 1 }, () => new Array<number>(said.length + 1).fill(0));
  for (let i = expected.length - 1; i >= 0; i--) {
    for (let j = said.length - 1; j >= 0; j--) {
      table[i][j] = expected[i] === said[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
    }
  }
  const cells: WordCell[] = [];
  let matched = 0;
  let i = 0;
  let j = 0;
  while (i < expected.length && j < said.length) {
    if (expected[i] === said[j]) {
      cells.push({ text: expected[i], status: 'match' });
      matched++; i++; j++;
    } else if (table[i + 1][j] >= table[i][j + 1]) {
      cells.push({ text: expected[i], status: 'missing' });
      i++;
    } else {
      cells.push({ text: said[j], status: 'extra' });
      j++;
    }
  }
  while (i < expected.length) cells.push({ text: expected[i++], status: 'missing' });
  while (j < said.length) cells.push({ text: said[j++], status: 'extra' });
  return { cells, matched, expected: expected.length };
}
