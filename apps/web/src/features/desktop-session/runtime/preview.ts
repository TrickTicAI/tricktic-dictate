import { apiFetch } from '../../auth/session';
import type { TranscriptionOptions } from '@metrix-writer/contracts';
import { isTauri, listen, invoke } from './native';
/** One speculative request at a time. Preview failure never controls capture,
 * final submission, delivery, or the durable raw transcript. */
export class LocalPreview {
  private abort?: AbortController;
  private timer?: ReturnType<typeof setTimeout>;
  private generation = 0;
  private unlisten?: () => void;
  constructor(private changed: (text: string) => void, private request = requestPreview, private enabled = isTauri) {}
  start(id: string, options: TranscriptionOptions | null) {
    this.stop(); this.changed('');
    if (!this.enabled() || options?.selection.provider !== 'local') return;
    const inferenceOptions: TranscriptionOptions = { ...structuredClone(options), cleanup: { mode: 'off' } };
    const generation = this.generation;
    if (isTauri() && this.request === requestPreview) {
      void (async () => {
        try {
          const off = await listen<{ id: string; text: string }>('writer-preview', value => {
            if (generation === this.generation && value.id === id) this.changed(value.text);
          });
          if (generation !== this.generation) { off(); return; }
          this.unlisten = off;
          await invoke('capture_preview_start', { id, options: inferenceOptions });
        } catch { /* Final transcription remains independent of speculative work. */ }
      })();
      return;
    }
    let busy = false;
    let next = Date.now() + 2000;
    const poll = async () => {
      if (generation !== this.generation || busy || Date.now() < next) return;
      busy = true; clearTimeout(this.timer);
      this.abort = new AbortController();
      try {
        const text = await this.request(id, inferenceOptions, this.abort.signal);
        if (generation === this.generation) this.changed(text);
      } catch { /* Missing/busy local model or preview ceiling: retain ordinary final inference. */ }
      busy = false; next = Date.now() + 500;
      if (generation === this.generation) this.timer = setTimeout(() => void poll(), 500);
    };
    // Native status events keep inference moving while the main WebView is hidden.
    void Promise.resolve().then(() => listen<{ id: string; active: boolean }>('writer-capture-status', status => {
      if (status.id === id && status.active) void poll();
    })).then(unlisten => {
      if (generation !== this.generation) unlisten(); else this.unlisten = unlisten;
    }).catch(() => { /* Timer fallback for older hosts. */ });
    this.timer = setTimeout(() => void poll(), 2000);
  }
  stop() { ++this.generation; clearTimeout(this.timer); this.abort?.abort(); this.abort = undefined; this.unlisten?.(); this.unlisten = undefined; }
}
async function requestPreview(id: string, options: TranscriptionOptions, signal: AbortSignal): Promise<string> {
  if (isTauri()) {
    const text = await invoke<string>('capture_preview', { id, options });
    if (signal.aborted) throw signal.reason;
    return text;
  }
  const snapshot = await fetch(`/native/preview-audio/${id}`, { signal });
  if (!snapshot.ok) throw new Error('Preview unavailable');
  const audio = await snapshot.blob();
  if (audio.size > 64 * 1024 * 1024) throw new Error('Preview size limit');
  const body = new FormData();
  body.append('request', JSON.stringify({ recording_id: id, options })); body.append('audio', audio, 'preview.wav');
  const response = await apiFetch('/api/v1/transcriptions/preview', { method: 'POST', body, signal });
  if (response.status !== 202) throw new Error('Preview unavailable');
  const task = await response.json() as { id?: unknown };
  if (typeof task.id !== 'string') throw new Error('Invalid preview');
  let result: { text?: unknown; provisional?: boolean };
  for (;;) {
    await waitForStatus(signal);
    const status = await apiFetch(`/api/v1/transcriptions/preview/${encodeURIComponent(task.id)}`, { signal });
    if (status.status === 202) continue;
    if (!status.ok) throw new Error('Preview unavailable');
    result = await status.json() as typeof result; break;
  }
  if (result.provisional !== true || typeof result.text !== 'string') throw new Error('Invalid preview');
  return result.text;
}

/** Native events also wake status checks: a completed preview must not remain
 * blocked on a throttled background timer while holding up the next segment. */
function waitForStatus(signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) { reject(signal.reason); return; }
    const deadline = Date.now() + 250;
    let settled = false;
    let unlisten: (() => void) | undefined;
    const finish = () => {
      if (settled) return;
      settled = true; clearTimeout(timer); unlisten?.(); signal.removeEventListener('abort', finish);
      if (signal.aborted) reject(signal.reason); else resolve();
    };
    const timer = setTimeout(finish, 250);
    signal.addEventListener('abort', finish, { once: true });
    void Promise.resolve().then(() => listen('writer-capture-status', () => {
      if (Date.now() >= deadline) finish();
    })).then(off => { if (settled) off(); else unlisten = off; }).catch(() => {});
  });
}
