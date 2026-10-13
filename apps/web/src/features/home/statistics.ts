import type { Job } from '@metrix-writer/contracts';
import type { Recording } from '../../recorder';

export interface SessionAttempt { job?: Job; history?: Job[] }

/** The newest successful attempt represents a recording once, even after a failed retry. */
export function successfulJob(attempt: SessionAttempt | undefined): Job | undefined {
  return [attempt?.job, ...(attempt?.history ?? [])]
    .filter((job): job is Job => job?.status.state === 'succeeded')
    .sort((a, b) => b.created_at_ms - a.created_at_ms || b.updated_at_ms - a.updated_at_ms)[0];
}

/** Unicode letters/numbers include Hebrew; punctuation alone is never a word. */
export function countWords(text: string): number {
  return (text.match(/[\p{L}\p{N}][\p{L}\p{N}\p{M}]*(?:['’״׳-][\p{L}\p{N}\p{M}]+)*/gu) ?? []).length;
}

export function sessionStatistics(recordings: Recording[], saved: Record<string, SessionAttempt>) {
  let words = 0, transcribed = 0, durationMs = 0, timedWords = 0;
  const seen = new Set<string>();
  for (const recording of recordings) {
    if (seen.has(recording.id)) continue;
    seen.add(recording.id);
    const job = successfulJob(saved[recording.id]);
    if (job?.status.state !== 'succeeded') continue;
    const count = countWords(job.status.result.transcript.raw_text);
    words += count;
    transcribed++;
    if (Number.isFinite(recording.duration) && recording.duration > 0) {
      durationMs += recording.duration;
      timedWords += count;
    }
  }
  return { recordings: seen.size, transcribed, words, durationMs,
    wordsPerMinute: durationMs > 0 ? Math.round(timedWords * 60_000 / durationMs) : null };
}
