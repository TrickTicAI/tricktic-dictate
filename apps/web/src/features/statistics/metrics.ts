import type { InputLanguage, Job, ModelSelection } from '@metrix-writer/contracts';
import type { Recording } from '../../recorder';
import { countWords, successfulJob, type SessionAttempt } from '../home/statistics';
import type { ProcessingAttempt } from '../modes/api';

export const RANGES = ['7', '30', 'all'] as const;
export type Range = typeof RANGES[number];
export const measured = (n: unknown): n is number => typeof n === 'number' && Number.isFinite(n) && n >= 0;
export function duration(ms: number | null | undefined): string {
  if (!measured(ms)) return '—';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const seconds = Math.round(ms / 1000);
  return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}
export function percentile(values: number[], p: number): number | null {
  const sorted = values.filter(measured).sort((a, b) => a - b);
  return sorted.length ? sorted[Math.max(0, Math.ceil(sorted.length * p) - 1)] : null;
}
export const average = (values: number[]) => values.length ? values.reduce((a, b) => a + b, 0) / values.length : null;
export function jobsFor(attempt?: SessionAttempt): Job[] {
  const jobs = new Map<string, Job>();
  for (const job of [...(attempt?.history ?? []), ...(attempt?.job ? [attempt.job] : [])]) {
    if (!jobs.has(job.id) || jobs.get(job.id)!.updated_at_ms <= job.updated_at_ms) jobs.set(job.id, job);
  }
  return [...jobs.values()].sort((a, b) => b.created_at_ms - a.created_at_ms);
}
/** Raw-only job lifecycle, including queue. Cleanup can overwrite updated_at_ms,
 * so those jobs have no trustworthy separate transcription duration. */
export function jobTime(job: Job): number | null {
  if (job.status.state === 'succeeded' && job.status.result.cleanup.state !== 'off') return null;
  return ['succeeded', 'failed'].includes(job.status.state) && measured(job.updated_at_ms) && measured(job.created_at_ms) && job.updated_at_ms >= job.created_at_ms
    ? job.updated_at_ms - job.created_at_ms : null;
}
export function selectedRecordings(recordings: Recording[], range: Range, now = new Date()): Recording[] {
  const start = new Date(now); start.setHours(0, 0, 0, 0);
  if (range !== 'all') start.setDate(start.getDate() - Number(range) + 1);
  return [...new Map(recordings.map(r => [r.id, r])).values()]
    .filter(r => range === 'all' || r.createdAt.getTime() >= start.getTime())
    .sort((a, b) => b.createdAt.getTime() - a.createdAt.getTime());
}
export function activity(recordings: Recording[], saved: Record<string, SessionAttempt>, days: number, now = new Date()) {
  return Array.from({ length: days }, (_, i) => {
    const day = new Date(now); day.setHours(0, 0, 0, 0); day.setDate(day.getDate() - days + i + 1);
    const end = new Date(day); end.setDate(end.getDate() + 1);
    const sessions = recordings.filter(r => r.createdAt >= day && r.createdAt < end);
    const words = sessions.reduce((sum, r) => sum + wordsOf(saved[r.id]), 0);
    const ms = sessions.reduce((sum, r) => sum + (measured(r.duration) ? r.duration : 0), 0);
    return { day, words, sessions: sessions.length, ms };
  });
}

/** Words in a recording's newest successful transcript, or 0. */
export function wordsOf(attempt: SessionAttempt | undefined): number {
  const job = successfulJob(attempt);
  return job?.status.state === 'succeeded' ? countWords(job.status.result.transcript.raw_text) : 0;
}

/** Time spent, as people say it: "45 s", "38 min", "4 h 12 min". */
export function timeSpent(ms: number | null | undefined): string {
  if (!measured(ms) || ms === 0) return '0 min';
  if (ms < 60_000) return `${Math.max(1, Math.round(ms / 1000))} s`;
  const minutes = Math.round(ms / 60_000);
  if (minutes < 60) return `${minutes} min`;
  return minutes % 60 ? `${Math.floor(minutes / 60)} h ${minutes % 60} min` : `${minutes / 60} h`;
}

/**
 * The recordings of the period just before this one, for "vs previous 7 days".
 * All history has no previous period.
 */
export function previousRecordings(recordings: Recording[], range: Range, now = new Date()): Recording[] | null {
  if (range === 'all') return null;
  const end = new Date(now); end.setHours(0, 0, 0, 0); end.setDate(end.getDate() - Number(range) + 1);
  const start = new Date(end); start.setDate(start.getDate() - Number(range));
  return [...new Map(recordings.map(r => [r.id, r])).values()].filter(r => r.createdAt >= start && r.createdAt < end);
}

/** Percentage change, or null when there is nothing honest to compare with. */
export function change(current: number | null, previous: number | null): number | null {
  if (current === null || previous === null || !(previous > 0)) return null;
  return Math.round((current - previous) / previous * 100);
}

/** Rounds a chart's top up to 1, 2, 2.5 or 5 times a power of ten. */
export function niceMax(value: number): number {
  if (!(value > 0)) return 1;
  const power = 10 ** Math.floor(Math.log10(value));
  return ([1, 2, 2.5, 5, 10].find(step => step * power >= value) ?? 10) * power;
}

const dayStart = (date: Date) => { const day = new Date(date); day.setHours(0, 0, 0, 0); return day; };

/**
 * Days in a row with at least one dictation. The current run ends today, or
 * yesterday when nothing has been said yet today; the longest is over all
 * loaded history. Local dates, so a run survives a daylight-saving change.
 */
export function streaks(recordings: Recording[], now = new Date()) {
  const days = [...new Set(recordings.map(r => dayStart(r.createdAt).getTime()))].sort((a, b) => a - b);
  const has = new Set(days);
  const cursor = dayStart(now);
  if (!has.has(cursor.getTime())) cursor.setDate(cursor.getDate() - 1);
  let current = 0;
  while (has.has(cursor.getTime())) { current++; cursor.setDate(cursor.getDate() - 1); }
  let longest = 0, run = 0, previous: Date | null = null;
  for (const time of days) {
    const day = new Date(time);
    if (previous) { const next = new Date(previous); next.setDate(next.getDate() + 1); run = next.getTime() === time ? run + 1 : 1; }
    else run = 1;
    longest = Math.max(longest, run);
    previous = day;
  }
  return { current, longest };
}

/** Words by weekday (Monday first) and hour of day. */
export function heatmap(recordings: Recording[], saved: Record<string, SessionAttempt>): number[][] {
  const grid = Array.from({ length: 7 }, () => new Array<number>(24).fill(0));
  for (const r of new Map(recordings.map(item => [item.id, item])).values()) {
    const words = wordsOf(saved[r.id]);
    if (words) grid[(r.createdAt.getDay() + 6) % 7][r.createdAt.getHours()] += words;
  }
  return grid;
}

/** Share of words per spoken language, largest first; languages with no words are left out. */
export function languageShare(recordings: Recording[], saved: Record<string, SessionAttempt>) {
  const words = new Map<InputLanguage, number>();
  for (const r of new Map(recordings.map(item => [item.id, item])).values()) {
    const count = wordsOf(saved[r.id]);
    if (count) words.set(r.language, (words.get(r.language) ?? 0) + count);
  }
  const total = [...words.values()].reduce((a, b) => a + b, 0);
  return [...words].map(([id, count]) => ({ id, words: count, share: total ? Math.round(count / total * 100) : 0 })).sort((a, b) => b.words - a.words);
}

/** The busiest day and the longest dictation: facts, nothing estimated. */
export function highlights(recordings: Recording[], saved: Record<string, SessionAttempt>) {
  const byDay = new Map<number, { day: Date; words: number; sessions: number }>();
  let longest: Recording | null = null;
  for (const r of new Map(recordings.map(item => [item.id, item])).values()) {
    const day = dayStart(r.createdAt);
    const entry = byDay.get(day.getTime()) ?? { day, words: 0, sessions: 0 };
    entry.words += wordsOf(saved[r.id]); entry.sessions += 1;
    byDay.set(day.getTime(), entry);
    if (measured(r.duration) && (!longest || r.duration > longest.duration)) longest = r;
  }
  const busiest = [...byDay.values()].filter(d => d.words > 0).sort((a, b) => b.words - a.words || b.sessions - a.sessions)[0] ?? null;
  return { busiest, longest };
}

/** Speech models: jobs, typical and 90% times, and how many succeeded. */
export function speechModels(jobs: Job[]) {
  const groups = new Map<string, Job[]>();
  for (const job of jobs) {
    const key = `${job.request.options.selection.provider}/${job.request.options.selection.model}`;
    groups.set(key, [...(groups.get(key) ?? []), job]);
  }
  return [...groups].map(([key, group]) => {
    const times = group.flatMap(j => { const ms = jobTime(j); return j.status.state === 'succeeded' && ms !== null ? [ms] : []; });
    const succeeded = group.filter(j => j.status.state === 'succeeded').length;
    const ended = succeeded + group.filter(j => j.status.state === 'failed').length;
    const cloud = group.some(j => j.status.state === 'succeeded' && j.status.result.transcript.execution_location === 'cloud');
    return { key, selection: group[0].request.options.selection as ModelSelection, cloud, jobs: group.length, succeeded, ended,
      median: percentile(times, .5), p90: percentile(times, .9) };
  }).sort((a, b) => b.jobs - a.jobs || a.key.localeCompare(b.key));
}

/** Typing the same words at `wpm`, minus the time spent speaking them. Never below zero. */
export function timeSaved(words: number, speakingMs: number, wpm: number): number {
  if (!(wpm > 0)) return 0;
  return Math.max(0, words / wpm * 60_000 - speakingMs);
}
export function superAttempts(attempts: ProcessingAttempt[]) {
  return [...new Map(attempts.filter(a => a.profile.kind === 'super').map(a => [a.id, a])).values()];
}
const selectedModel = (attempt: ProcessingAttempt) => attempt.diagnostics?.selected_model ?? (attempt.diagnostics?.fallback_used ? attempt.profile.fallback_model : null) ?? attempt.profile.text_model;
export const textModelKey = (attempt: ProcessingAttempt) => { const m = selectedModel(attempt); return m ? `${m.provider}/${m.model}` : ''; };
function modelCalls(attempt: ProcessingAttempt): { selection: ProcessingAttempt['profile']['text_model']; state: string; time: number | null }[] {
  const d = attempt.diagnostics;
  if (d?.model_attempts?.length) return d.model_attempts.map(c => ({ selection: c.selection, state: c.state, time: !d.reused_draft && c.state === 'succeeded' && measured(c.elapsed_ms) ? c.elapsed_ms : null }));
  return [{ selection: selectedModel(attempt), state: d?.original_used ? 'failed' : attempt.state.state, time: !d?.original_used && !d?.reused_draft && attempt.state.state === 'succeeded' && measured(d?.processing_ms) ? d.processing_ms : null }];
}
/**
 * Measured Super model attempts, with captured-primary counts for older records.
 *
 * Times cover successful edits that sent their own request. An edit that reused
 * an early draft finished without one, so it would make a model look faster by
 * how often drafts happened to match rather than by how fast it answers.
 */
export function textModelUse(attempts: ProcessingAttempt[]) {
  const groups = new Map<string, ReturnType<typeof modelCalls>>();
  for (const attempt of attempts) for (const call of modelCalls(attempt)) { const key = call.selection ? `${call.selection.provider}/${call.selection.model}` : ''; groups.set(key, [...(groups.get(key) ?? []), call]); }
  return [...groups].map(([key, group]) => {
    const succeeded = group.filter(a => a.state === 'succeeded');
    const times = succeeded.flatMap(a => a.time !== null ? [a.time] : []);
    return { key, selection: group[0].selection, attempts: group.length, succeeded: succeeded.length, failed: group.filter(a => a.state === 'failed' || a.state === 'timed_out').length, times };
  }).sort((a, b) => b.attempts - a.attempts || a.key.localeCompare(b.key));
}
/** Super model attempts per local day. Each belongs to its recording's date, as retries do elsewhere on the page. */
export function textModelActivity(attempts: ProcessingAttempt[], recordedAt: Map<string, Date>, days: number, now = new Date()) {
  return Array.from({ length: days }, (_, i) => {
    const day = new Date(now); day.setHours(0, 0, 0, 0); day.setDate(day.getDate() - days + i + 1);
    const end = new Date(day); end.setDate(end.getDate() + 1);
    const counts = new Map<string, number>();
    for (const attempt of attempts) {
      const at = recordedAt.get(attempt.job_id);
      if (at && at >= day && at < end) for (const call of modelCalls(attempt)) { const key = call.selection ? `${call.selection.provider}/${call.selection.model}` : ''; counts.set(key, (counts.get(key) ?? 0) + 1); }
    }
    return { day, counts, total: [...counts.values()].reduce((a, b) => a + b, 0) };
  });
}

/** Super model attempts grouped by the host that answered, for one text model. */
export function hostsFor(attempts: ProcessingAttempt[]) {
  const groups = new Map<string, number[]>();
  for (const a of attempts) {
    if (a.state.state !== 'succeeded' || !a.diagnostics || a.diagnostics.original_used || a.diagnostics.reused_draft || !measured(a.diagnostics.processing_ms)) continue;
    const host = a.diagnostics.served_by ?? '';
    groups.set(host, [...(groups.get(host) ?? []), a.diagnostics.processing_ms]);
  }
  return [...groups].map(([host, times]) => ({ host, edits: times.length, median: percentile(times, .5)! })).sort((a, b) => b.edits - a.edits);
}
/**
 * What the saved measurements support saying, and nothing more.
 *
 * Each sentence names the model, the count behind it and the change to make, so
 * it can be checked against the rows above rather than taken on trust.
 */
export function textModelAdvice(attempts: ProcessingAttempt[]): string[] {
  const advice: string[] = [];
  for (const model of textModelUse(attempts)) {
    const name = model.selection?.model;
    if (!name) continue;
    const group = attempts.filter(a => textModelKey(a) === model.key);
    const hosts = hostsFor(group).filter(h => h.host && h.edits >= 3).sort((a, b) => a.median - b.median);
    const fastest = hosts[0];
    const slowest = hosts[hosts.length - 1];
    if (hosts.length > 1 && slowest.median >= 1.5 * fastest.median) {
      advice.push(`${name} took ${duration(slowest.median)} on ${slowest.host} and ${duration(fastest.median)} on ${fastest.host}. In Manage custom models you can pin ${fastest.host} or avoid ${slowest.host}.`);
    }
    const slow = percentile(model.times, .9);
    const primaryGroup = attempts.filter(a => a.profile.text_model && `${a.profile.text_model.provider}/${a.profile.text_model.model}` === model.key);
    const fallbacks = primaryGroup.filter(a => a.diagnostics?.fallback_used).length;
    if (slow !== null && slow > 2500 && model.times.length >= 5 && !fallbacks) {
      advice.push(`One in ten ${name} edits took over ${duration(slow)}. A fallback model for this mode would answer while a slow one is still working.`);
    }
    if (fallbacks && fallbacks >= Math.max(3, model.attempts / 5)) {
      advice.push(`${name} needed its fallback in ${fallbacks} of ${primaryGroup.length} edits. Consider making the fallback the mode's model.`);
    }
  }
  return advice.slice(0, 3);
}
