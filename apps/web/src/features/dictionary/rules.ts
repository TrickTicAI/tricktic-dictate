import type { VocabularyDocument } from '@metrix-writer/contracts';

export type Subtab = 'words' | 'corrections' | 'snippets';

/*
 * What the speech models accept, from the adapters themselves
 * (providers/cloud/mod.rs and local_stt/mod.rs): at most 128 words, each at
 * most 256 bytes without angle brackets or line breaks, 8,000 bytes in all for
 * a cloud model and 4,096 for a local one. Past these, every transcription
 * fails, so the Dictionary says so before that happens.
 */
export const ACTIVE_WORDS = 128;
export const WORD_BYTES = 256;
export const LOCAL_WORD_BYTES = 4096;

export const bytes = (text: string) => new TextEncoder().encode(text).length;

const fold = (text: string) => text.toLocaleLowerCase().normalize('NFKC').replace(/\s+/g, ' ').trim();

/** Whether any of the fields contains the search, ignoring case and spacing. */
export function matches(needle: string, ...fields: string[]): boolean {
  const query = fold(needle);
  return !query || fields.some((field) => fold(field).includes(query));
}

/**
 * What is wrong with an entry about to be added (or edited), in words, or null.
 * Blank is reported as a problem too, so Add stays disabled until something
 * real is typed; callers show the sentence only once something was typed.
 */
export function entryProblem(tab: Subtab, value: VocabularyDocument, a: string, b: string): string | null {
  const first = a.trim();
  if (tab === 'words') {
    if (!first) return 'Type a word or phrase.';
    if (/[<>\r\n]/.test(first)) return 'Words can’t contain < or > or line breaks.';
    if (bytes(first) > WORD_BYTES) return `Too long — a word can be at most ${WORD_BYTES} bytes.`;
    if (value.hints.some((hint) => fold(hint.text) === fold(first))) return `“${first}” is already in your words.`;
    return null;
  }
  if (tab === 'corrections') {
    if (!first) return 'Type what you hear.';
    if (!b.trim()) return 'Type what it should say.';
    if (value.corrections.some((rule) => rule.from === first)) return `There is already a correction for “${first}”.`;
    return null;
  }
  if (!first) return 'Type the phrase you will say.';
  if (!b.trim()) return 'Type the text to insert.';
  if (value.snippets.some((rule) => fold(rule.phrase) === fold(first))) return `There is already a snippet for “${first}”.`;
  return null;
}
