import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { AlertTriangle, ArrowUpRight, Check, ClipboardCheck, Copy, Download, Info, Play, RefreshCw, Trash2 } from 'lucide-react';
import type { Job, Settings } from '@metrix-writer/contracts';
import type { Recording } from '../../recorder';
import type { SessionOwner } from '../desktop-session/runtime/owner';
import { useCopyFeedback } from '../shell/clipboard';
import type { MenuInput } from '../shell/menu';
import { modKey } from '../shell/TopBar';
import { useEnumParam, useParam } from '../shell/urlState';
import { countWords, successfulJob } from './statistics';
import { Dock, type DockProvider } from './Dock';
import { EmptyHome } from './EmptyHome';
import { LiveStage } from './LiveStage';
import { FILTERS, SessionList, rowText, type SessionRow } from './SessionList';
import { modeText, useModeResults, loadingMode } from '../modes/useModeResults';
import { modeApi, type ModeProfile, type ModeSettings } from '../modes/api';
import type { TextChoice } from '../modes/useModeProviders';
import { request } from '../transcripts/api';
import { TranscriptReader } from './TranscriptReader';
import { useLevels } from './useLevels';
import { useDictationFlow, type DictationFlow } from './useDictationFlow';
import { hereLabel, languageLabel, modelName, shortModel, textModelName } from './language';
import { invoke, isTauri, listen } from '../desktop-session/runtime/native';
import { isMac } from '../settings/platform';
import { PRODUCT_SLUG } from '../../product';
import './home.css';
import { Reprocess } from './Reprocess';

type LiveOwner = SessionOwner & {
  preview?: string;
  subscribePreview?: (listener: () => void) => () => void;
};

const VIEWS = ['result', 'raw', 'corrected', 'cleaned'] as const;
/** How long a discarded dictation can still be brought back. */
const UNDO_MS = 5000;

/** Hands the caller a file without leaving anything mounted behind. */
function save(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = filename;
  link.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export interface HomeProps {
  capture: {
    state: 'idle' | 'requesting' | 'recording' | 'stopping';
    recordings: Recording[];
    selectedId: string | null;
    error: string | null;
    elapsed: number;
    supported: boolean;
    loading: boolean;
    stream?: MediaStream;
    start: () => void;
    stop: () => void;
    select: (id: string) => void;
    exportRecording: (id: string) => Promise<Blob>;
    markDownloaded: (id: string) => void;
  };
  jobs: {
    saved: Record<string, { job?: Job; history?: Job[] }>;
    errors: Record<string, string>;
    working: Record<string, boolean>;
    uploadProgress?: Record<string, string>;
    transcribe: (recording: Recording) => void;
    updateJob: (job: Job) => void;
  };
  settings: Settings;
  integrated: boolean;
  owner?: SessionOwner;
  busy: boolean;
  usable: boolean;
  unavailableReason?: string;
  /** A hard mismatch between the model and the requested languages. */
  capabilityIssue?: string;
  shortcut: string | null;
  /** The shortcut is pressed once to start and again to stop (rather than held). */
  toggle?: boolean;
  /** Discards the dictation in progress: no transcript, and the audio deleted after a short Undo window. */
  onCancel?: () => void;
  /** Opens Settings → Transcription, which is what the dock's engine switch names. */
  onOpenModels?: () => void;
  /** Opens Settings → Modes, from the dock's mode switch. */
  onOpenModes?: () => void;
  /** Opens any Settings section. */
  onOpenSettings?: (section: string) => void;
  /** Saves changed transcription settings (the dock's language and model switches). */
  onSettings?: (settings: Settings) => void;
  /** Providers the backend reports, for the dock's model switch. */
  providers?: DockProvider[];
}

interface Toast { id: number; text: string; icon: ReactNode; action?: { label: string; run: () => void } }

export function Home(props: HomeProps) {
  const { capture, jobs } = props;
  const [selected, setSelected] = useParam('session');
  const [query, setQuery] = useParam('q');
  const [view, setView] = useEnumParam('view', VIEWS, 'result');
  const [filter, setFilter] = useEnumParam('filter', FILTERS, 'all');
  const [showReader, setShowReader] = useState(false);
  const [playFor, setPlayFor] = useState<string | null>(null);
  const [fresh, setFresh] = useState<string | null>(null);
  const [toast, setToast] = useState<Toast | null>(null);
  const [modes, setModes] = useState<ModeSettings | null>(null);
  const [textChoices, setTextChoices] = useState<TextChoice[]>([]);
  const [accessibilityError, setAccessibilityError] = useState('');
  const search = useRef<HTMLInputElement>(null);
  const clipboard = useCopyFeedback();
  const liveOwner = props.owner as LiveOwner | undefined;
  const [preview, setPreview] = useState(liveOwner?.preview ?? '');
  useEffect(() => {
    setPreview(liveOwner?.preview ?? '');
    return liveOwner?.subscribePreview?.(() => setPreview(liveOwner.preview ?? ''));
  }, [liveOwner]);

  const flow = useDictationFlow({ owner: props.owner, captureState: capture.state, recordings: capture.recordings, saved: jobs.saved, working: jobs.working });
  const levels = useLevels(flow.flow === 'recording' || capture.state === 'recording', props.owner, capture.stream, 56);
  const live = flow.flow === 'opening' || flow.flow === 'recording' || flow.flow === 'transcribing' || flow.flow === 'processing';

  // The modes and which of them can run, read when Home opens and again as
  // each dictation starts, so a change made in Settings or the tray shows.
  const loadModes = useCallback(() => {
    void modeApi.settings().then(setModes).catch(() => { /* The dock keeps its last known mode. */ });
    void request<TextChoice[]>('/api/v1/modes/providers').then(setTextChoices).catch(() => { /* Treated as not ready. */ });
  }, []);
  useEffect(loadModes, [loadModes]);
  useEffect(() => { if (flow.flow === 'opening') loadModes(); }, [flow.flow, loadModes]);
  // The tray's Mode menu saves the active mode itself, then says so.
  useEffect(() => {
    if (!isTauri()) return;
    let stop: (() => void) | undefined;
    let gone = false;
    void listen('writer-modes-changed', loadModes)
      .then((unlisten) => { if (gone) unlisten(); else stop = unlisten; })
      .catch(() => { /* The dock still re-reads modes as each dictation starts. */ });
    return () => { gone = true; stop?.(); };
  }, [loadModes]);
  const modeReady = useCallback((profile: ModeProfile) => profile.kind === 'regular' || textChoices.some((choice) =>
    choice.configured && choice.selection.provider === profile.text_model?.provider && choice.selection.model === profile.text_model?.model), [textChoices]);
  const chooseMode = useCallback(async (id: string) => {
    try {
      const latest = await modeApi.settings();
      setModes(await modeApi.save({ ...latest, active: id }));
    } catch (error) {
      setToast({ id: Date.now(), icon: <AlertTriangle size={16} aria-hidden />, text: `Mode not changed: ${(error as Error).message}` });
    }
  }, []);
  const activeMode = modes?.profiles.find((profile) => profile.id === modes.active);

  const modeResults = useModeResults(capture.recordings.flatMap(recording => {
    const job = successfulJob(jobs.saved[recording.id]);
    return job ? [job.id] : [];
  }));
  // A row keeps its identity while nothing in it changed, so the list redraws
  // only the rows that did: a long history used to redraw in full for every
  // job update and every mode result.
  const rowCache = useRef(new Map<string, SessionRow>());
  const rows = useMemo<SessionRow[]>(() => {
    const kept = new Map<string, SessionRow>();
    const list = capture.recordings.map((recording) => {
      const saved = jobs.saved[recording.id];
      const success = successfulJob(saved);
      const next: SessionRow = {
        recording,
        job: saved?.job,
        successfulJob: success,
        working: !!jobs.working[recording.id],
        uploadProgress: jobs.uploadProgress?.[recording.id],
        mode: modeResults.results[success?.id ?? ''] ?? loadingMode,
      };
      const before = rowCache.current.get(recording.id);
      const row = before && before.recording === next.recording && before.job === next.job && before.successfulJob === next.successfulJob
        && before.uploadProgress === next.uploadProgress && before.working === next.working && before.mode === next.mode ? before : next;
      kept.set(recording.id, row);
      return row;
    });
    rowCache.current = kept;
    return list;
  }, [capture.recordings, jobs.saved, jobs.working, jobs.uploadProgress, modeResults.results]);

  // A session id in the URL that no longer exists falls back to the newest one,
  // so a stale link still opens something rather than an empty pane.
  const current = rows.find((row) => row.recording.id === selected) ?? rows[0];

  useEffect(() => {
    if (current && current.recording.id !== capture.selectedId) capture.select(current.recording.id);
    // `capture.select` reads the audio blob for playback; it is safe to repeat.
  }, [current?.recording.id, capture.selectedId]);

  // The dictation that just finished takes over the reader, and its row
  // slides into history — unless you opened another one while it was
  // finishing, in which case your choice stands.
  const picked = useRef(false);
  useEffect(() => { if (flow.flow === 'opening' || flow.flow === 'recording') picked.current = false; }, [flow.flow]);
  const select = useCallback((id: string) => {
    if (flow.flow !== 'idle') picked.current = true;
    setSelected(id);
  }, [flow.flow, setSelected]);
  useEffect(() => {
    if ((flow.flow !== 'done' && flow.flow !== 'failed') || !flow.sessionId) return;
    if (!picked.current) { setSelected(flow.sessionId); setView('result'); }
    setFresh(flow.sessionId);
  }, [flow.flow, flow.sessionId]);
  useEffect(() => {
    if (!fresh) return;
    const timer = setTimeout(() => setFresh(null), 2400);
    return () => clearTimeout(timer);
  }, [fresh]);

  // Ctrl/⌘ F searches what you said.
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.shiftKey || event.altKey || event.key.toLowerCase() !== 'f') return;
      if (document.querySelector('[aria-modal="true"]')) return;
      event.preventDefault();
      setShowReader(false);
      search.current?.focus();
      search.current?.select();
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, []);

  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast((value) => value?.id === toast.id ? null : value), toast.action ? UNDO_MS : 3200);
    return () => clearTimeout(timer);
  }, [toast]);
  useEffect(() => setAccessibilityError(''), [flow.delivery]);

  const attempt = current ? jobs.saved[current.recording.id] : undefined;
  const played = useCallback(() => setPlayFor(null), []);

  // Saving audio reads the stored recording rather than the blob URL the reader
  // happens to hold, so a row that was never opened can still be saved.
  const saveAudio = useCallback(async (recording: Recording) => {
    try {
      const blob = await capture.exportRecording(recording.id);
      save(blob, recording.filename);
      capture.markDownloaded(recording.id);
    } catch { /* The recorder already reports unreadable storage in place. */ }
  }, [capture.exportRecording, capture.markDownloaded]);

  /*
   * What can be done to a session without opening it. Nothing here deletes a
   * recording or its transcript: this menu is for taking the words somewhere
   * else, and losing a dictation must stay an explicit act elsewhere.
   */
  // The list's callbacks never change and read the current state when they run,
  // so passing them does not make every row redraw.
  const latest = useRef({ select, rowMenu, clipboard, jobs });
  latest.current = { select, rowMenu, clipboard, jobs };
  const selectRow = useCallback((id: string) => { latest.current.select(id); setShowReader(true); }, []);
  const copyRow = useCallback((id: string, text: string) => { void latest.current.clipboard.copy(text, id); }, []);
  const retryRow = useCallback((row: SessionRow) => { latest.current.select(row.recording.id); latest.current.jobs.transcribe(row.recording); }, []);
  const menuForRow = useCallback((row: SessionRow, text: string) => latest.current.rowMenu(row, text), []);

  function rowMenu(row: SessionRow, text: string): MenuInput {
    const { recording } = row;
    const transcribed = !!row.job || !!row.successfulJob;
    return [
      { label: 'Copy text', icon: Copy, disabled: !text, run: () => void clipboard.copy(text, recording.id) },
      { label: 'Play audio', icon: Play, run: () => { select(recording.id); setShowReader(true); setPlayFor(recording.id); } },
      props.usable && {
        label: transcribed ? 'Transcribe again' : 'Transcribe',
        icon: RefreshCw,
        disabled: row.working,
        run: () => jobs.transcribe(recording),
      },
      { kind: 'separator' as const },
      { label: 'Save audio…', icon: Download, run: () => void saveAudio(recording) },
      {
        label: 'Save text…',
        icon: Download,
        disabled: !text,
        run: () => save(new Blob([text], { type: 'text/plain;charset=utf-8' }), `${PRODUCT_SLUG}-${recording.id}-result.txt`),
      },
    ];
  }

  function discard() {
    const id = props.owner?.snapshot.session_id;
    const owner = props.owner as (SessionOwner & { discardWithUndo?: (graceMs: number) => Promise<unknown>; keep?: (id: string) => boolean }) | undefined;
    if (!id || !owner?.discardWithUndo) { props.onCancel?.(); return; }
    void owner.discardWithUndo(UNDO_MS);
    setToast({
      id: Date.now(),
      icon: <Trash2 size={16} aria-hidden />,
      text: 'Dictation discarded',
      action: {
        label: 'Undo',
        run: () => {
          const kept = owner.keep?.(id);
          setToast({ id: Date.now(), icon: kept ? <Check size={16} aria-hidden /> : <Info size={16} aria-hidden />, text: kept ? 'Kept. It’s in your history, not transcribed.' : 'It was already deleted.' });
          if (kept) setSelected(id);
        },
      },
    });
  }

  async function openAccessibility() {
    setAccessibilityError('');
    try { await invoke('accessibility_open'); }
    catch { setAccessibilityError('Could not open Accessibility settings. Open System Settings → Privacy & Security → Accessibility.'); }
  }

  const { selection, input_language: language } = props.settings.transcription;
  const outcome = outcomeOf(flow, props.integrated, wordsOf(flow.sessionId, jobs.saved, modeResults.results), accessibilityError, openAccessibility);
  const canStart = capture.supported && (props.integrated ? !live || flow.flow === 'transcribing' || flow.flow === 'processing' : !props.busy);
  const startBlocked = !capture.supported
    ? 'Microphone capture needs a supported browser on localhost or HTTPS.'
    : 'Finishing the current dictation…';
  const engineReady = props.usable;
  const textModel = activeMode?.text_model ? textModelName(activeMode.text_model.model) : null;
  const dock = (
    <Dock
      flow={flow.flow}
      levels={levels.slice(-24)}
      elapsedMs={capture.elapsed}
      shortcut={props.shortcut}
      toggle={!!props.toggle}
      integrated={props.integrated}
      canStart={canStart}
      startBlocked={startBlocked}
      onStart={capture.start}
      onStop={capture.stop}
      onDiscard={props.onCancel && discard}
      outcome={outcome}
      modes={modes}
      modeReady={modeReady}
      onMode={(id) => void chooseMode(id)}
      onOpenModes={props.onOpenModes}
      settings={props.settings}
      providers={props.providers ?? []}
      engineReady={engineReady}
      onSettings={props.onSettings}
      onOpenModels={props.onOpenModels}
    />
  );
  const toastView = toast && (
    <div className="toast" role="status" key={toast.id}>
      {toast.icon}
      <span>{toast.text}</span>
      {toast.action && <button type="button" className="toast-action" onClick={() => { const run = toast.action!.run; setToast(null); run(); }}>{toast.action.label}</button>}
    </div>
  );
  const problems = (
    <>
      {!capture.supported && (
        <p className="notice alert home-problem" role="alert">
          Microphone capture needs a supported browser on localhost or HTTPS. Open this page at a secure address.
        </p>
      )}
      {capture.error && <p className="notice alert home-problem" role="alert">{capture.error}</p>}
    </>
  );

  if (!rows.length && !capture.loading && flow.flow === 'idle') {
    return (
      <div className="page home-page">
        {problems}
        <EmptyHome
          shortcut={props.shortcut}
          toggle={!!props.toggle}
          integrated={props.integrated}
          canStart={canStart}
          startBlocked={startBlocked}
          onStart={capture.start}
          onShortcut={props.integrated && props.onOpenSettings ? () => props.onOpenSettings?.('shortcuts') : undefined}
          setup={`${activeMode?.name ?? 'Regular'} · ${shortModel(selection.model)} · ${selection.provider === 'local' ? (props.integrated ? hereLabel() : 'on your server') : 'cloud'} · ${languageLabel(language)}`}
        />
        {toastView}
      </div>
    );
  }

  return (
    <div className="page home-page">
      {problems}
      <Reprocess recordings={capture.recordings} saved={jobs.saved} settings={props.settings} modes={modes} exportAudio={capture.exportRecording} />
      <div className={`home-split ${(current && showReader) || live ? 'show-reader' : ''}`}>
        <SessionList
          rows={rows}
          selectedId={current?.recording.id ?? null}
          onSelect={selectRow}
          query={query ?? ''}
          onQuery={(value) => setQuery(value || null)}
          filter={filter}
          onFilter={setFilter}
          loading={capture.loading}
          menuFor={menuForRow}
          copiedId={clipboard.copied}
          copyStatus={clipboard.message}
          onCopy={copyRow}
          onRetry={props.usable ? retryRow : undefined}
          freshId={fresh}
          searchRef={search}
        />

        <div className="reader-stage">
          {live ? (
            <LiveStage
              flow={flow.flow}
              text={liveOwner?.snapshot?.state === 'canceled' ? '' : preview}
              levels={levels}
              elapsedMs={capture.elapsed}
              shortcut={props.shortcut}
              toggle={!!props.toggle}
              livePreview={selection.provider === 'local'}
              model={modelName(selection.model)}
              kind={activeMode?.kind}
              textModel={textModel}
              deliver={props.integrated}
            />
          ) : current ? (
            <TranscriptReader
              key={current.recording.id}
              onBack={() => setShowReader(false)}
              mode={attempt?.job?.status.state === 'succeeded' ? modeResults.results[attempt.job.id] ?? loadingMode : undefined}
              onReloadMode={modeResults.reload}
              recording={current.recording}
              job={attempt?.job}
              history={attempt?.history ?? []}
              working={current.working}
              uploadProgress={current.uploadProgress}
              attempted={!!attempt && !attempt.job}
              error={jobs.errors[current.recording.id] || undefined}
              onTranscribe={props.usable ? () => jobs.transcribe(current.recording) : undefined}
              onJobChange={jobs.updateJob}
              onDownloadAudio={() => void saveAudio(current.recording)}
              unavailableReason={props.unavailableReason}
              capabilityIssue={props.capabilityIssue}
              integrated={props.integrated}
              view={view}
              onView={setView}
              playRequested={playFor === current.recording.id}
              onPlayed={played}
              onOpenModels={props.onOpenModels}
              modes={modes}
            />
          ) : (
            <section className="region reader-column" aria-label="Transcript">
              <div className="empty">
                <strong>Nothing selected</strong>
                <p>Pick a dictation on the left to read it.</p>
              </div>
            </section>
          )}
          <div className="reader-fade" aria-hidden />
          {dock}
          {toastView}
        </div>
      </div>
    </div>
  );
}

/** The words in the finished dictation's text, for "Pasted · 34 words". */
function wordsOf(id: string | null, saved: HomeProps['jobs']['saved'], results: ReturnType<typeof useModeResults>['results']): number {
  if (!id) return 0;
  const job = successfulJob(saved[id]);
  if (!job || job.status.state !== 'succeeded') return 0;
  return countWords(modeText(results[job.id]) ?? job.status.result.transcript.raw_text);
}

/**
 * The outcome of the dictation that just finished, in one line. It says what
 * happened to the words — pasted, only copied, or kept — and never more than
 * the shell reported.
 */
function outcomeOf(flow: DictationFlow, integrated: boolean, words: number, accessibilityError: string, openAccessibility: () => void): ReactNode {
  if (flow.flow === 'failed') {
    return <span className="dock-outcome danger"><AlertTriangle size={17} aria-hidden /> Couldn’t transcribe — your audio is saved</span>;
  }
  if (flow.flow !== 'done') return null;
  if (flow.recordOnly) return <span className="dock-outcome warn"><Info size={17} aria-hidden /> Saved — not transcribed, no speech model is ready</span>;
  const count = words ? ` · ${words} word${words === 1 ? '' : 's'}` : '';
  if (!integrated) return <span className="dock-outcome"><Check size={17} aria-hidden /> Transcribed{count} · ready to copy</span>;
  const delivery = flow.delivery;
  const time = flow.turnaroundMs ? <span className="muted"> in {(flow.turnaroundMs / 1000).toFixed(1)} s</span> : null;
  if (!delivery || delivery.message === 'Processing the captured mode…') return <span className="dock-outcome"><Check size={17} aria-hidden /> Transcribed{count}</span>;
  if (delivery.pasted) return <span className="dock-outcome"><Check size={17} aria-hidden /> Pasted{delivery.app ? ` into ${delivery.app}` : ''}{count}{time}</span>;
  const accessibility = isTauri() && isMac() && delivery.message.includes('Accessibility');
  if (delivery.copied) {
    return (
      <span className="dock-outcome warn" title={accessibilityError || delivery.message}>
        <ClipboardCheck size={17} aria-hidden /> {accessibilityError || `Copied — press ${modKey()} V to paste`}
        {accessibility && (
          <button type="button" className="icon-button" aria-label="Open Accessibility settings" title="Open Accessibility settings" onClick={openAccessibility}>
            <ArrowUpRight size={16} aria-hidden />
          </button>
        )}
      </span>
    );
  }
  return <span className="dock-outcome warn" title={delivery.message}><Info size={17} aria-hidden /> {delivery.message || 'Kept — nothing was pasted'}</span>;
}
