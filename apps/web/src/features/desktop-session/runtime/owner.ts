import type { InputLanguage, Job, TranscriptionOptions } from '@metrix-writer/contracts';
import { DurableCapture, type CaptureController } from '../../recorder/capture';
import type { LocalSession } from '../../recorder/storage';
import { canTransition, parseMessage, type Ack, type Command, type DesktopSession, type Level, type Snapshot, type SessionState } from '../contracts/protocol';
import { SessionJobs, type JobDependencies } from './jobs';
import { meterStream, type Meter } from './levels';
import { isTauri, NativeCapture } from './native';
import { reserveSession } from './reservations';
import { LocalPreview } from './preview';
import { deliverTranscript } from './delivery';
import { recognitionProgress } from '../../transcripts/progress';
export interface DeliveryResult { copied: boolean; pasted: boolean; message: string; app?: string }
declare global { interface Window { writerCanDeleteHistory?: () => boolean } }

export interface SessionSettings { language: InputLanguage; options: TranscriptionOptions | null; loading?: boolean }
export interface OwnerDependencies {
  settings(): SessionSettings;
  capture?: CaptureController;
  jobs?: JobDependencies;
  meter?: typeof meterStream;
  reserve?: typeof reserveSession;
  deliver?: (job: Job) => Promise<DeliveryResult>;
}
/** A discard blocked by an active job is retried for about two minutes. */
const discardRetryMs = 2000;
const discardAttempts = 60;
/** Far more earlier dictations than can still be finishing at once. */
const detachedLimit = 16;
const terminal = (state: SessionState) => ['idle', 'saved', 'success', 'canceled', 'error'].includes(state);
const copy = <T>(value: T): T => structuredClone(value);
type Acknowledge = (outcome: Ack['outcome'], error_code?: string | null) => Ack;

export class SessionOwner implements DesktopSession {
  private static owner?: SessionOwner;
  private static creating?: Promise<SessionOwner>;
  private value: Snapshot = { version: 1, kind: 'snapshot', epoch: crypto.randomUUID(), revision: 0,
    session_id: null, job_id: null, state: 'idle', audio_saved: false, error_code: null };
  readonly capture: CaptureController;
  readonly jobs: SessionJobs;
  readonly recorder: { capture: CaptureController; start: () => Promise<Ack>; stop: () => Promise<Ack> };
  private listeners = new Set<(event: Snapshot | Level) => void>();
  private acknowledgements = new Map<string, { command: string; ack: Ack }>();
  private settings?: SessionSettings;
  private waitingForSettings = new Map<string, { key: string; metadata?: LocalSession }>();
  /** Bind only sessions that began during startup, at the first complete settings load. */
  settingsLoaded() {
    const settings = copy(this.dependencies.settings());
    if (settings.loading || this.disposed) return;
    for (const [id, pending] of this.waitingForSettings) {
      this.waitingForSettings.delete(id);
      if (id === this.value.session_id && !this.canceled) {
        this.settings = settings;
        if (this.value.state === 'capturing') this.previewWorker.start(id, settings.options);
      }
      if (pending.metadata) {
        if (settings.options) void this.submit(pending.metadata, pending.key, settings);
        else {
          this.submitting.delete(id); this.settleDetached(id);
          if (id === this.value.session_id && this.value.state === 'saved') this.publish('saved');
        }
      }
    }
  }
  get transcriptionReady() { return this.ready && !!this.dependencies.settings().options; }
  private generation = 0;
  private canceled = false;
  /*
   * `canceled` means "do not transcribe this", and is also set when the shell
   * drains a recording on quit -- that path must keep the audio. `discarding`
   * is the narrower, explicit user gesture: throw this dictation away. Only it
   * deletes anything. Capture, storage, transcription and delivery failures
   * still preserve whatever audio was saved.
   */
  private discarding = false;
  /*
   * Work that outlives the dictation on screen is tracked per session. The next
   * dictation can start while the previous one is still being submitted or its
   * text inserted, and one shared flag would be cleared by -- or would hold
   * back -- the wrong dictation.
   */
  private submitting = new Set<string>();
  private processing = new Set<string>();
  /*
   * Earlier dictations that were still being transcribed or inserted when a
   * newer one started. They no longer own the panel, but they still finish, and
   * their text is still inserted, exactly once. `live` says whether it is
   * inserted at all: a history retry's text is not.
   */
  private detached = new Map<string, { live: boolean }>();
  /** Starts waiting for the previous recording to drain, by session; true once let go before they could begin. */
  private queuedStarts = new Map<string, boolean>();
  private disposed = false;
  private ready = false;
  private autoKey = '';
  private stopTask?: Promise<void>;
  private discards = new Map<string, number>();
  /*
   * Discards started from the main window wait out a short Undo window before
   * anything is deleted: session id → earliest time the delete may run. The
   * panel's bin and the discard shortcut never pass through here; they delete
   * at once, as before, because nothing on screen offers to undo them.
   */
  private undoable = new Map<string, number>();
  private discardAfter = new Map<string, number>();
  private discardTimer?: ReturnType<typeof setTimeout>;
  private discardRunning = false;
  private meter?: Meter;
  private meterGeneration = 0;
  private sequence = 0;
  private previewValue = '';
  private deliveryValue?: DeliveryResult;
  private previewListeners = new Set<() => void>();
  private deliveryListeners = new Set<() => void>();
  private waitingListeners = new Set<() => void>();
  private liveSession?: string;
  /** The current dictation's microphone opened, whether or not it has sent sound yet. */
  private opened = false;
  private delivered = new Set<string>();
  private previewWorker = new LocalPreview(text => { this.previewValue = text; this.previewListeners.forEach(f => f()); });
  get preview() { return this.previewValue; }
  get delivery() { return this.deliveryValue && { ...this.deliveryValue }; }
  subscribePreview(listener: () => void) { this.previewListeners.add(listener); return () => { this.previewListeners.delete(listener); }; }
  subscribeDelivery(listener: () => void) { this.deliveryListeners.add(listener); return () => { this.deliveryListeners.delete(listener); }; }
  /**
   * The window's Discard: the same as the cancel command, except that the
   * recording is deleted only after `graceMs`, so `keep` can still bring it back.
   */
  async discardWithUndo(graceMs: number): Promise<Ack> {
    const id = this.value.session_id;
    const command: Command = { version: 1, kind: 'command', epoch: this.value.epoch, command_id: crypto.randomUUID(), action: 'cancel', session_id: id };
    if (id) this.undoable.set(id, graceMs);
    const result = await this.dispatch(command);
    if (id && result.outcome === 'rejected') this.undoable.delete(id);
    return result;
  }
  /**
   * Keeps a recording whose discard is still waiting out its Undo window. It
   * stays in history as saved audio, untranscribed; nothing is pasted. False
   * when the delete has already run (or was never waiting).
   */
  keep(id: string): boolean {
    this.undoable.delete(id);
    if (!this.discardAfter.has(id) || !this.discards.has(id)) return false;
    this.discardAfter.delete(id);
    this.discards.delete(id);
    this.scheduleDiscards();
    return true;
  }
  /** Earlier dictations whose text is still to come while a newer one owns the panel. */
  get waiting() { let count = 0; for (const later of this.detached.values()) if (later.live) count++; return count; }
  subscribeWaiting(listener: () => void) { this.waitingListeners.add(listener); return () => { this.waitingListeners.delete(listener); }; }
  private unsubscribe: () => void;
  private unsubscribeLevel?: () => void;
  private constructor(private dependencies: OwnerDependencies) {
    window.writerCanDeleteHistory = () => !this.disposed && this.ready && !this.submitting.size && !this.processing.size && !this.detached.size &&
      !this.stopTask && terminal(this.value.state) && this.capture.snapshot.state === 'idle' && !this.jobs.busy;
    this.capture = dependencies.capture ?? (isTauri() ? new NativeCapture() : new DurableCapture());
    const command = (action: 'start' | 'stop') => this.dispatch({ version: 1, kind: 'command', epoch: this.value.epoch,
      command_id: crypto.randomUUID(), action, session_id: action === 'start' ? crypto.randomUUID() : this.value.session_id ?? crypto.randomUUID() });
    this.recorder = { capture: this.capture, start: () => command('start'), stop: () => command('stop') };
    this.jobs = new SessionJobs((id, job, error) => this.observeJob(id, job, error), dependencies.jobs);
    this.unsubscribe = this.capture.subscribe(() => this.observeCapture());
    this.unsubscribeLevel = this.capture.subscribeLevel?.((rms, peak) => {
      if (this.value.state === 'capturing') this.emit({ version: 1, kind: 'level', epoch: this.value.epoch,
        revision: this.value.revision, session_id: this.value.session_id!, sequence: ++this.sequence, rms, peak });
    });
  }
  static create(dependencies: OwnerDependencies): Promise<SessionOwner> {
    if (this.owner && !this.owner.disposed) return Promise.resolve(this.owner);
    if (this.creating) return this.creating;
    this.creating = this.initialize(dependencies).then(owner => { this.owner = owner; return owner; })
      .finally(() => { this.creating = undefined; });
    return this.creating;
  }
  private static async initialize(dependencies: OwnerDependencies) {
    const owner = new SessionOwner(dependencies);
    try {
      await owner.jobs.initialize();
      const latest = (await owner.capture.list()).find(s => s.byte_length > 0);
      if (latest) {
        owner.value = { ...owner.value, session_id: latest.id, audio_saved: true, state: 'saved' };
        const attempt = owner.jobs.get(latest.id);
        if (attempt?.job) owner.observeJob(latest.id, attempt.job);
      }
      owner.ready = true;
      return owner;
    } catch (error) { owner.jobs.dispose(); owner.unsubscribe(); owner.capture.dispose(); throw error; }
  }
  private presented(snapshot: Snapshot): Snapshot {
    const id = snapshot.session_id;
    const automatic = id && !this.canceled && (this.submitting.has(id) ||
      id === this.liveSession && (!!this.settings?.options || this.waitingForSettings.has(id)));
    // Audio durability is internal progress when transcription is still due.
    // Keep the processing animation until a job takes over; retain Saved for
    // deliberately record-only sessions and restored history.
    return snapshot.state === 'saved' && automatic ? { ...snapshot, state: 'saving' } : snapshot;
  }
  get snapshot() { return copy(this.presented(this.value)); }
  subscribe(listener: (event: Snapshot | Level) => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  private emit(event: Snapshot | Level) { this.listeners.forEach(listener => { try { listener(copy(event.kind === 'snapshot' ? this.presented(event) : event)); } catch { /* a detached view cannot interrupt saving */ } }); }
  private publish(state: SessionState, changes: Partial<Snapshot> = {}, resynchronize = false) {
    if (!resynchronize && !canTransition(this.value.state, state)) throw new Error('invalid_transition');
    const next = { ...this.value, ...changes, state, revision: this.value.revision + 1, error_code: state === 'error' ? changes.error_code ?? 'capture_failed' : null };
    if (!['queued', 'transcribing'].includes(state) || !next.progress) delete next.progress;
    parseMessage(JSON.stringify(next));
    this.value = next;
    this.emit(next);
  }
  private error(code: string, saved = this.value.audio_saved) {
    if (this.value.session_id) this.waitingForSettings.delete(this.value.session_id);
    this.publish('error', { error_code: code, audio_saved: saved }, true);
  }
  async dispatch(input: Command): Promise<Ack> {
    const parsed = parseMessage(JSON.stringify(input));
    if (parsed.kind !== 'command') throw new Error('invalid_message');
    const command = parsed;
    const ack: Acknowledge = (outcome, error_code = null) => ({ version: 1, kind: 'ack', epoch: this.value.epoch,
      command_id: command.command_id, outcome, error_code });
    if (command.epoch !== this.value.epoch) return ack('rejected', 'stale_epoch');
    const remembered = this.acknowledgements.get(command.command_id);
    if (remembered) return remembered.command === JSON.stringify(command)
      ? remembered.ack.outcome === 'rejected' ? copy(remembered.ack) : ack('duplicate') : ack('rejected', 'invalid_message');
    let result: Ack;
    const current = this.value.session_id ?? '';
    if (this.disposed || !this.ready) result = ack('rejected', 'owner_unavailable');
    else if (command.action === 'snapshot') { this.emit(this.value); result = ack('accepted'); }
    else if (command.action === 'start') result = await this.accept(command.session_id!, command.command_id, ack);
    else if (command.action === 'retry' && terminal(this.value.state) && !this.submitting.size && !this.processing.size && !this.stopTask) {
      if (this.jobs.hasCompletedCommand(command.command_id)) result = ack('duplicate');
      else {
        this.waitingForSettings.delete(command.session_id!);
        this.settings = copy(this.dependencies.settings());
        this.liveSession = undefined;
        this.canceled = false; this.discarding = false; this.submitting.add(command.session_id!);
        const generation = ++this.generation;
        void this.retry(command.session_id!, generation, command.command_id);
        result = ack('accepted');
      }
    } else if ((command.action === 'stop' || command.action === 'cancel') && this.queuedStarts.has(command.session_id!)) {
      // Let go before it could begin: a start still waiting for the previous
      // recording to drain is abandoned rather than left recording.
      this.queuedStarts.set(command.session_id!, true);
      result = ack('accepted');
    } else if (command.session_id !== this.value.session_id) result = ack('rejected', 'stale_session');
    else if (command.action === 'stop' || command.action === 'cancel') {
      if (['success', 'saved', 'error', 'canceled'].includes(this.value.state) && !this.submitting.has(current)) result = ack('accepted');
      else {
        // Released while an open microphone warms up: the recording is real and
        // is finished like any other, not dropped as a start that never began.
        if (command.action === 'stop' && this.warming()) this.publish('capturing');
        if (command.action === 'cancel' || this.value.state === 'requesting') {
          this.canceled = true; this.waitingForSettings.delete(command.session_id!);
        }
        if (command.action === 'cancel') this.discarding = true;
        if (this.value.state === 'requesting') {
          // Our own stop is not the device failing: capture.stop re-enters observeCapture.
          ++this.generation; this.opened = false; this.capture.stop(); this.publish('canceled');
          if (this.discarding) this.queueDiscard(command.session_id!);
        } else if (this.value.state === 'capturing' || this.value.state === 'saving') void this.stop();
        else if (command.action === 'cancel' && (this.submitting.has(current) || ['queued', 'transcribing'].includes(this.value.state))) {
          this.submitting.delete(current); this.processing.delete(current); ++this.generation;
          this.publish('canceled', {}, true);
          void this.jobs.cancel(command.session_id!).catch(() => { /* Polling retries the durable cancellation. */ });
          // The API refuses to delete a recording whose job is still running,
          // so this is retried until that job settles.
          this.queueDiscard(command.session_id!);
        }
        result = ack('accepted');
      }
    } else result = ack('rejected', 'busy');
    this.acknowledgements.set(command.command_id, { command: JSON.stringify(command), ack: copy(result) });
    if (this.acknowledgements.size > 256) this.acknowledgements.delete(this.acknowledgements.keys().next().value!);
    return result;
  }
  /*
   * Starts a dictation. One still being transcribed or inserted does not hold it
   * back: it is detached and finishes in the background. One still draining its
   * last audio is waited for -- tens of milliseconds -- because starting over the
   * drain would abandon that dictation before it was ever submitted.
   */
  private async accept(id: string, key: string, ack: Acknowledge): Promise<Ack> {
    if (this.queuedStarts.has(id)) return ack('duplicate');
    if (this.stopTask) {
      this.queuedStarts.set(id, false);
      await this.stopTask.catch(() => undefined);
      const abandoned = this.queuedStarts.get(id);
      this.queuedStarts.delete(id);
      if (abandoned) return ack('rejected', 'canceled');
      if (this.disposed) return ack('rejected', 'owner_unavailable');
    }
    if (['requesting', 'capturing', 'saving'].includes(this.value.state) || this.capture.snapshot.state !== 'idle' || this.stopTask) return ack('rejected', 'busy');
    this.detach();
    this.settings = copy(this.dependencies.settings());
    if (this.settings.loading) this.waitingForSettings.set(id, { key });
    const generation = ++this.generation;
    this.canceled = false; this.discarding = false; this.opened = false; this.sequence = 0; this.autoKey = key;
    this.liveSession = id; this.deliveryValue = undefined; this.previewValue = '';
    this.deliveryListeners.forEach(f => f()); this.previewListeners.forEach(f => f());
    // Another session begins: the phase table governs one session's own
    // transitions, and the previous one may still be transcribing.
    this.publish('requesting', { session_id: id, job_id: null, audio_saved: false }, true);
    void this.start(id, generation);
    return ack('accepted');
  }
  /** Hands the dictation on screen to the background if its transcription or insertion is still running. */
  private detach() {
    const id = this.value.session_id;
    if (!id || this.canceled) return;
    if (!this.submitting.has(id) && !this.processing.has(id) && !['queued', 'transcribing'].includes(this.value.state)) return;
    this.detached.set(id, { live: this.liveSession === id });
    while (this.detached.size > detachedLimit) this.detached.delete(this.detached.keys().next().value!);
    this.waitingListeners.forEach(f => f());
  }
  /** Drops an earlier dictation once nothing more is coming from it. */
  private settleDetached(id: string) {
    if (this.detached.delete(id)) this.waitingListeners.forEach(f => f());
  }
  private current(id: string, generation: number) { return !this.disposed && id === this.value.session_id && generation === this.generation; }
  private async start(id: string, generation: number) {
    try {
      const existing = await this.capture.store.get(id);
      const reserved = !existing && await (this.dependencies.reserve ?? reserveSession)(id);
      if (!this.current(id, generation)) return;
      if (!reserved) { this.error('session_exists', !!existing?.byte_length); return; }
      await this.capture.start(this.settings!.language, id);
      if (this.current(id, generation) && this.capture.snapshot.state === 'idle' && this.value.state === 'requesting') this.error('capture_failed');
    } catch { if (this.current(id, generation)) this.error('storage_failed'); }
  }
  /** The microphone for the dictation still shown as requesting is open. */
  private warming() { return this.value.state === 'requesting' && this.opened && this.capture.snapshot.state === 'recording'; }
  private observeCapture() {
    if (this.disposed) return;
    const state = this.capture.snapshot.state;
    if (state === 'recording' && this.value.state === 'requesting') this.opened = true;
    if (this.opened && (state === 'stopping' || state === 'idle') && this.value.state === 'requesting') {
      // The device failed or closed before it sent any sound. What it saved is
      // finished and reported like any interrupted recording.
      this.publish('capturing'); this.publish('saving'); void this.stop();
    } else if (state === 'recording' && this.value.state === 'requesting' && this.capture.snapshot.listening !== false) {
      this.publish('capturing');
      this.previewWorker.start(this.value.session_id!, this.settings?.options ?? null);
      const stream = this.capture.activeStream;
      if (stream) void this.startMeter(stream);
    } else if (state === 'recording' && this.value.state === 'capturing' && this.capture.snapshot.savedBytes && !this.value.audio_saved) {
      this.publish('capturing', { audio_saved: true });
    } else if ((state === 'stopping' || state === 'idle') && this.value.state === 'capturing') {
      this.publish('saving'); void this.stop();
    }
  }
  private async startMeter(stream: MediaStream) {
    const generation = ++this.meterGeneration;
    try {
      const meter = await (this.dependencies.meter ?? meterStream)(stream, (rms, peak) => {
        if (generation !== this.meterGeneration || this.value.state !== 'capturing' || this.disposed) return;
        this.emit({ version: 1, kind: 'level', epoch: this.value.epoch, revision: this.value.revision,
          session_id: this.value.session_id!, sequence: ++this.sequence, rms, peak });
      });
      if (generation !== this.meterGeneration || this.value.state !== 'capturing') await meter.dispose();
      else this.meter = meter;
    } catch { /* No synthetic level or stale meter on unsupported/suspended audio contexts. Capture remains durable. */ }
  }
  private async stopMeter() {
    this.previewWorker.stop();
    ++this.meterGeneration; const meter = this.meter; this.meter = undefined;
    try { await meter?.dispose(); } catch { /* Meter failure must never prevent microphone drain. */ }
  }
  /** One attempt at removing a discarded session. True when it is gone. */
  private async discardOnce(id: string) {
    try {
      await this.capture.deleteRecording(id);
      await this.jobs.forget(id);
      this.discards.delete(id);
      return true;
    } catch { return false; }
  }
  /*
   * Discarding is a durable intent rather than one attempt. A dictation
   * canceled while its job is queued or transcribing cannot be removed yet --
   * the store refuses to delete a recording with an active job -- so the
   * request is retried until that job settles. Until it succeeds the session
   * keeps reporting `audio_saved`, so nothing claims a deletion that has not
   * happened; when it does succeed the snapshot is corrected.
   */
  private queueDiscard(id: string) {
    this.discards.set(id, this.discards.get(id) ?? discardAttempts);
    const grace = this.undoable.get(id);
    if (grace !== undefined) { this.undoable.delete(id); this.discardAfter.set(id, Date.now() + grace); }
    this.scheduleDiscards();
  }
  /** The next pass runs when the earliest waiting discard is due. */
  private scheduleDiscards() {
    clearTimeout(this.discardTimer);
    if (!this.discards.size || this.disposed) return;
    const now = Date.now();
    const due = Math.min(...[...this.discards.keys()].map((id) => Math.max(0, (this.discardAfter.get(id) ?? now + discardRetryMs) - now)));
    this.discardTimer = setTimeout(() => void this.runDiscards(), due);
  }
  private async runDiscards() {
    if (this.disposed || this.discardRunning) return;
    this.discardRunning = true;
    try {
      for (const [id, left] of [...this.discards]) {
        const after = this.discardAfter.get(id);
        if (after !== undefined && after > Date.now()) continue;
        this.discardAfter.delete(id);
        if (!this.discards.has(id)) continue;
        if (await this.discardOnce(id)) {
          if (id === this.value.session_id && this.value.state === 'canceled' && this.value.audio_saved) {
            this.publish('canceled', { audio_saved: false }, true);
          }
        } else if (left <= 1) {
          // Giving up is reported by leaving `audio_saved` true: the recording
          // is still there and still listed, which is the truth.
          this.discards.delete(id);
        } else this.discards.set(id, left - 1);
      }
    } finally { this.discardRunning = false; }
    this.scheduleDiscards();
  }
  private stop() {
    if (this.stopTask) return this.stopTask;
    // Schedule the drain before calling capture.stop, whose synchronous updates
    // re-enter observeCapture. All stop/cancel sources join this one finalizer.
    this.stopTask = Promise.resolve().then(() => this.finishCapture()).finally(() => { this.stopTask = undefined; });
    return this.stopTask;
  }
  private async finishCapture() {
    const id = this.value.session_id!; const generation = this.generation;
    try {
      if (this.value.state === 'capturing') this.publish('saving');
      // A kept dictation lets the native host start its final pass now, before
      // the import and job writes below; a canceled one must not decode at all.
      const draining = this.capture.stopAndDrain(!this.canceled);
      await this.stopMeter();
      await draining;
      const metadata = await this.metadata(id);
      if (!this.current(id, generation)) return;
      const saved = !!metadata?.byte_length;
      if (this.capture.snapshot.error) { this.error('capture_or_storage_failed', saved); return; }
      if (this.canceled) {
        this.waitingForSettings.delete(id);
        // Quitting also cancels, and must keep the audio; only the explicit
        // discard gesture deletes it.
        const discarded = this.discarding && !this.undoable.has(id) && await this.discardOnce(id);
        if (this.discarding && !discarded) this.queueDiscard(id);
        if (this.current(id, generation)) this.publish('canceled', { audio_saved: saved && !discarded });
        return;
      }
      if (!saved) { this.error('audio_missing', false); return; }
      this.publish('saved', { audio_saved: true });
      const pending = this.waitingForSettings.get(id);
      if (pending) { pending.metadata = metadata!; this.submitting.add(id); }
      else if (this.settings?.options) void this.submit(metadata!, this.autoKey);
    } catch { if (this.current(id, generation)) this.error('storage_failed'); }
  }
  private async metadata(id: string): Promise<LocalSession | undefined> {
    const metadata = await this.capture.store.get(id);
    return metadata && { ...metadata, capture_state: metadata.capture_state === 'capturing' ? 'interrupted' : metadata.capture_state };
  }
  private async retry(id: string, generation: number, key: string) {
    const current = () => !this.disposed && generation === this.generation;
    try {
      const metadata = await this.metadata(id);
      if (!current()) return;
      if (!metadata?.byte_length) {
        this.publish('error', { session_id: id, job_id: null, audio_saved: false, error_code: 'audio_missing' }, true); return;
      }
      this.publish('saved', { session_id: id, job_id: null, audio_saved: true }, true);
      await this.submit(metadata, key);
    } catch {
      if (current()) this.publish('error', { session_id: id, job_id: null, audio_saved: false, error_code: 'storage_failed' }, true);
    } finally { this.submitting.delete(id); }
  }
  private async submit(metadata: LocalSession, key: string, settings = this.settings) {
    this.submitting.add(metadata.id);
    try { await this.jobs.run(metadata, settings?.options ?? null, () => this.capture.exportRecording(metadata.id), key); }
    finally { this.submitting.delete(metadata.id); }
  }
  private observeJob(id: string, job: Job | undefined, error?: string) {
    if (this.disposed) return;
    if (id !== this.value.session_id) { this.observeDetached(id, job, error); return; }
    if (this.canceled || ['requesting', 'capturing', 'saving'].includes(this.value.state)) return;
    if (error) { this.error(error); return; }
    if (!job || this.processing.has(id) && this.delivered.has(job.id)) return;
    const state = job.status.state;
    const insert = state === 'succeeded' && this.ready && id === this.liveSession && !this.delivered.has(job.id);
    // An API poll is a full authoritative resynchronization and can legitimately
    // skip queued/transcribing. Publish only its persisted real status/result.
    // A live dictation is not finished until its text is in place, so it stays
    // "transcribing" -- and the panel stays up -- until insertion has returned.
    this.publish(insert ? 'transcribing' : state === 'succeeded' ? 'success' : state === 'failed' ? 'error' : state,
      { job_id: job.id, audio_saved: true, error_code: state === 'failed' ? job.status.error.code : null,
        progress: insert ? 'processing_text' : recognitionProgress(job) }, true);
    if (insert) this.deliver(id, job);
  }
  private deliver(id: string, job: Job) {
    this.delivered.add(job.id);
    this.processing.add(id);
    const generation = this.generation;
    this.deliveryValue = { copied: false, pasted: false, message: 'Processing the captured mode…' };
    this.deliveryListeners.forEach(f => f());
    // Wanted while it is the dictation on screen and not discarded, and still
    // wanted once a newer dictation has taken the screen over from it.
    const wanted = () => this.detached.has(id) || this.current(id, generation) && !this.canceled;
    const run = this.dependencies.deliver ?? ((completed: Job) => deliverTranscript(completed, wanted));
    const settle = (result: DeliveryResult) => {
      if (!this.current(id, generation)) return;
      if (!this.canceled) this.publish('success', {}, true);
      this.deliveryValue = result; this.deliveryListeners.forEach(f => f());
    };
    void run(job)
      .then(settle, () => settle({ copied: false, pasted: false, message: 'Text is saved. Automatic delivery failed; use Copy on the transcript.' }))
      .finally(() => { this.processing.delete(id); this.settleDetached(id); });
  }
  /** An earlier dictation finishing behind a newer one: its text is inserted, and the panel is left alone. */
  private observeDetached(id: string, job: Job | undefined, error?: string) {
    const later = this.detached.get(id);
    if (!later) return;
    if (error) {
      // A failed poll of a running job may still recover; a failed submission has
      // no job to wait for. Either way the failure stays in history.
      const running = this.jobs.get(id)?.job;
      if (!running || !['queued', 'transcribing'].includes(running.status.state)) this.settleDetached(id);
      return;
    }
    if (!job || this.processing.has(id) || ['queued', 'transcribing'].includes(job.status.state)) return;
    if (job.status.state !== 'succeeded' || !later.live || !this.ready || this.delivered.has(job.id)) { this.settleDetached(id); return; }
    this.delivered.add(job.id);
    this.processing.add(id);
    const run = this.dependencies.deliver ?? ((completed: Job) => deliverTranscript(completed, () => this.detached.has(id)));
    void run(job).catch(() => undefined).finally(() => { this.processing.delete(id); this.settleDetached(id); });
  }
  async stopAndDrain() {
    if (this.warming()) this.publish('capturing');
    if (this.value.state === 'requesting') {
      this.canceled = true; this.waitingForSettings.delete(this.value.session_id!); ++this.generation; this.opened = false; this.capture.stop(); this.publish('canceled');
    } else if (this.value.state === 'capturing' || this.value.state === 'saving') {
      this.canceled = true; await this.stop();
    }
    await this.stopMeter();
  }
  async dispose() {
    if (this.disposed) return;
    await this.stopAndDrain();
    this.disposed = true; ++this.generation; clearTimeout(this.discardTimer); this.discards.clear(); this.waitingForSettings.clear(); this.detached.clear(); this.queuedStarts.clear(); delete window.writerCanDeleteHistory;
    this.unsubscribe(); this.unsubscribeLevel?.(); this.capture.dispose(); this.jobs.dispose(); this.listeners.clear(); this.previewListeners.clear(); this.deliveryListeners.clear(); this.waitingListeners.clear();
  }
}
