import type { Job } from '@metrix-writer/contracts';
import { invoke, isTauri } from './native';
import { request } from '../../transcripts/api';
import type { ProcessingAttempt } from '../../modes/api';
/** `app` is the application the text went to, by its own name ("Slack"), when the shell could tell. */
export interface DeliveryResult { copied: boolean; pasted: boolean; message: string; app?: string }
const pending = new Map<string, Promise<DeliveryResult>>();
const discarded: DeliveryResult = { copied: false, pasted: false, message: 'The dictation was discarded before its text was inserted.' };
/** Invoked only for a just-completed, live dictation. History restoration must
 * never overwrite clipboard content or insert old text into another app.
 * `wanted` is asked once the final text exists, immediately before insertion,
 * so a dictation discarded while its mode was still processing is not inserted. */
export function deliverTranscript(job: Job, wanted: () => boolean = () => true): Promise<DeliveryResult> {
  if (!isTauri() || job.status.state !== 'succeeded') return Promise.resolve({ copied: false, pasted: false, message: '' });
  const previous = pending.get(job.id);
  if (previous) return previous;
  const raw = job.status.result.transcript.raw_text;
  const noSpeech: DeliveryResult = { copied: false, pasted: false, message: 'No speech detected.' };
  if (!raw.trim()) return Promise.resolve(wanted() ? noSpeech : discarded);
  const insert = async (text: string): Promise<DeliveryResult> => {
    if (!wanted()) return discarded;
    if (!text.trim()) return noSpeech;
    try { return await invoke<DeliveryResult>('deliver_transcript', { sessionId: job.request.recording_id, jobId: job.id, text }); }
    catch (error) { return { copied: false, pasted: false, message: typeof error === 'string' ? error : 'Text delivery failed. Copy your saved transcript from history.' }; }
  };
  const task = (async () => {
    // Current hosts wait on native completion events. Older hosts retain the
    // authoritative HTTP path below, including its mode-failure checks.
    const prepared = await invoke<string | { text: string; original_used: boolean; notice: string | null }>('prepare_delivery', { jobId: job.id }).catch(() => undefined);
    if (typeof prepared === 'string') return insert(prepared);
    if (prepared && typeof prepared.text === 'string') {
      const delivered = await insert(prepared.text);
      return prepared.original_used && (delivered.pasted || delivered.copied)
        ? { ...delivered, message: `${delivered.pasted ? 'Original inserted' : 'Original copied'} — polishing unavailable.` }
        : delivered;
    }
    let text = raw;
    let modeProcessed = false;
    // Failure to read the mode binding must never paste a Command instruction as
    // ordinary dictation. Wait only for the already saved, automatic attempt.
    const deadline = Date.now() + 930_000;
    for (;;) {
      const live = await request<{bound: boolean; attempt: ProcessingAttempt | null}>(`/api/v1/jobs/${encodeURIComponent(job.id)}/live-processing`);
      if (!live.bound) break;
      if (live.attempt?.state.state === 'succeeded') { text = live.attempt.state.text; modeProcessed = true; break; }
      if (live.attempt?.state.state === 'failed') return { copied: false, pasted: false, message: `Mode processing failed: ${live.attempt.state.error.message} The original is saved.` };
      if (Date.now() >= deadline) return { copied: false, pasted: false, message: 'Mode processing is incomplete. Your original transcript is saved; check processing history.' };
      await new Promise(resolve => setTimeout(resolve, 250));
    }
    if (!modeProcessed) try {
      const value = await request<{text?: unknown}>(`/api/v1/jobs/${encodeURIComponent(job.id)}/processed`);
      if (typeof value.text === 'string') text = value.text;
    } catch { /* Raw text stays usable if optional processing cannot be read. */ }
    return insert(text);
  })();
  pending.set(job.id, task);
  return task;
}
