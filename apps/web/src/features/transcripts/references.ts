import { useCallback, useEffect, useState } from 'react';
import { apiFetch } from '../auth/session';
import { ApiFailure } from './api';

/** A corrected text kept beside (never over) a recording's raw transcript. */
export interface Reference {
  recording_id: string;
  /** The transcription this corrects; its raw text is unchanged. */
  job_id: string;
  text: string;
  /** Counted in the personal accuracy set. */
  gold: boolean;
  source: 'user' | 'draft';
  /** Agent drafts disagreed, so this waits for the user's check. */
  needs_check: boolean;
  alternative?: string;
  updated_at_ms: number;
}

async function call<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await apiFetch(url, init);
  if (response.status === 204) return null as T;
  const body = await response.json().catch(() => null);
  if (!response.ok || body?.api_version !== '1') {
    throw new ApiFailure(typeof body?.error?.message === 'string' ? body.error.message : `Request failed (${response.status}).`, response.status);
  }
  return body.data as T;
}

const path = (recordingId: string) => `/api/v1/recordings/${recordingId}/reference`;
export const listReferences = () => call<Reference[]>('/api/v1/references');
export const saveReference = (recordingId: string, jobId: string, text: string, gold: boolean) => call<Reference>(path(recordingId), {
  method: 'PUT',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ api_version: '1', data: { job_id: jobId, text, gold } }),
});
export const removeReference = (recordingId: string) => call<null>(path(recordingId), { method: 'DELETE' });

/** Every saved reference by recording. History works the same without them. */
export function useReferences() {
  const [byRecording, setByRecording] = useState<Record<string, Reference>>({});
  useEffect(() => {
    let live = true;
    listReferences()
      .then((all) => { if (live) setByRecording(Object.fromEntries(all.map((item) => [item.recording_id, item]))); })
      .catch(() => {});
    return () => { live = false; };
  }, []);
  const set = useCallback((recordingId: string, reference: Reference | null) => setByRecording((current) => {
    const next = { ...current };
    if (reference) next[recordingId] = reference;
    else delete next[recordingId];
    return next;
  }), []);
  return { byRecording, set };
}
