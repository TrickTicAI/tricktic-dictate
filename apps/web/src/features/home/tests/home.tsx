import '@fontsource/inter/400.css';
import '@fontsource/inter/500.css';
import '@fontsource/inter/600.css';
import '@fontsource/assistant/400.css';
import '@fontsource/assistant/600.css';
import { useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import type { InputLanguage, Job, Settings } from '@metrix-writer/contracts';
import type { SessionOwner } from '../../desktop-session/runtime/owner';
import type { Snapshot, SessionState } from '../../desktop-session/contracts/protocol';
import type { DeliveryResult } from '../../desktop-session/runtime/delivery';
import type { Recording } from '../../../recorder';
import { Home } from '../Home';
import { TopBar } from '../../shell/TopBar';
import { MenuHost } from '../../shell/menu';
import { applyTheme } from '../../shell/theme';
import { themeById } from '../../../themes';
import '../../../styles.css';

/*
 * The real Home surface over fabricated sessions.
 *
 * Everything the page reaches for outside itself — the vocabulary document, the
 * processed text, modes, cleanup providers — is routed by the spec, so this
 * harness only has to supply what the workspace normally supplies: recordings,
 * saved jobs, the settings snapshot and a session owner. The owner here is a
 * script: `window.session(...)` moves it through the states a real dictation
 * goes through, so the dock and the live stage can be driven without audio.
 */

const base = new Date('2026-09-11T10:42:00');

function recording(id: string, language: InputLanguage, minutes: number, durationMs: number, state: 'complete' | 'interrupted' = 'complete'): Recording {
  const created = new Date(base.getTime() - minutes * 60_000);
  return {
    id,
    url: '',
    filename: `metrix-writer-${id}.webm`,
    bytes: 40_000,
    duration: durationMs,
    language,
    createdAt: created,
    downloaded: false,
    metadata: {
      id,
      byte_length: 40_000,
      capture_state: state,
      created_at_ms: created.getTime(),
      duration_ms: durationMs,
      input_language: language,
      mime_type: 'audio/webm;codecs=opus',
    },
  };
}

function succeeded(id: string, text: string, language: InputLanguage): Job {
  return {
    id: `job-${id}`,
    created_at_ms: base.getTime(),
    updated_at_ms: base.getTime(),
    request: {
      idempotency_key: `key-${id}`,
      recording_id: id,
      options: {
        cleanup: { mode: 'off' },
        input_language: language,
        output_language: { mode: 'preserve' },
        selection: { provider: 'local', model: 'ivrit-ai/whisper-large-v3' },
        vocabulary: { hints: [], corrections: [], snippets: [] },
      },
    },
    status: {
      state: 'succeeded',
      result: {
        cleanup: { state: 'off' },
        transcript: {
          execution_location: 'self_hosted_server',
          input_language: language,
          output_language: { mode: 'preserve' },
          raw_text: text,
          selection: { provider: 'local', model: 'ivrit-ai/whisper-large-v3' },
        },
      },
    },
  };
}

function failed(id: string): Job {
  return {
    ...succeeded(id, '', 'en'),
    status: { state: 'failed', error: { code: 'provider_unavailable', message: 'The local model worker did not answer.', retryable: true } },
  };
}

const settings: Settings = {
  ui_language: 'en',
  transcription: {
    selection: { provider: 'local', model: 'ivrit-ai/whisper-large-v3' },
    input_language: 'en',
    output_language: { mode: 'preserve' },
    vocabulary: { hints: [], corrections: [], snippets: [] },
    cleanup: { mode: 'off' },
  },
};

/* A 44-byte silent WAV, so the player mounts and a play request is real. */
const SILENT = 'data:audio/wav;base64,UklGRiQAAABXQVZFZm10IBAAAAABAAEAESsAACJWAAACABAAZGF0YQAAAAA=';

const ENGLISH = 'The quick brown fox jumps over the lazy dog, and then keeps going for a while longer so the row has something to clamp.';
const HEBREW = 'שלום, זאת הקלטה בעברית כדי לבדוק שהשורה מסתדרת מימין לשמאל כמו שצריך.';

/** What the spec can move: the owner's state, the live words, the outcome. */
interface SessionUpdate {
  state: SessionState;
  id?: string;
  text?: string;
  progress?: Snapshot['progress'];
  delivery?: DeliveryResult;
  empty?: boolean;
}

declare global {
  interface Window {
    openModels: number;
    started: number;
    discarded: number;
    kept: string[];
    session(update: SessionUpdate): void;
  }
}
window.openModels = 0;
window.started = 0;
window.discarded = 0;
window.kept = [];

/** A scripted stand-in for the session owner, with the surface Home reads. */
function fakeOwner() {
  let snapshot: Snapshot = { version: 1, kind: 'snapshot', epoch: 'harness', revision: 0, session_id: null, job_id: null, state: 'idle', audio_saved: false, error_code: null };
  let preview = '';
  let delivery: DeliveryResult | undefined;
  const listeners = new Set<(event: Snapshot) => void>();
  const previewListeners = new Set<() => void>();
  const deliveryListeners = new Set<() => void>();
  const owner = {
    get snapshot() { return { ...snapshot }; },
    get preview() { return preview; },
    get delivery() { return delivery && { ...delivery }; },
    subscribe(listener: (event: Snapshot) => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
    subscribePreview(listener: () => void) { previewListeners.add(listener); return () => { previewListeners.delete(listener); }; },
    subscribeDelivery(listener: () => void) { deliveryListeners.add(listener); return () => { deliveryListeners.delete(listener); }; },
    async discardWithUndo() { window.discarded += 1; owner.move({ state: 'canceled' }); return { outcome: 'accepted' }; },
    keep(id: string) { window.kept.push(id); return true; },
    move(update: SessionUpdate) {
      snapshot = { ...snapshot, revision: snapshot.revision + 1, state: update.state, session_id: update.id ?? snapshot.session_id, progress: update.progress ?? null };
      if (update.text !== undefined) { preview = update.text; previewListeners.forEach((listener) => listener()); }
      listeners.forEach((listener) => listener({ ...snapshot }));
      if (update.delivery) { delivery = update.delivery; deliveryListeners.forEach((listener) => listener()); }
    },
  };
  return owner;
}

const captureFor = (state: SessionState): 'idle' | 'requesting' | 'recording' | 'stopping' =>
  state === 'requesting' ? 'requesting' : state === 'capturing' ? 'recording' : state === 'saving' ? 'stopping' : 'idle';

function Harness() {
  const params = new URLSearchParams(location.search);
  const [selectedId, setSelectedId] = useState<string | null>('a');
  const [transcribed, setTranscribed] = useState<string[]>([]);
  const owner = useMemo(fakeOwner, []);
  const [captureState, setCaptureState] = useState<'idle' | 'requesting' | 'recording' | 'stopping'>('idle');
  const [empty, setEmpty] = useState(params.has('empty'));
  window.session = (update) => {
    if (update.empty !== undefined) setEmpty(update.empty);
    setCaptureState(captureFor(update.state));
    owner.move(update);
  };
  if (params.get('state') === 'recording' && owner.snapshot.state === 'idle') queueMicrotask(() => window.session({ state: 'capturing', id: 'live', text: '' }));

  const recordings = useMemo(() => [
    recording('a', 'en', 0, 14_000),
    recording('b', 'he', 9, 23_000),
    recording('c', 'en', 40, 5_000, 'interrupted'),
    recording('d', 'en', 70, 8_000),
    // Saved audio becomes readable only for the session that is open, exactly
    // as the recorder loads one blob at a time.
  ].map((item) => ({ ...item, url: item.id === selectedId ? SILENT : '' })), [selectedId]);

  const saved = useMemo(() => ({
    a: params.has('failedRetry') ? { job: { ...failed('a'), id: 'job-retry-a' }, history: [succeeded('a', ENGLISH, 'en')] } : { job: succeeded('a', ENGLISH, 'en'), history: [] },
    b: { job: succeeded('b', HEBREW, 'he'), history: [] },
    c: { job: failed('c'), history: [] },
  }), []);

  return (
    /* The workspace frame: an optional top bar, then one full-height row
       holding the page, so the columns and scrollers behave as in the app. */
    <div className="shell" style={params.has('frame') ? undefined : { gridTemplateRows: 'minmax(0, 1fr)' }}>
      {params.has('frame') && <TopBar tab="home" settingsOpen={false} onTab={() => {}} onSettings={() => {}} onStatusAction={() => {}} onEngineSettings={() => {}} status={{ tone: 'online', title: 'Ready', detail: 'Local model ready', action: 'Open models' }} />}
      <div className="shell-body">
        <Home
          capture={{
            state: captureState,
            recordings: empty ? [] : recordings,
            selectedId,
            error: null,
            elapsed: 14_000,
            supported: true,
            loading: false,
            start: () => { window.started += 1; },
            stop: () => {},
            select: (id) => setSelectedId(id),
            exportRecording: async () => new Blob(['audio'], { type: 'audio/webm' }),
            markDownloaded: () => {},
          }}
          jobs={{
            saved,
            errors: {},
            working: {},
            transcribe: (item) => setTranscribed((list) => [...list, item.id]),
            updateJob: () => {},
          }}
          owner={owner as unknown as SessionOwner}
          onCancel={() => {}}
          settings={settings}
          integrated={params.get('integrated') !== '0'}
          busy={false}
          usable
          shortcut="Ctrl+Shift+D"
          onOpenModels={() => { window.openModels += 1; }}
          onOpenSettings={() => {}}
          providers={[{ id: 'local', name: 'Local model', configured: true, available: true, models: ['ivrit-ai/whisper-large-v3'] }]}
        />
      </div>
      <MenuHost />
      <p data-testid="transcribed" className="visually-hidden">{transcribed.join(',')}</p>
    </div>
  );
}

applyTheme(themeById(new URLSearchParams(location.search).get('theme') ?? 'paper'));
createRoot(document.getElementById('root')!).render(<Harness />);
