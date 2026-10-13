import { apiFetch } from '../auth/session';
import type { Vocabulary, VocabularyDocument } from '@metrix-writer/contracts';

export interface VocabularyApi {
  load(): Promise<VocabularyDocument>;
  save(value: VocabularyDocument): Promise<VocabularyDocument>;
  /** Loads with the stored version, when the backend reports one. */
  loadVersioned?(): Promise<Versioned>;
  /** Saves only if the stored dictionary is still `version`; throws {@link StaleVocabulary} otherwise. */
  saveIf?(value: VocabularyDocument, version: string | null): Promise<Versioned>;
}

export interface Versioned { document: VocabularyDocument; version: string | null }

/**
 * The stored dictionary changed after this copy was loaded, for example because
 * sync applied another device's edit while this one was being typed. Nothing
 * was saved; the caller reloads instead of overwriting the newer document.
 */
export class StaleVocabulary extends Error {
  constructor() { super('The saved dictionary changed while you were editing.'); }
}

export const emptyDocument = (): VocabularyDocument => ({ hints: [], corrections: [], snippets: [] });

/**
 * What dictation is given: the entries that are switched on, in the order they
 * are listed, with the switch itself dropped.
 *
 * The saved dictionary and the vocabulary a transcription runs with are two
 * different objects on purpose. Everything that transcribes takes the second,
 * so a switched-off rule cannot reach a provider by way of a forgotten filter,
 * and the contract refuses the document shape outright if one ever tried.
 */
export function activeVocabulary(document: VocabularyDocument): Vocabulary {
  return {
    hints: document.hints.filter((hint) => hint.enabled).map((hint) => hint.text),
    corrections: document.corrections.filter((rule) => rule.enabled).map(({ from, to }) => ({ from, to })),
    snippets: document.snippets.filter((rule) => rule.enabled).map(({ phrase, text }) => ({ phrase, text })),
  };
}

/** A dictionary from a backend that predates the switch: everything in it was in force. */
type Stored = {
  hints: (string | VocabularyDocument['hints'][number])[];
  corrections: (Omit<VocabularyDocument['corrections'][number], 'enabled'> & { enabled?: boolean })[];
  snippets: (Omit<VocabularyDocument['snippets'][number], 'enabled'> & { enabled?: boolean })[];
};
function normalize(data: Stored): VocabularyDocument {
  return {
    hints: data.hints.map((hint) => typeof hint === 'string' ? { text: hint, enabled: true } : hint),
    corrections: data.corrections.map((rule) => ({ ...rule, enabled: rule.enabled ?? true })),
    snippets: data.snippets.map((rule) => ({ ...rule, enabled: rule.enabled ?? true })),
  };
}

async function request(method: 'GET' | 'PUT', value?: VocabularyDocument, version?: string | null): Promise<Versioned> {
  const headers: Record<string, string> = {};
  if (value) headers['Content-Type'] = 'application/json';
  if (version) headers['If-Match'] = version;
  const response = await apiFetch('/api/v1/vocabulary', {
    method,
    headers: Object.keys(headers).length ? headers : undefined,
    body: value ? JSON.stringify({ api_version: '1', data: value }) : undefined,
  });
  if (response.status === 412) throw new StaleVocabulary();
  if (!response.ok) {
    // Do not surface arbitrary proxy/upstream bodies in the settings UI.
    throw new Error(response.status === 422 ? 'Check for blank entries or oversized settings.' :
      response.status === 413 ? 'Settings exceed the 256 KiB request limit.' :
      'Could not access saved vocabulary. Your edits are still here; try again.');
  }
  const payload = await response.json() as { api_version: string; data: Stored };
  if (payload.api_version !== '1' || !payload.data ||
    !Array.isArray(payload.data.hints) || !Array.isArray(payload.data.corrections) || !Array.isArray(payload.data.snippets)) {
    throw new Error('The server returned invalid vocabulary settings.');
  }
  return { document: normalize(payload.data), version: response.headers.get('ETag') };
}

export const vocabularyApi: VocabularyApi = {
  load: () => request('GET').then((r) => r.document),
  save: (value) => request('PUT', value).then((r) => r.document),
  loadVersioned: () => request('GET'),
  saveIf: (value, version) => request('PUT', value, version),
};
