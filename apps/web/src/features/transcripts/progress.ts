import type { Job } from '@metrix-writer/contracts';

export function recognitionProgress(job?: Job): 'waiting_for_capacity' | 'retrying_1' | 'retrying_2' | undefined {
  if (!job || !['queued', 'transcribing'].includes(job.status.state)) return;
  if (job.recovery?.waiting_for_capacity) return 'waiting_for_capacity';
  const started = job.recovery?.attempts_started ?? 0;
  const retry = job.status.state === 'queued' ? started : started - 1;
  if (retry > 0) return retry >= 2 ? 'retrying_2' : 'retrying_1';
}

export function recognitionStatus(job?: Job): string {
  const progress = recognitionProgress(job);
  if (progress === 'waiting_for_capacity') return 'Waiting for processing capacity…';
  if (progress === 'retrying_1') return 'Retrying transcription automatically · 1 of 2…';
  if (progress === 'retrying_2') return 'Retrying transcription automatically · 2 of 2…';
  return job?.status.state === 'queued' ? 'Transcription queued…' : 'Transcribing…';
}
