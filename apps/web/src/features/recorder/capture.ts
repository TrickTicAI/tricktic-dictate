import type { InputLanguage } from '@metrix-writer/contracts';
import { microphoneConstraints } from './microphones';
import { IndexedRecordingStore, type LocalSession, type RecordingStore } from './storage';

export type CapturePhase = 'idle' | 'requesting' | 'recording' | 'stopping';
/** `listening` false: the device is open and recording but has sent only digital
 * silence so far, as a Bluetooth headset does while it switches to its call
 * profile. Absent means listening. */
export type CaptureSnapshot = { state: CapturePhase; error: string | null; elapsed: number; savedBytes?: number; listening?: boolean; recordingId?: string };
const formats = ['audio/webm;codecs=opus', 'audio/webm', 'audio/mp4', 'audio/ogg;codecs=opus'];
const maxPendingBytes = 8 * 1024 * 1024;
const maxPendingChunks = 8;

export interface CaptureController {
  readonly store: RecordingStore;
  readonly snapshot: CaptureSnapshot;
  readonly supported: boolean;
  readonly elapsed: number;
  readonly activeStream: MediaStream | undefined;
  subscribe(listener: () => void): () => void;
  subscribeLevel?(listener: (rms: number, peak: number) => void): () => void;
  start(language: InputLanguage, id?: string): Promise<void>;
  stop(): void;
  /** `transcribe` tells a native host whether the final pass may start at once;
   * a canceled or discarded dictation passes false. Browser capture ignores it. */
  stopAndDrain(transcribe?: boolean): Promise<void>;
  list(limit?: number): Promise<LocalSession[]>;
  exportRecording(id: string): Promise<Blob>;
  deleteRecording(id: string): Promise<void>;
  dispose(): void;
}

export class DurableCapture {
  snapshot: CaptureSnapshot = { state: 'idle', error: null, elapsed: 0 };
  private listeners = new Set<() => void>();
  private recorder?: MediaRecorder;
  private stream?: MediaStream;
  private active?: LocalSession;
  private started = 0;
  private savedBytes = 0;
  private queue = Promise.resolve();
  private pendingBytes = 0;
  private pendingChunks = 0;
  private failed = false;
  private writesFailed = false;
  private finalized = false;
  private disposed = false;
  private requestVersion = 0;
  private timer?: ReturnType<typeof setTimeout>;
  constructor(readonly store: RecordingStore = new IndexedRecordingStore()) {}
  get supported() {
    return window.isSecureContext && !!navigator.mediaDevices?.getUserMedia && typeof MediaRecorder !== 'undefined';
  }
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(values: Partial<CaptureSnapshot>) {
    this.snapshot = { ...this.snapshot, ...values };
    this.listeners.forEach((listener) => listener());
  }
  private fail(message: string) { this.failed = true; this.update({ error: message }); }
  private storageFailure() {
    this.writesFailed = true;
    this.fail('Recording storage could not keep up or is full. Capture stopped; previously saved audio is retained. Export it, free browser storage, then retry. The final audio may be missing.');
    this.stop();
  }
  // Desktop callers reserve the recording UUID before permission. Browser-only
  // callers retain automatic allocation; storage.add prevents reopening an ID.
  async start(language: InputLanguage, id: string = crypto.randomUUID()) {
    if (this.snapshot.state !== 'idle' || this.disposed) return;
    if (!this.supported) { this.update({ error: 'Microphone capture requires a supported browser on localhost or HTTPS.' }); return; }
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id)) throw new Error('invalid_session_id');
    const requestVersion = ++this.requestVersion;
    this.update({ state: 'requesting', error: null, elapsed: 0, savedBytes: 0 });
    this.failed = false;
    this.writesFailed = false;
    this.finalized = false;
    this.queue = Promise.resolve();
    this.pendingBytes = 0;
    this.pendingChunks = 0;
    this.savedBytes = 0;
    try {
      const mimeType = formats.find((format) => MediaRecorder.isTypeSupported(format));
      if (!mimeType) {
        this.update({ error: 'No supported audio recording format was found. Try a current Chrome, Edge, Firefox or Safari browser.', state: 'idle' });
        return;
      }
      const stream = await navigator.mediaDevices.getUserMedia(microphoneConstraints());
      if (this.disposed || requestVersion !== this.requestVersion) { stream.getTracks().forEach((track) => track.stop()); return; }
      this.stream = stream;
      const recorder = new MediaRecorder(stream, { mimeType });
      this.recorder = recorder;
      this.started = Date.now();
      this.active = { id, created_at_ms: this.started, input_language: language,
        duration_ms: 0, byte_length: 0, mime_type: recorder.mimeType || mimeType, capture_state: 'capturing', chunk_count: 0 };
      const session = this.active;
      try { await this.store.create(session); }
      catch { if (requestVersion === this.requestVersion) this.storageFailure(); throw new Error('storage'); }
      if (this.disposed || requestVersion !== this.requestVersion) {
        stream.getTracks().forEach((track) => track.stop());
        await this.store.finish(session.id, 'interrupted', 0);
        return;
      }
      recorder.ondataavailable = ({ data }) => {
        if (this.recorder !== recorder || !data.size || this.finalized || this.writesFailed) return;
        if (this.pendingBytes + data.size > maxPendingBytes || this.pendingChunks >= maxPendingChunks) {
          this.storageFailure(); return;
        }
        this.pendingBytes += data.size;
        this.pendingChunks++;
        const id = this.active!.id;
        const duration = Date.now() - this.started;
        this.queue = this.queue.then(async () => {
          // Stop accepting later chunks after a failed write, preserving a contiguous prefix.
          if (!this.writesFailed) { await this.store.append(id, data, duration); this.savedBytes += data.size; this.update({ savedBytes: this.savedBytes }); }
        }).catch(() => this.storageFailure()).finally(() => {
          this.pendingBytes -= data.size;
          this.pendingChunks--;
        });
      };
      recorder.onerror = () => {
        if (this.recorder !== recorder) return;
        this.fail('Recording was interrupted by the microphone or browser. Saved audio is retained; export it and check the input device before retrying.');
        this.stop();
      };
      recorder.onstop = () => { if (this.recorder === recorder) void this.finalize(); };
      stream.getAudioTracks().forEach((track) => track.addEventListener('ended', () => {
        if (this.recorder === recorder && this.snapshot.state === 'recording') {
          this.fail('The microphone disconnected. Saved audio is retained; reconnect the input and retry.');
          this.stop();
        }
      }));
      recorder.start(1000);
      this.update({ state: 'recording' });
    } catch (error) {
      if (requestVersion !== this.requestVersion) return;
      clearTimeout(this.timer); this.timer = undefined;
      this.stream?.getTracks().forEach((track) => track.stop());
      this.stream = undefined;
      this.recorder = undefined;
      if (this.active) {
        try { await this.store.finish(this.active.id, 'interrupted', Date.now() - this.started); } catch { /* saved prefix remains recoverable */ }
      }
      this.active = undefined;
      const denied = error instanceof DOMException && error.name === 'NotAllowedError';
      const missing = error instanceof DOMException && ['NotFoundError', 'OverconstrainedError'].includes(error.name);
      this.update({ state: 'idle', error: this.snapshot.error ?? (denied
        ? 'Microphone access was denied. Allow microphone access in your browser, then try again.'
        : missing ? 'The selected microphone is unavailable. Reconnect it or choose an input in Settings.'
        : 'Could not start the microphone. Check that a working input device is available, then retry.') });
    }
  }
  stop() {
    if (this.snapshot.state === 'requesting') {
      this.requestVersion++;
      this.stream?.getTracks().forEach((track) => track.stop());
      this.stream = undefined;
      this.recorder = undefined;
      this.active = undefined;
      this.update({ state: 'idle' });
      return;
    }
    if (!this.recorder || this.finalized) return;
    this.update({ state: 'stopping' });
    // Some implementations never deliver stop after device interruption.
    if (!this.timer) this.timer = setTimeout(() => {
      this.fail('The browser did not finish stopping. Saved audio is retained; the final chunk may be missing. Retry with a working microphone.');
      void this.finalize();
    }, 5000);
    try {
      if (this.recorder.state !== 'inactive') this.recorder.stop();
    } catch {
      this.fail('The browser could not stop normally. Saved audio is retained; the final chunk may be missing.');
      void this.finalize();
    }
  }
  private async finalize() {
    if (this.finalized) return;
    this.finalized = true;
    clearTimeout(this.timer); this.timer = undefined;
    this.stream?.getTracks().forEach((track) => track.stop());
    await this.queue;
    const elapsed = Date.now() - this.started;
    if (!this.savedBytes && !this.failed) this.fail('The microphone returned no saved audio. Check the selected input and try again.');
    if (this.active) {
      try { await this.store.finish(this.active.id, this.failed || this.disposed ? 'interrupted' : 'complete', elapsed); }
      catch { this.storageFailure(); }
    }
    this.active = undefined;
    this.recorder = undefined;
    this.stream = undefined;
    this.update({ state: 'idle', elapsed });
  }
  // The owner meters this exact stream; observers never call getUserMedia.
  get elapsed() { return this.snapshot.state === 'recording' || this.snapshot.state === 'stopping' ? Date.now() - this.started : this.snapshot.elapsed; }
  get activeStream() { return this.snapshot.state === 'recording' ? this.stream : undefined; }
  async stopAndDrain() {
    if (this.snapshot.state === 'idle') return;
    await new Promise<void>((resolve) => {
      const unsubscribe = this.subscribe(() => {
        if (this.snapshot.state === 'idle') { unsubscribe(); resolve(); }
      });
      this.stop();
    });
  }
  async list(limit?: number) {
    return (await this.store.list(limit)).map((session) => ({ ...session,
      capture_state: session.capture_state === 'capturing' && session.id !== this.active?.id ? 'interrupted' as const : session.capture_state }));
  }
  exportRecording(id: string) { return this.store.export(id); }
  async deleteRecording(id: string) {
    if (id === this.active?.id) throw new Error('Stop recording before deleting its audio.');
    await this.store.delete(id);
  }
  dispose() {
    this.disposed = true;
    this.stop();
    this.stream?.getTracks().forEach((track) => track.stop());
    this.listeners.clear();
  }
}
