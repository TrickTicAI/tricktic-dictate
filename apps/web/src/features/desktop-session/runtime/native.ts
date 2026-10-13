import type { InputLanguage } from '@metrix-writer/contracts';
import type { CaptureSnapshot } from '../../recorder/capture';
import type { LocalSession, RecordingStore } from '../../recorder/storage';
import type { CaptureController } from '../../recorder/capture';
import { PRODUCT_NAME } from '../../../product';

type NativeStatus = { active: boolean; id: string | null; elapsed: number; saved_bytes: number; error: string | null; rms: number; peak: number; level_sequence: number; listening?: boolean };
type Tauri = { core: { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> }; event: { listen<T>(name: string, cb: (event: { payload: T }) => void): Promise<() => void> } };
declare global { interface Window { __TAURI__?: Tauri } }
export const isTauri = () => !!window.__TAURI__ && ['http://tauri.localhost', 'https://tauri.localhost', 'tauri://localhost'].includes(location.origin);
export function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) return Promise.reject(new Error('Native host unavailable.'));
  return window.__TAURI__!.core.invoke<T>(command, args);
}
export function listen<T>(name: string, cb: (payload: T) => void) { return window.__TAURI__!.event.listen<T>(name, e => cb(e.payload)); }
class NativeStore implements RecordingStore {
  create(): Promise<void> { return Promise.reject(new Error('Native capture owns creation.')); }
  append(): Promise<void> { return Promise.reject(new Error('Native capture owns audio writes.')); }
  finish(): Promise<void> { return Promise.reject(new Error('Native capture owns finalization.')); }
  async get(id: string) { return (await invoke<LocalSession | null>('capture_get', { id })) ?? undefined; }
  list() { return invoke<LocalSession[]>('capture_list'); }
  async export(id: string) { const response = await fetch(`/native/audio/${id}`); if (!response.ok) throw new Error('Saved audio unavailable.'); return response.blob(); }
  delete(id: string) { return invoke<void>('capture_delete', { id }); }
}
/** The UI sees the same capture interface. PCM and the microphone never enter JS. */
export class NativeCapture implements CaptureController {
  readonly store = new NativeStore();
  snapshot: CaptureSnapshot = { state: 'idle', error: null, elapsed: 0 };
  readonly supported = true;
  readonly activeStream = undefined;
  private listeners = new Set<() => void>();
  private levels = new Set<(rms: number, peak: number) => void>();
  private timer?: ReturnType<typeof setTimeout>;
  private starting?: Promise<void>;
  private stopping?: Promise<void>;
  private stopRequested = false;
  /** capture_start has not returned: the host may still close the device and remove it. */
  private opening = false;
  private disposed = false;
  private activeId?: string;
  private levelSequence = -1;
  private unlistenStatus?: () => void;
  private listeningStatus?: Promise<void>;
  subscribe = (cb: () => void) => { this.listeners.add(cb); return () => { this.listeners.delete(cb); }; };
  subscribeLevel = (cb: (rms: number, peak: number) => void) => { this.levels.add(cb); return () => { this.levels.delete(cb); }; };
  private update(change: Partial<CaptureSnapshot>) { this.snapshot = { ...this.snapshot, ...change }; this.listeners.forEach(cb => cb()); }
  get elapsed() { return this.snapshot.elapsed; }
  start(language: InputLanguage, id: string = crypto.randomUUID()) {
    if (this.snapshot.state !== 'idle' || this.disposed) return Promise.resolve();
    this.stopRequested = false; this.activeId = id; this.levelSequence = -1; this.opening = true;
    this.update({ state: 'requesting', error: null, elapsed: 0, savedBytes: 0, listening: false });
    this.starting = this.ensureStatusEvents().then(() => invoke<NativeStatus>('capture_start', { id, language })).finally(() => { this.opening = false; }).then(async status => {
      if (this.stopRequested || this.disposed) { this.apply(await invoke<NativeStatus>('capture_stop', { transcribe: false })); return; }
      this.apply(status); this.poll();
    }).catch(error => { this.update({ state: 'idle', error: typeof error === 'string' ? error : error instanceof Error ? error.message : 'Cannot start the native microphone. Check system permissions, input device and disk space.' }); }).finally(() => { this.starting = undefined; });
    return this.starting;
  }
  private ensureStatusEvents(): Promise<void> {
    if (this.unlistenStatus) return Promise.resolve();
    if (this.listeningStatus) return this.listeningStatus;
    this.listeningStatus = Promise.resolve().then(() => listen<NativeStatus>('writer-capture-status', status => {
      if (this.disposed || this.stopRequested || this.opening || !this.activeId || status.id !== this.activeId) return;
      this.apply(status);
    })).then(unlisten => {
      if (this.disposed) unlisten(); else this.unlistenStatus = unlisten;
    }).catch(() => { /* Older hosts retain polling; event setup cannot prevent recording. */ })
      .finally(() => { this.listeningStatus = undefined; });
    return this.listeningStatus;
  }
  private apply(status: NativeStatus) {
    // Older hosts do not report `listening`; their open microphone counts as heard.
    this.update({ state: status.active ? 'recording' : 'idle', error: status.error, elapsed: status.elapsed, savedBytes: status.saved_bytes,
      listening: status.active && status.listening !== false, recordingId: status.id ?? undefined });
    if (status.active && status.level_sequence > 0 && status.level_sequence > this.levelSequence) { this.levelSequence = status.level_sequence; this.levels.forEach(cb => cb(status.rms, status.peak)); }
  }
  private poll() {
    if (this.disposed || this.stopRequested || this.snapshot.state !== 'recording') return;
    const id = this.activeId;
    this.timer = setTimeout(() => { void invoke<NativeStatus>('capture_status').then(s => {
      if (this.disposed || this.stopRequested || id !== this.activeId || s.id !== id) return;
      this.apply(s); this.poll();
    }).catch(() => { if (id === this.activeId && !this.stopRequested) void this.stopAndDrain(); }); }, 100);
  }
  stop() { void this.stopAndDrain(false); }
  stopAndDrain(transcribe = false): Promise<void> {
    if (this.stopping) return this.stopping;
    this.stopRequested = true; clearTimeout(this.timer);
    this.stopping = Promise.resolve().then(async () => {
      await this.starting;
      if (this.snapshot.state === 'idle') return;
      this.update({ state: 'stopping' });
      try { this.apply(await invoke<NativeStatus>('capture_stop', { transcribe })); }
      catch { this.update({ state: 'idle', error: `Native capture could not confirm its final save. Relaunch ${PRODUCT_NAME} to recover retained audio.` }); }
    }).finally(() => { this.stopping = undefined; });
    return this.stopping;
  }
  list() { return this.store.list(); }
  exportRecording(id: string) { return this.store.export(id); }
  deleteRecording(id: string) { if (this.snapshot.state !== 'idle' && id === this.activeId) return Promise.reject(new Error('Stop recording first.')); return this.store.delete(id); }
  dispose() { this.disposed = true; this.unlistenStatus?.(); this.unlistenStatus = undefined; void this.stopAndDrain(); this.listeners.clear(); this.levels.clear(); }
}
