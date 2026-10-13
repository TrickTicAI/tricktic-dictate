import type { Job, RecordingMetadata, TranscriptionOptions } from '@metrix-writer/contracts';
import { cancelRetries, ensureUploaded, getJob, submit } from '../../transcripts/api';
import { isTauri, invoke, listen } from './native';
import { attempts, deleteAttempt, saveAttempt, type Attempt } from '../../transcripts/storage';

export interface JobDependencies {
  load(): Promise<Attempt[]>;
  save(attempt: Attempt): Promise<void>;
  upload(metadata: RecordingMetadata, audio: () => Promise<Blob>): Promise<void>;
  submit: typeof submit;
  get: typeof getJob;
  cancelRetries?: typeof cancelRetries;
  /** Removes a discarded session's attempt. Defaults to the durable store. */
  forget?(id: string): Promise<void>;
  pollMs?: number;
}
const defaults: JobDependencies = { load: async () => {
  const local = await attempts();
  if (!isTauri()) return local;
  const retained = await invoke<Attempt[]>('saved_attempts');
  // API history survives the shell/origin migration. Preserve any locally saved
  // uncertain request so idempotent submission recovery still uses its exact key.
  return [...local, ...retained.filter(a => !local.some(b => a.id === b.id))];
}, save: saveAttempt, forget: deleteAttempt, upload: async (metadata, audio) => {
  if (isTauri()) await invoke('capture_import', { id: metadata.id });
  else await ensureUploaded(metadata, audio);
}, submit, get: getJob, cancelRetries };
const active = (job: Job) => ['queued', 'transcribing'].includes(job.status.state);
const clone = <T>(value: T): T => structuredClone(value);

/** Single coordinator for the persistent document. Uses the existing durable
 * attempt schema, upload reconciliation, API and raw/history stores. MW-21 must
 * replace useTranscription/App auto-submit when composing this owner. */
export class SessionJobs {
  private canceled = new Set<string>();
  cancel(id: string) {
    this.canceled.add(id);
    const before = this.locks.get(id) ?? Promise.resolve();
    const task = before.catch(() => {}).then(async () => {
      const previous = this.saved.get(id);
      if (!previous?.job || !active(previous.job) || previous.job.recovery?.canceled) return;
      const job = await (this.dependencies.cancelRetries ?? cancelRetries)(previous.job.id);
      this.validateJob(job, previous);
      await this.remember({ ...previous, job });
      this.changed(id, clone(job));
    }).finally(() => { if (this.locks.get(id) === task) this.locks.delete(id); });
    this.locks.set(id, task);
    return task;
  }
  private saved = new Map<string, Attempt>();
  get busy() { return this.locks.size > 0 || this.pendingWrites.size > 0 || [...this.saved.values()].some(a => a.job && active(a.job)); }
  private listeners = new Set<() => void>();
  subscribe(listener: () => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  list() { return [...this.saved.values()].map(clone); }
  async updateJob(job: Job) {
    const previous = this.saved.get(job.request.recording_id);
    if (!previous?.job || previous.job.id !== job.id) return;
    this.validateJob(job, previous);
    await this.remember({ ...previous, job: clone(job) });
    this.changed(previous.id, clone(job));
  }
  /**
   * Drops one session's attempt because its recording was deliberately
   * discarded. Any in-flight work for it settles first, so a completing request
   * cannot write the attempt back after it has been removed.
   */
  async forget(id: string) {
    await this.locks.get(id)?.catch(() => { /* a failed attempt is still finished */ });
    await (this.dependencies.forget ?? deleteAttempt)(id);
    this.saved.delete(id);
    this.pendingWrites.delete(id);
    this.deferredNative.delete(id);
    this.canceled.delete(id);
    this.listeners.forEach((listener) => listener());
  }
  private locks = new Map<string, Promise<void>>();
  private deferredNative = new Map<string, Job>();
  private timer?: ReturnType<typeof setTimeout>;
  private nativeListener?: Promise<() => void>;
  private disposed = false;
  private polling = false;
  private pendingWrites = new Map<string, Attempt>();
  constructor(private changed: (id: string, job: Job | undefined, error?: string) => void,
    private dependencies: JobDependencies = defaults) {}
  async initialize() {
    for (const attempt of await this.dependencies.load()) this.saved.set(attempt.id, clone(attempt));
    if (isTauri()) this.nativeListener = listen<Job>('writer-job-status', job => this.receiveNative(job)).catch(() => () => {});
    this.schedule();
  }
  private receiveNative(job: Job) {
    if (this.disposed || !job?.request) return;
    const id = job.request.recording_id;
    const lock = this.locks.get(id);
    if (lock) {
      // A completion can arrive during submission persistence or a slower poll.
      // Retain the newest event and revalidate after that operation releases its lock.
      const pending = this.deferredNative.get(id);
      if (!pending || job.updated_at_ms >= pending.updated_at_ms) this.deferredNative.set(id, clone(job));
      if (!pending) void lock.catch(() => {}).then(() => {
        const deferred = this.deferredNative.get(id); this.deferredNative.delete(id);
        if (deferred) this.receiveNative(deferred);
      });
      return;
    }
    const previous = this.saved.get(id);
    if (!previous?.job || job.updated_at_ms < previous.job.updated_at_ms) return;
    if (JSON.stringify(job) === JSON.stringify(previous.job)) return;
    const task = Promise.resolve().then(async () => {
      try {
        this.validateJob(job, previous);
        const next = { ...previous, job: clone(job) };
        // The native event follows the authoritative durable write. Browser history
        // is a mirror and must not delay delivery of that already saved result.
        const completed = job.status.state === 'succeeded';
        if (completed) {
          this.saved.set(id, next);
          this.listeners.forEach(listener => { try { listener(); } catch { /* A detached view cannot delay delivery. */ } });
          this.changed(id, clone(job));
        }
        await this.remember(next);
        if (!completed && !this.disposed) this.changed(id, clone(job));
      } catch { /* Fall back to authoritative polling; no provisional native result. */ }
    }).finally(() => { if (this.locks.get(id) === task) this.locks.delete(id); });
    this.locks.set(id, task);
  }
  get(id: string) { const attempt = this.saved.get(id); return attempt && clone(attempt); }
  hasCompletedCommand(key: string) {
    return [...this.saved.values()].some(a => a.request.idempotency_key === key && !!a.job || a.history?.some(j => j.request.idempotency_key === key));
  }
  private async remember(attempt: Attempt) {
    // Keep a completed response in memory if disk temporarily fails, so another
    // poll/retry first retries persistence rather than repeating inference.
    this.pendingWrites.set(attempt.id, clone(attempt));
    await this.dependencies.save(clone(attempt));
    this.saved.set(attempt.id, clone(attempt));
    this.pendingWrites.delete(attempt.id);
    this.listeners.forEach(listener => { try { listener(); } catch { /* a detached observer cannot fail persistence */ } });
  }
  run(metadata: RecordingMetadata, options: TranscriptionOptions | null, audio: () => Promise<Blob>, commandKey: string) {
    const current = this.locks.get(metadata.id);
    if (current) return current;
    const task = this.execute(metadata, options, audio, commandKey).finally(() => { if (this.locks.get(metadata.id) === task) this.locks.delete(metadata.id); });
    this.locks.set(metadata.id, task);
    return task;
  }
  private async execute(metadata: RecordingMetadata, options: TranscriptionOptions | null, audio: () => Promise<Blob>, commandKey: string) {
    const id = metadata.id;
    try {
      if (this.disposed) return;
      const pending = this.pendingWrites.get(id);
      if (pending) await this.remember(pending);
      const previous = this.saved.get(id);
      if (previous?.job && active(previous.job)) {
        const job = await this.dependencies.get(previous.job.id);
        this.validateJob(job, previous);
        await this.remember({ ...previous, job });
        this.changed(id, clone(job)); return;
      }
      // Expired transport retries cannot allocate a second billed job.
      if (this.hasCompletedCommand(commandKey)) { this.changed(id, previous?.job && clone(previous.job)); return; }
      if (!options && !(previous && !previous.job)) { this.changed(id, undefined); return; }
      // A new explicit retry of retained, settled audio is a new user intent.
      if (previous?.job) this.canceled.delete(id);
      let attempt: Attempt = previous && !previous.job ? clone(previous) : {
        id, history: [...(previous?.history ?? []), ...(previous?.job ? [clone(previous.job)] : [])],
        request: { recording_id: id, idempotency_key: commandKey, options: { ...clone(options!), cleanup: { mode: 'off' } },
          ...(previous?.job ? { retry_of_job_id: previous.job.id } : {}) },
      };
      await this.remember(attempt); // before any upload/submission; immutable request survives lost responses
      if (this.disposed) return;
      await this.dependencies.upload(metadata, audio);
      if (this.disposed || this.canceled.has(id)) return;
      const job = await this.dependencies.submit(clone(attempt.request));
      this.validateJob(job, attempt);
      attempt = { ...attempt, job };
      await this.remember(attempt);
      this.changed(id, clone(job));
      this.schedule();
    } catch { this.changed(id, undefined, 'backend_or_storage_unavailable'); }
  }
  private validateJob(job: Job, attempt: Attempt) {
    if (job.request.recording_id !== attempt.id || job.request.idempotency_key !== attempt.request.idempotency_key ||
      attempt.job && job.id !== attempt.job.id || job.status.state === 'succeeded' && typeof job.status.result.transcript.raw_text !== 'string')
      throw new Error('stale_job');
    if (attempt.job?.status.state === 'succeeded' && (job.status.state !== 'succeeded' ||
      job.status.result.transcript.raw_text !== attempt.job.status.result.transcript.raw_text)) throw new Error('raw_changed');
  }
  private schedule() {
    clearTimeout(this.timer); this.timer = undefined;
    if (!this.disposed && !this.polling) this.timer = setTimeout(() => void this.poll(), this.dependencies.pollMs ?? ([...this.saved.values()].some(a => a.job && active(a.job)) ? 150 : 1000));
  }
  private async poll() {
    if (this.polling || this.disposed) return;
    this.polling = true;
    try {
      for (const stored of this.saved.values()) {
        if (this.disposed) return;
        if (this.locks.has(stored.id) || !this.pendingWrites.has(stored.id) && (!stored.job || !active(stored.job))) continue;
        const task = (async () => {
          try {
            const pending = this.pendingWrites.get(stored.id);
            if (pending) { await this.remember(pending); this.changed(stored.id, pending.job && clone(pending.job)); return; }
            const latest = this.saved.get(stored.id)!;
            if (!latest.job || !active(latest.job)) return;
            let job = await this.dependencies.get(latest.job.id);
            if (this.canceled.has(stored.id) && active(job) && !job.recovery?.canceled)
              job = await (this.dependencies.cancelRetries ?? cancelRetries)(latest.job.id);
            this.validateJob(job, latest);
            await this.remember({ ...latest, job });
            this.changed(latest.id, clone(job));
          } catch { this.changed(stored.id, undefined, 'backend_or_storage_unavailable'); }
        })();
        this.locks.set(stored.id, task);
        await task;
        if (this.locks.get(stored.id) === task) this.locks.delete(stored.id);
      }
    } finally { this.polling = false; this.schedule(); }
  }
  dispose() {
    this.disposed = true;
    clearTimeout(this.timer); this.listeners.clear();
    void this.nativeListener?.then(unsubscribe => unsubscribe()).catch(() => {});
    // Already sent requests retain their exact persisted key. Do not block Quit
    // on network completion or claim cancellation of remote compute/billing.
  }
}
