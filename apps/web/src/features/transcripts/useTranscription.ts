import { useCallback, useEffect, useRef, useState } from 'react';
import type { Job, RecordingMetadata, TranscriptionOptions } from '@metrix-writer/contracts';
import { ensureUploaded, getJob, submit } from './api';
import { attempts, saveAttempt, type Attempt } from './storage';
import { modeApi, type ModeProfile } from '../modes/api';

export function useTranscription(exportAudio: (id: string) => Promise<Blob>) {
  const [saved, setSaved] = useState<Record<string, Attempt>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [uploadProgress, setUploadProgress] = useState<Record<string, string>>({});
  const [working, setWorking] = useState<Record<string, boolean>>({});
  const [ready, setReady] = useState(false);
  const current = useRef<Record<string, Attempt>>({});
  const locks = useRef(new Set<string>());
  const error = useCallback((id: string, cause: unknown) => setErrors(previous => ({ ...previous, [id]: cause instanceof Error ? cause.message : 'Request failed. Retry the saved session.' })), []);
  const remember = useCallback(async (attempt: Attempt) => {
    await saveAttempt(attempt);
    current.current = { ...current.current, [attempt.id]: attempt };
    setSaved(current.current);
  }, []);
  useEffect(() => {
    let active = true;
    attempts().then(items => {
      if (!active) return;
      current.current = Object.fromEntries(items.map(item => [item.id, item]));
      setSaved(current.current); setReady(true);
    }).catch(cause => error('storage', cause));
    return () => { active = false; };
  }, [error]);
  useEffect(() => {
    if (!ready) return;
    let active = true;
    let timer: number;
    async function poll() {
      for (const attempt of Object.values(current.current)) {
        if (!active) return;
        if (!attempt.job || !['queued', 'transcribing'].includes(attempt.job.status.state)) continue;
        try {
          const job = await getJob(attempt.job.id);
          if (active && current.current[attempt.id]?.request.idempotency_key === attempt.request.idempotency_key) {
            await remember({ ...attempt, job });
            setErrors(previous => ({ ...previous, [attempt.id]: '' }));
          }
        } catch (cause) { if (active) error(attempt.id, cause); }
      }
      if (active) timer = window.setTimeout(() => void poll(), Object.values(current.current).some(a => a.job && ['queued', 'transcribing'].includes(a.job.status.state)) ? 150 : 1000);
    }
    void poll();
    return () => { active = false; window.clearTimeout(timer); };
  }, [ready, remember, error]);
  const transcribe = useCallback(async (metadata: RecordingMetadata, options: TranscriptionOptions, capturedMode?: ModeProfile) => {
    const id = metadata.id;
    if (!ready || locks.current.has(id)) return;
    locks.current.add(id); setWorking(previous => ({ ...previous, [id]: true }));
    setErrors(previous => ({ ...previous, [id]: '' }));
    try {
      const previous = current.current[id];
      // Unacknowledged submits reuse their exact immutable payload and idempotency key.
      let attempt: Attempt = previous && !previous.job ? previous : {
        id, capturedMode: previous?.capturedMode ?? capturedMode, history: [...(previous?.history ?? []), ...(previous?.job ? [previous.job] : [])], request: { recording_id: id, idempotency_key: crypto.randomUUID(), options: { ...options, cleanup: { mode: 'off' } },
          ...(previous?.job ? { retry_of_job_id: previous.job.id } : {}) },
      };
      if (previous?.job && ['queued', 'transcribing'].includes(previous.job.status.state)) return;
      await remember(attempt);
      await ensureUploaded(metadata, () => exportAudio(id), message => setUploadProgress(previous => ({ ...previous, [id]: message })));
      if (attempt.capturedMode) await modeApi.bind(id, attempt.capturedMode);
      attempt = { ...attempt, job: await submit(attempt.request) };
      await remember(attempt);
    } catch (cause) { error(id, cause); }
    finally { setUploadProgress(previous => ({ ...previous, [id]: '' })); locks.current.delete(id); setWorking(previous => ({ ...previous, [id]: false })); }
  }, [ready, remember, exportAudio, error]);
  const updateJob = useCallback((job: Job) => {
    const attempt = current.current[job.request.recording_id];
    if (attempt?.job?.id === job.id) void remember({ ...attempt, job }).catch(cause => error(attempt.id, cause));
  }, [remember, error]);
  const uploadState: { uploadProgress?: Record<string, string> } = { uploadProgress };
  return { saved, errors, working, ...uploadState, ready, transcribe, updateJob };
}
