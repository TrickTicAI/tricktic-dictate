import { apiFetch } from '../auth/session';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { Job, ModelSelection } from '@metrix-writer/contracts';

export type CleanupChoice = { selection: ModelSelection; configured: boolean; execution_location: 'cloud' };

async function request<T>(url: string, options: RequestInit = {}): Promise<T> {
  const response = await apiFetch(url, options);
  const body = await response.json();
  if (!response.ok || body.api_version !== '1' || !('data' in body)) {
    throw new Error(body.error?.message || 'Cleanup request failed. Your original transcript remains available.');
  }
  return body.data as T;
}

/** The configured cleanup providers. Reading this never starts a billable call. */
export function useCleanupProviders() {
  const [choices, setChoices] = useState<CleanupChoice[]>([]);
  const [error, setError] = useState('');
  useEffect(() => {
    const controller = new AbortController();
    void request<CleanupChoice[]>('/api/v1/cleanup/providers', { signal: controller.signal })
      .then(setChoices)
      .catch(() => { if (!controller.signal.aborted) setError('Cleanup providers could not be loaded. Original transcripts are unaffected.'); });
    return () => controller.abort();
  }, []);
  return { choices, error };
}

/**
 * Starts and follows one optional cleanup pass.
 *
 * Only an explicit call to `start` sends text to a provider; mounting and
 * polling read status only. Cleanup never replaces the raw transcript — the
 * cleaned text arrives as a separate field on the same job.
 */
export function useCleanup(job: Job | undefined, onJobChange?: (job: Job) => void) {
  const [sending, setSending] = useState(false);
  const [error, setError] = useState('');
  const callback = useRef(onJobChange);
  callback.current = onJobChange;

  const pending = job?.status.state === 'succeeded' && job.status.result.cleanup.state === 'pending';
  const id = job?.id;

  useEffect(() => {
    if (!pending || !id) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const updated = await request<Job>(`/api/v1/jobs/${id}`, { signal: controller.signal });
        if (controller.signal.aborted) return;
        callback.current?.(updated);
        setError('');
        if (updated.status.state !== 'succeeded' || updated.status.result.cleanup.state !== 'pending') return;
      } catch {
        if (controller.signal.aborted) return;
        setError('Cleanup status could not be refreshed. The original transcript is available; retrying is safe.');
      }
      timer = setTimeout(() => void poll(), 1000);
    }
    timer = setTimeout(() => void poll(), 500);
    return () => { controller.abort(); clearTimeout(timer); };
  }, [id, pending]);

  const start = useCallback(async (selection: ModelSelection) => {
    if (!id || sending) return;
    setSending(true);
    setError('');
    try {
      const updated = await request<Job>(`/api/v1/jobs/${id}/cleanup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ api_version: '1', data: selection }),
      });
      callback.current?.(updated);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Cleanup failed. The original transcript is available.');
    } finally {
      setSending(false);
    }
  }, [id, sending]);

  return { start, sending, pending, error };
}
