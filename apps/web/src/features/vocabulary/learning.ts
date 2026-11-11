import { apiFetch } from '../auth/session';

/** Words recognition hears in place of one of your terms, such as "marriage" for "merge". */
export interface Mishearing { heard: string; meant: string; when?: string }
export interface Learning { mishearings: Mishearing[]; declined: string[] }
export type Suggestion =
  | { kind: 'term'; text: string }
  | { kind: 'mishearing'; heard: string; meant: string };

/**
 * What the polish learns from a correction (MW-123). Suggestions are proposed
 * by the API from what was heard and what you wrote; accepting a term adds it
 * to the dictionary's words, accepting a pair keeps it for the polish, and a
 * declined suggestion is never proposed again.
 */
export interface LearningApi {
  suggest(heard: string, corrected: string): Promise<Suggestion[]>;
  decide(suggestion: Suggestion, accept: boolean): Promise<Learning>;
  load(): Promise<Learning>;
  setMishearings(mishearings: Mishearing[]): Promise<Learning>;
}

async function call<T>(method: 'GET' | 'POST' | 'PUT', path: string, data?: unknown): Promise<T> {
  const response = await apiFetch(path, {
    method,
    headers: data === undefined ? undefined : { 'Content-Type': 'application/json' },
    body: data === undefined ? undefined : JSON.stringify({ api_version: '1', data }),
  });
  if (!response.ok) throw new Error('Could not reach what Writer learned from your corrections.');
  const payload = await response.json() as { api_version: string; data: T };
  if (payload.api_version !== '1' || payload.data === undefined) throw new Error('The server returned an invalid answer.');
  return payload.data;
}

export const learningApi: LearningApi = {
  suggest: (heard, corrected) => call('POST', '/api/v1/vocabulary/suggestions', { heard, corrected }),
  decide: (suggestion, accept) => call('POST', '/api/v1/vocabulary/suggestions/decision', { suggestion, accept }),
  load: () => call('GET', '/api/v1/vocabulary/learning'),
  setMishearings: (mishearings) => call('PUT', '/api/v1/vocabulary/learning', { mishearings }),
};

export const suggestionKey = (s: Suggestion) =>
  s.kind === 'term' ? `term:${s.text.toLowerCase()}` : `mishearing:${s.heard.toLowerCase()}→${s.meant.toLowerCase()}`;
