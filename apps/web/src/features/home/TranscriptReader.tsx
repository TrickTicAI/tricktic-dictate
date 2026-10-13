import { modeText, modeReady, type ModeResult } from '../modes/useModeResults';
import { modeApi, emptyContext, type ModeProfile, type ModeSettings } from '../modes/api';
import { ProcessingPanel } from '../modes/ProcessingPanel';
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { AlertTriangle, ArrowLeft, AudioLines, Check, ChevronDown, Command as CommandIcon, Copy, Cpu, Download, FileText, Info, Languages, Mic, MoreHorizontal, RefreshCw, Sparkles, Wand2 } from 'lucide-react';
import { Problem } from '../shell/Problem';
import { AudioPlayer } from './AudioPlayer';
import { textDiff } from './textDiff';
import type { Job, ModelSelection } from '@metrix-writer/contracts';
import { formatDuration, type Recording } from '../../recorder';
import { request } from '../transcripts/api';
import { recognitionStatus } from '../transcripts/progress';
import { copyText } from '../shell/clipboard';
import { openMenu, type MenuInput } from '../shell/menu';
import { modKey } from '../shell/TopBar';
import { useCleanup, useCleanupProviders } from '../cleanup/useCleanup';
import { directionFor, hereLabel, languageLabel, modelName, textModelName } from './language';
import { countWords } from './statistics';
import { clockOf, dayLabel } from './SessionList';
import { PRODUCT_SLUG } from '../../product';

type View = 'result' | 'raw' | 'corrected' | 'cleaned';

export interface TranscriptReaderProps {
  recording: Recording;
  mode?: ModeResult;
  onReloadMode?: () => void;
  job?: Job;
  history: Job[];
  working: boolean;
  uploadProgress?: string;
  /** A request exists for this recording but no job was ever acknowledged. */
  attempted: boolean;
  error?: string;
  /** Absent when no model is usable; the reader then explains instead of offering. */
  onTranscribe?: () => void;
  onJobChange?: (job: Job) => void;
  onDownloadAudio: () => void;
  unavailableReason?: string;
  capabilityIssue?: string;
  integrated: boolean;
  view: View;
  onView: (view: View) => void;
  /** Set when a session was opened from its row menu with "Play audio". */
  playRequested?: boolean;
  onPlayed?: () => void;
  onBack?: () => void;
  /** Opens the speech engine settings, offered when a dictation could not be transcribed. */
  onOpenModels?: () => void;
  /** The modes, for "Polish with …" on a finished dictation. */
  modes?: ModeSettings | null;
}

function download(text: string, name: string) {
  const url = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Preserve order and show deleted text as well as additions, including punctuation. */
function markChanges(spoken: string, polished: string): ReactNode[] {
  return textDiff(spoken, polished).map(({ kind, text }, index) => kind === 'same' ? text
    : kind === 'removed' ? <del key={index} title="Removed" aria-label={`Removed: ${text}`}>{text}</del>
      : <mark key={index} title="Added" aria-label={`Added: ${text}`}>{text}</mark>);
}

const seconds = (ms: number) => `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;

/**
 * One dictation, as a sheet of paper.
 *
 * The header says when and what (length, words, language, mode) and which
 * version is on the page — ✦ Polished or As spoken, plus Corrected or Cleaned
 * only when they differ. The page is the text at reading size in its own
 * direction, with Copy as the one main action. Below it, the saved audio with
 * its real waveform, and one quiet line saying how the text was made; the
 * attempt history is behind Details.
 *
 * Nothing here asserts what it cannot know: polishing that failed says so and
 * shows what was heard; a delivery is described only from its receipt.
 */
export function TranscriptReader(props: TranscriptReaderProps) {
  const { recording, job, history } = props;
  const [status, setStatus] = useState('');
  const [flash, setFlash] = useState(false);
  const [corrected, setCorrected] = useState<string | null>(null);
  const [correctedError, setCorrectedError] = useState(false);
  const [reload, setReload] = useState(0);
  const [polishBusy, setPolishBusy] = useState(false);
  const [polishError, setPolishError] = useState('');
  const [polishedAgain, setPolishedAgain] = useState(false);
  const [showChanges, setShowChanges] = useState(false);
  const [details, setDetails] = useState(false);
  const [revealed, setRevealed] = useState(false);
  const providers = useCleanupProviders();
  const cleanup = useCleanup(job, props.onJobChange);

  const result = job?.status.state === 'succeeded' ? job.status.result : null;
  const raw = result?.transcript.raw_text ?? '';
  const processed = modeText(props.mode);
  const attempt = props.mode?.attempt;
  const profile = attempt?.profile;
  const modeName = profile?.name ?? 'Mode';
  const waitingForMode = !!result && !!props.mode && !modeReady(props.mode);
  const cleaned = result?.cleanup.state === 'succeeded' ? result.cleanup.cleaned_text : '';
  const originalUsed = !!attempt?.diagnostics?.original_used;

  // Corrections and snippets are a deterministic server pass over the raw text.
  useEffect(() => {
    if (!job || job.status.state !== 'succeeded') { setCorrected(null); return; }
    const controller = new AbortController();
    request<{ text: string }>(`/api/v1/jobs/${job.id}/processed`, { signal: controller.signal })
      .then((value) => { if (!controller.signal.aborted) { setCorrected(value.text); setCorrectedError(false); } })
      .catch(() => { if (!controller.signal.aborted) setCorrectedError(true); });
    return () => controller.abort();
  }, [job, reload]);

  const views = useMemo(() => {
    const list: { id: View; label: string; icon?: typeof Mic; text: string }[] = [{ id: 'raw', label: 'As spoken', icon: Mic, text: raw }];
    if (processed !== undefined && (processed !== raw || profile?.kind !== 'regular')) {
      list.unshift({
        id: 'result',
        label: originalUsed ? 'Original' : profile?.kind === 'super' ? 'Polished' : profile?.kind === 'command' ? 'Command result' : 'Corrected',
        icon: originalUsed ? undefined : profile?.kind === 'super' ? Sparkles : profile?.kind === 'command' ? CommandIcon : undefined,
        text: processed,
      });
    }
    // The Regular mode's result already is the corrected text.
    if (corrected && corrected !== raw && corrected !== processed && profile?.kind !== 'regular') list.push({ id: 'corrected', label: 'Corrected', text: corrected });
    // Offered only when a cleanup result actually exists.
    if (cleaned) list.push({ id: 'cleaned', label: 'Cleaned', text: cleaned });
    return list;
  }, [raw, corrected, cleaned, processed, profile?.kind, originalUsed]);

  const active = views.find((view) => view.id === props.view) ?? views[0];
  const language = result?.transcript.input_language ?? recording.language;
  const failed = job?.status.state === 'failed';
  const attempts = history.length;
  const text = active?.text ?? '';
  const words = text ? countWords(text) : 0;
  const superProfiles = (props.modes?.profiles ?? []).filter((item) => item.kind === 'super');

  // A transcription run again from history arrives with a moment of emphasis,
  // so the change is noticed rather than silently swapped in.
  const wasWorking = useRef(props.working);
  useEffect(() => {
    if (wasWorking.current && !props.working && job?.status.state === 'succeeded') {
      setRevealed(true);
      const timer = setTimeout(() => setRevealed(false), 6000);
      wasWorking.current = props.working;
      return () => clearTimeout(timer);
    }
    wasWorking.current = props.working;
  }, [props.working, job?.status.state]);

  // Polishing that finishes while you watch confirms itself in place.
  const wasPending = useRef(props.mode?.status === 'pending');
  useEffect(() => {
    const pending = props.mode?.status === 'pending';
    if (wasPending.current && !pending && props.mode?.status === 'succeeded') {
      setPolishedAgain(true);
      const timer = setTimeout(() => setPolishedAgain(false), 1600);
      wasPending.current = pending;
      return () => clearTimeout(timer);
    }
    wasPending.current = pending;
  }, [props.mode?.status]);

  async function polish(chosen?: ModeProfile) {
    if (!job || polishBusy) return;
    setPolishBusy(true); setPolishError('');
    try {
      const settings = await modeApi.settings();
      const target = (chosen && settings.profiles.find(p => p.id === chosen.id))
        ?? settings.profiles.find(p => p.id === profile?.id && p.kind === 'super')
        ?? settings.profiles.find(p => p.id === settings.active && p.kind === 'super')
        ?? settings.profiles.find(p => p.kind === 'super');
      if (!target) throw new Error('Choose a Super profile in Settings → Modes.');
      await modeApi.process(job.id, { operation_id: crypto.randomUUID(), profile_id: target.id, profile_revision: target.revision, context: emptyContext() });
      props.onView('result'); props.onReloadMode?.();
    } catch (error) { setPolishError((error as Error).message); }
    finally { setPolishBusy(false); }
  }

  const copy = useCallback(async (value: string) => {
    const done = await copyText(value);
    setStatus(done ? 'Copied' : 'Clipboard unavailable. Select the text and copy it manually.');
    setFlash(done);
    window.setTimeout(() => { setFlash(false); setStatus(''); }, 1600);
  }, []);

  // Ctrl/⌘ C with nothing selected copies the page, as the hint beside Copy says.
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.shiftKey || event.altKey || event.key.toLowerCase() !== 'c') return;
      const target = event.target as HTMLElement | null;
      if (!text || target?.closest('input, textarea, select, [contenteditable="true"]')) return;
      if (window.getSelection()?.toString() || document.querySelector('[aria-modal="true"]')) return;
      event.preventDefault();
      void copy(text);
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [text, copy]);

  const saveText = (value: string, id: string) => download(value, `${PRODUCT_SLUG}-${recording.id}-${id}.txt`);

  /** Entries for a block of transcript text, wherever it is drawn. */
  function textEntries(value: string, id: string): MenuInput {
    const selection = window.getSelection()?.toString().trim() ?? '';
    return [
      !!selection && { label: 'Copy selection', icon: Copy, run: () => void copy(selection) },
      { label: 'Copy text', icon: Copy, hint: `${modKey()}+C`, disabled: !value, run: () => void copy(value) },
      { kind: 'separator' as const },
      { label: 'Save text…', icon: Download, disabled: !value, run: () => saveText(value, id) },
      !!props.onTranscribe && { label: 'Transcribe again', icon: RefreshCw, disabled: props.working, run: () => props.onTranscribe?.() },
    ];
  }

  const cleanupChoice: ModelSelection | undefined = providers.choices.find((choice) => choice.configured)?.selection;

  /** Everything that is not one of the three buttons. */
  function moreEntries(): MenuInput {
    return [
      { label: 'Save text…', icon: Download, disabled: !text, run: () => saveText(text, active?.id ?? 'result') },
      { label: 'Save audio…', icon: Download, run: props.onDownloadAudio },
      !!result && { kind: 'separator' as const },
      !!result && {
        label: cleaned ? 'Redo cleanup' : 'Clean up',
        icon: Wand2,
        disabled: !cleanupChoice || cleanup.sending || cleanup.pending,
        run: () => { if (cleanupChoice) void cleanup.start(cleanupChoice); },
      },
      ...(result ? superProfiles.filter((item) => item.id !== profile?.id).map((item) => ({
        label: `Polish with ${item.name}`,
        icon: Sparkles,
        disabled: polishBusy || props.mode?.status === 'pending',
        run: () => void polish(item),
      })) : []),
    ];
  }

  const played = useCallback(() => props.onPlayed?.(), [props.onPlayed]);
  const polishing = polishBusy || props.mode?.status === 'pending';
  const polishable = !!result && superProfiles.length > 0 && profile?.kind !== 'command';
  const heardBy = result ? modelName(result.transcript.selection.model) : null;
  const heardWhere = result?.transcript.execution_location === 'cloud' ? 'in the cloud' : props.integrated ? hereLabel() : 'on your server';
  const textModel = attempt?.diagnostics?.selected_model ?? profile?.text_model;
  const receipt = props.mode?.delivery;
  const delay = attempt?.diagnostics?.delivery?.stop_to_delivery_ms;
  const earlierText = failed && history.some((previous) => previous.status.state === 'succeeded');
  const showAttempts = details || earlierText;

  return (
    <section className="region reader-column" aria-label="Transcript">
      <div className="reader-head">
        {props.onBack && (
          <button type="button" className="icon-button reader-back" aria-label="Back to history" title="Back to history" onClick={props.onBack}>
            <ArrowLeft size={18} aria-hidden />
          </button>
        )}
        <div className="reader-title">
          <h2>{dayLabel(recording.createdAt)}, {clockOf(recording.createdAt)}</h2>
          <div className="reader-chips">
            <span className="chip plain mono"><AudioLines size={13} aria-hidden /> {formatDuration(recording.duration)}</span>
            {!!words && <span className="chip plain"><FileText size={13} aria-hidden /> {words} words</span>}
            <span className="chip plain"><Languages size={13} aria-hidden /> {languageLabel(language)}</span>
            {profile && profile.kind !== 'regular' && (
              <span className="chip plain">{profile.kind === 'super' ? <Sparkles size={13} aria-hidden /> : <CommandIcon size={13} aria-hidden />} {profile.name}</span>
            )}
          </div>
        </div>
        <span className="spacer" />
        {views.length > 1 && (
          <div className="segmented" role="group" aria-label="Transcript version">
            {views.map((view) => {
              const Icon = view.icon;
              return (
                <button key={view.id} type="button" aria-pressed={active?.id === view.id} onClick={() => { props.onView(view.id); setShowChanges(false); }}>
                  {Icon && <Icon size={14} aria-hidden />}{view.label}
                </button>
              );
            })}
          </div>
        )}
      </div>

      <div className="scroller reader-body">
        {polishError && <p className="notice alert" role="alert">{polishError}</p>}
        {props.capabilityIssue && <p className="notice alert" role="alert">{props.capabilityIssue}</p>}
        {props.error && <p className="notice alert" role="alert">{props.error}</p>}
        {correctedError && (
          <Problem
            title="Corrections and snippets could not be loaded"
            keeps="What was heard is unaffected."
            onRetry={() => setReload((value) => value + 1)}
          />
        )}
        {result && props.mode?.status === 'unavailable' && <Problem title="Processed result could not be loaded" keeps="The original transcript is still available." onRetry={props.onReloadMode} />}
        {attempt?.state.state === 'failed' && <p className="notice alert" role="alert">{modeName} failed: {attempt.state.error.message} Showing the original transcript.</p>}
        {originalUsed && <p className="notice warn" role="status">Polishing unavailable. Using the original with your corrections and snippets.</p>}
        {receipt?.original_used && receipt.attempt_id !== attempt?.id && <p className="notice" role="status">The original was {receipt.pasted ? 'inserted' : receipt.copied ? 'copied' : 'kept'}. This later result is available here.</p>}
        {cleanup.error && <p className="notice alert" role="alert">{cleanup.error}</p>}
        {result?.cleanup.state === 'failed' && <p className="notice alert" role="alert">Cleanup failed: {result.cleanup.error.message}</p>}

        <article className={`paper${result ? '' : ' empty-paper'}`}>
          {result && (
            <div className="paper-tools">
              {active?.id === 'result' && profile?.kind === 'super' && !originalUsed && processed !== raw && (
                <button type="button" className={`button small ${showChanges ? '' : 'ghost'}`} aria-pressed={showChanges} onClick={() => setShowChanges((value) => !value)}>
                  <Wand2 size={14} aria-hidden /> {showChanges ? 'Hide changes' : 'Show changes'}
                </button>
              )}
              {!!text && (
                <button type="button" className="icon-button" aria-label={`Copy the ${active!.label.toLowerCase()} transcript`} title="Copy this version" onClick={() => void copy(text)}>
                  {flash ? <Check size={16} aria-hidden /> : <Copy size={16} aria-hidden />}
                </button>
              )}
            </div>
          )}

          {props.working && (
            <div className="paper-line accent" role="status">
              <span className="spinner" aria-hidden />
              {job ? `${recognitionStatus(job)} Your saved ${formatDuration(recording.duration)} recording stays available.` : props.uploadProgress || 'Sending your saved recording for transcription…'}
            </div>
          )}
          {revealed && !props.working && (
            <div className="paper-line success reveal" role="status">
              <span className="check-pop" aria-hidden><Check size={13} /></span>
              Transcribed again — it wasn’t pasted anywhere, so it’s ready to copy.
            </div>
          )}
          {result && polishing && (
            <div className="paper-line accent" role="status">
              <span className="spinner" aria-hidden />
              {attempt?.diagnostics?.fallback_attempted ? 'Trying the backup polishing model…' : 'Polishing…'} You can copy what you said while polishing finishes.
              {waitingForMode && props.view === 'result' && <button type="button" className="row-link" onClick={() => props.onView('raw')}>Use original transcript</button>}
            </div>
          )}
          {result && props.mode?.status === 'loading' && <div className="paper-line" role="status"><span className="spinner" aria-hidden /> Checking the saved mode result…</div>}
          {cleanup.pending && (
            <div className="paper-line accent" role="status">
              <span className="spinner" aria-hidden /> Cleaning up… If it was interrupted, asking again does not start a second call.
            </div>
          )}
          {result && !polishing && waitingForMode && props.view === 'result' && props.mode?.status !== 'loading' && (
            <div className="paper-line"><button type="button" className="button small" onClick={() => props.onView('raw')}>Use original transcript</button></div>
          )}

          {failed && job.status.state === 'failed' && !props.working && (
            <div className="notice alert paper-notice" role="alert">
              <AlertTriangle size={18} aria-hidden />
              <div>
                <strong>This dictation couldn’t be transcribed</strong>
                {job.status.error.message} Your {formatDuration(recording.duration)} recording is saved — nothing is lost.
              </div>
            </div>
          )}
          {!job && !props.working && props.attempted && (
            <div className="notice warn paper-notice" role="status">
              <Info size={18} aria-hidden />
              <div>
                <strong>Waiting for an answer</strong>
                Submission was not acknowledged. The audio is saved and the request is kept; retrying checks the same recording and
                resumes the same request rather than starting a second one.
              </div>
            </div>
          )}
          {!job && !props.working && !props.attempted && (
            <div className="notice paper-notice" role="status">
              <AudioLines size={18} aria-hidden />
              <div>
                <strong>Not transcribed yet</strong>
                {props.unavailableReason ?? 'This audio is saved and has not been transcribed yet. Transcribe it when you are ready.'}
              </div>
            </div>
          )}

          {result
            ? (
              <div
                className="transcript-block"
                role="group"
                aria-label={`${active?.label ?? 'As spoken'} transcript`}
                onContextMenu={(event) => openMenu(event, textEntries(text, active?.id ?? 'raw'))}
              >
                {showChanges && active?.id === 'result' && <p className="transcript-diff-key">Compared with As spoken · <del>Removed</del> · <mark>Added</mark>. Copy saves the selected version.</p>}
                {/* An empty transcript has no direction to follow, and forcing
                    RTL there mirrored the English placeholder. */}
                <p
                  className={`transcript-text${revealed || polishedAgain ? ' reveal' : ''}${polishing && active?.id === 'result' ? ' processing' : ''}`}
                  dir={text ? directionFor(language) : 'auto'}
                  lang={language === 'he' ? 'he' : undefined}
                  data-empty="No speech was heard in this recording."
                >
                  {showChanges && active?.id === 'result' ? markChanges(raw, text) : text}
                </p>
              </div>
            )
            : props.working
              ? <div className="skeleton" aria-hidden><i style={{ width: '94%' }} /><i style={{ width: '88%' }} /><i style={{ width: '62%' }} /></div>
              : <p className="paper-empty">No text yet.</p>}

          <footer className="paper-actions">
            {result ? (
              <>
                <button type="button" className={`button ${flash ? 'done' : 'primary'}`} disabled={!text} onClick={() => void copy(text)}>
                  {flash ? <><Check size={15} aria-hidden /> Copied</> : <><Copy size={15} aria-hidden /> Copy</>}
                </button>
                {polishable && (
                  <button type="button" className={`button${polishing ? ' busy' : ''}${polishedAgain ? ' done' : ''}`} disabled={polishing} onClick={() => void polish()}>
                    {polishing ? <><span className="spinner" aria-hidden /> Polishing…</> : polishedAgain ? <><Check size={15} aria-hidden /> Polished</> : <><Sparkles size={15} aria-hidden /> {profile?.kind === 'super' ? 'Polish again' : 'Polish'}</>}
                  </button>
                )}
                <button
                  type="button"
                  className={`button ghost${props.working ? ' busy' : ''}`}
                  disabled={!props.onTranscribe || props.working}
                  title={props.onTranscribe ? 'Transcribe the saved audio again. Nothing is pasted.' : props.unavailableReason}
                  onClick={() => props.onTranscribe?.()}
                >
                  {props.working ? <span className="spinner" aria-hidden /> : <RefreshCw size={15} aria-hidden />} Transcribe again
                </button>
              </>
            ) : (
              <>
                <button
                  type="button"
                  className={`button primary${props.working ? ' busy' : ''}`}
                  disabled={!props.onTranscribe || props.working}
                  title={props.onTranscribe ? undefined : props.unavailableReason}
                  onClick={() => props.onTranscribe?.()}
                >
                  {props.working ? <><span className="spinner" aria-hidden /> Transcribing…</> : <><RefreshCw size={15} aria-hidden /> {job || props.attempted ? 'Transcribe again' : 'Transcribe'}</>}
                </button>
                {props.onOpenModels && !props.working && (
                  <button type="button" className="button" onClick={props.onOpenModels}><Cpu size={15} aria-hidden /> Try another model</button>
                )}
              </>
            )}
            <button type="button" className="icon-button" aria-label="More actions" title="More" onClick={(event) => openMenu(event, moreEntries())}>
              <MoreHorizontal size={18} aria-hidden />
            </button>
            <span className="spacer" />
            {!!text && <span className="kbd paper-hint" title="Copies this version when nothing is selected" aria-hidden><kbd>{modKey()}</kbd><kbd>C</kbd></span>}
          </footer>
          <p className="visually-hidden" role="status">{status}</p>
        </article>

        {recording.url && (
          <AudioPlayer
            src={recording.url}
            durationMs={recording.duration}
            label="saved audio"
            playRequested={props.playRequested}
            onPlayed={played}
            onSave={props.onDownloadAudio}
          />
        )}

        <div className="made">
          <Info size={14} aria-hidden />
          {heardBy
            ? <span>Heard by <b>{heardBy}</b> {heardWhere}</span>
            : <span>Audio kept {props.integrated ? hereLabel() : 'in this browser'}</span>}
          {props.mode?.status === 'succeeded' && profile && profile.kind !== 'regular' && !originalUsed && (
            <span>
              · {profile.kind === 'command' ? 'Command run' : 'Polished'}{textModel ? <> by <b>{textModelName(textModel.model)}</b></> : null}
              {attempt?.diagnostics ? ` in ${seconds(attempt.diagnostics.processing_ms)}` : ''}
              {attempt?.diagnostics?.reused_draft ? (attempt.diagnostics.prepared_at_release ? ', using work prepared at release' : ', using work prepared while you spoke') : ''}
              {processed === raw ? ' · The model returned the original wording.' : ''}
            </span>
          )}
          {receipt && (
            <span>· {receipt.pasted ? `Pasted${receipt.app ? ` into ${receipt.app}` : ''}${delay ? ` ${(delay / 1000).toFixed(1)} s after you stopped` : ''}` : receipt.copied ? 'Copied — automatic paste wasn’t available' : 'Not pasted'}</span>
          )}
          {heardBy && <span>· Audio kept {props.integrated ? hereLabel() : 'in this browser'}</span>}
          {(!!job || attempts > 0) && (
            <button type="button" className="row-link made-details" aria-expanded={showAttempts} onClick={() => setDetails((value) => !value)}>
              {showAttempts ? 'Hide details' : 'Details'}{attempts ? ` · ${attempts} earlier attempt${attempts > 1 ? 's' : ''}` : ''}
              <ChevronDown size={13} aria-hidden />
            </button>
          )}
        </div>

        {showAttempts && (
          <div className="reader-details reveal">
            {result && job && <ProcessingPanel key={job.id} jobId={job.id} onChange={props.onReloadMode} />}
            {!!attempts && (
              <div className="attempt-list">
                <h3>Earlier attempts ({attempts})</h3>
                {history.map((previous) => (
                  <article className="attempt" key={previous.id}>
                    <header>
                      <span>{new Date(previous.created_at_ms).toLocaleString()}</span>
                      <span className="badge">{modelName(previous.request.options.selection.model)}</span>
                      {previous.status.state === 'failed' && <span className="badge danger">{previous.status.error.code}</span>}
                    </header>
                    {previous.status.state === 'succeeded' && (
                      <p
                        className="transcript-text"
                        dir={directionFor(previous.status.result.transcript.input_language)}
                        onContextMenu={(event) => previous.status.state === 'succeeded'
                          && openMenu(event, textEntries(previous.status.result.transcript.raw_text, `attempt-${previous.id}`))}
                      >
                        {previous.status.result.transcript.raw_text}
                      </p>
                    )}
                    {previous.status.state === 'failed' && <p className="small muted">{previous.status.error.message}</p>}
                  </article>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
