import { modeText, type ModeResult } from '../modes/useModeResults';
import { memo, useMemo, useRef, type KeyboardEvent, type RefObject } from 'react';
import { AlertTriangle, AudioLines, Check, Command as CommandIcon, Copy, Search, Sparkles } from 'lucide-react';
import type { Job } from '@metrix-writer/contracts';
import { formatDuration, type Recording } from '../../recorder';
import { openMenu, type MenuInput } from '../shell/menu';
import { modKey } from '../shell/TopBar';
import { directionFor, languageLabel } from './language';
import { languageCode } from './Dock';
import { recognitionStatus } from '../transcripts/progress';

export interface SessionRow {
  recording: Recording;
  mode?: ModeResult;
  job?: Job;
  successfulJob?: Job;
  /** True while this session is uploading, queued or transcribing. */
  working: boolean;
  uploadProgress?: string;
}

export const FILTERS = ['all', 'attention', 'polished', 'hebrew'] as const;
export type HistoryFilter = typeof FILTERS[number];

// Made once: a formatter per call cost more than the rest of a history row.
const DAY = new Intl.DateTimeFormat(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
const CLOCK = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });

export function dayLabel(date: Date): string {
  const today = new Date();
  const start = (value: Date) => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime();
  const days = Math.round((start(today) - start(date)) / 86_400_000);
  if (days === 0) return 'Today';
  if (days === 1) return 'Yesterday';
  return DAY.format(date);
}

export function transcriptOf(job: Job | undefined): string {
  return job?.status.state === 'succeeded' ? job.status.result.transcript.raw_text : '';
}

export const clockOf = (date: Date) => CLOCK.format(date);

/** The text a row stands for: the mode's result when there is one, else what was heard. */
export const rowText = (row: SessionRow) => modeText(row.mode) ?? transcriptOf(row.successfulJob ?? row.job);

/** Failed, cut short or never transcribed: the rows the "Needs attention" filter keeps. */
export function needsAttention(row: SessionRow): boolean {
  if (row.working) return false;
  return row.job?.status.state === 'failed' || row.recording.metadata.capture_state === 'interrupted' || (!row.job && !row.successfulJob);
}
export const polished = (row: SessionRow) => row.mode?.status === 'succeeded' && row.mode.attempt?.profile.kind === 'super' && !row.mode.attempt.diagnostics?.original_used;
const spokenHebrew = (row: SessionRow) => row.recording.language !== 'en';

/** Where the words went, from the delivery receipt; nothing is claimed without one. */
export function deliveryLabel(mode?: ModeResult): string | null {
  const receipt = mode?.delivery;
  if (!receipt) return null;
  const into = receipt.app ? ` into ${receipt.app}` : '';
  if (receipt.pasted) return `${receipt.original_used ? 'Original pasted' : 'Pasted'}${into}`;
  if (receipt.copied) return 'Copied only';
  return 'Not pasted';
}

/**
 * A feed of what you said.
 *
 * The dictated words are the reason a row exists, so they are set at reading
 * size and carry the row; the clock and the length annotate it, and a quiet
 * meta line says the language, the mode and where the words went. A Hebrew
 * row is the exact mirror of an English one, because each row takes its
 * direction from its own transcript rather than from the interface. A
 * dictation that failed, was cut short or was never transcribed says what is
 * kept and offers the one thing to do about it in place.
 */
export function SessionList({ rows, selectedId, onSelect, query, onQuery, filter, onFilter, loading, menuFor, onCopy, copiedId, copyStatus, onRetry, freshId, searchRef }: {
  rows: SessionRow[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  query: string;
  onQuery: (value: string) => void;
  filter: HistoryFilter;
  onFilter: (filter: HistoryFilter) => void;
  loading: boolean;
  /** Right-click entries for one row; the list itself has no menu. */
  menuFor?: (row: SessionRow, text: string) => MenuInput;
  onCopy?: (id: string, text: string) => void;
  /** The row whose copy has just succeeded, however it was started. */
  copiedId?: string;
  copyStatus?: string;
  /** Transcribes a row's saved audio (again). Absent when no model is usable. */
  onRetry?: (row: SessionRow) => void;
  /** A dictation that just finished, which slides in. */
  freshId?: string | null;
  searchRef?: RefObject<HTMLInputElement | null>;
}) {
  const list = useRef<HTMLDivElement>(null);
  const attention = useMemo(() => rows.filter(needsAttention).length, [rows]);
  const offered = useMemo(() => FILTERS.filter((id) =>
    id === 'all' || id === 'attention' || (id === 'polished' ? rows.some(polished) : rows.some(spokenHebrew)) || id === filter), [rows, filter]);

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return rows.filter((row) => {
      if (filter === 'attention' && !needsAttention(row)) return false;
      if (filter === 'polished' && !polished(row)) return false;
      if (filter === 'hebrew' && !spokenHebrew(row)) return false;
      if (!needle) return true;
      return (modeText(row.mode) ?? '').toLowerCase().includes(needle) ||
        transcriptOf(row.successfulJob ?? row.job).toLowerCase().includes(needle) ||
        row.recording.createdAt.toLocaleTimeString().toLowerCase().includes(needle);
    });
  }, [rows, query, filter]);

  const groups = useMemo(() => {
    const map = new Map<string, SessionRow[]>();
    for (const row of filtered) {
      const label = dayLabel(row.recording.createdAt);
      const bucket = map.get(label);
      if (bucket) bucket.push(row);
      else map.set(label, [row]);
    }
    return [...map.entries()];
  }, [filtered]);

  // One row is in the Tab order: the open one when it is listed, else the first.
  const tabbable = filtered.some((row) => row.recording.id === selectedId) ? selectedId : filtered[0]?.recording.id;

  /* Up and Down walk the list and open each dictation as they go, the way a
     mail list does; Home and End jump to the ends. */
  function walk(event: KeyboardEvent) {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    const hits = [...(list.current?.querySelectorAll<HTMLButtonElement>('.row-hit') ?? [])];
    if (!hits.length) return;
    const at = hits.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? hits.length - 1
      : at < 0 ? 0 : Math.max(0, Math.min(hits.length - 1, at + (event.key === 'ArrowDown' ? 1 : -1)));
    event.preventDefault();
    hits[next].focus();
    hits[next].scrollIntoView({ block: 'nearest' });
    const id = hits[next].dataset.id;
    if (id) onSelect(id);
  }

  return (
    <section className="region session-column" aria-label="Sessions">
      <div className="session-column-head">
        <label className="search-field history-search">
          <Search size={16} aria-hidden />
          <span className="visually-hidden">Search dictations</span>
          <input
            ref={searchRef}
            type="search"
            placeholder="Search everything you said"
            aria-keyshortcuts={modKey() === '⌘' ? 'Meta+F' : 'Control+F'}
            value={query}
            onChange={(event) => onQuery(event.target.value)}
          />
          <span className="kbd" aria-hidden><kbd>{modKey()}</kbd><kbd>F</kbd></span>
        </label>
        {rows.length > 0 && (
          <div className="history-filters" role="group" aria-label="Show">
            {offered.map((id) => (
              <button key={id} type="button" className="chip" aria-pressed={filter === id} onClick={() => onFilter(id)}>
                {id === 'all' ? 'All' : id === 'attention' ? 'Needs attention' : id === 'polished' ? <><Sparkles size={12} aria-hidden /> Polished</> : 'Hebrew'}
                {id === 'attention' && attention > 0 && <span className="chip-count">{attention}</span>}
              </button>
            ))}
          </div>
        )}
      </div>

      <p className="visually-hidden" role="status">{copyStatus ?? ''}</p>

      <div className="scroller history-list" ref={list} onKeyDown={walk}>
        {loading && !rows.length && <p className="notice" role="status" style={{ margin: 12 }}>Restoring saved audio…</p>}
        {!loading && !rows.length && (
          <div className="empty">
            <AudioLines size={26} aria-hidden />
            <strong>Your dictations will appear here</strong>
            <p>Audio is saved before anything is transcribed, and stays here through a restart.</p>
          </div>
        )}
        {!!rows.length && !filtered.length && (
          <div className="empty">
            <strong>Nothing here</strong>
            <p>{query ? `No saved dictation contains “${query}”.` : filter === 'attention' ? 'Every dictation was transcribed. Nothing needs attention.' : 'No dictation matches this filter.'}</p>
          </div>
        )}

        {groups.map(([label, items]) => (
          <div key={label} className="session-day">
            <h2 className="session-group">{label}<span>· {items.length}</span></h2>
            {items.map((row) => (
              <MemoRow
                key={row.recording.id}
                row={row}
                current={selectedId === row.recording.id}
                tabbable={tabbable === row.recording.id}
                fresh={freshId === row.recording.id}
                copied={copiedId === row.recording.id}
                onSelect={onSelect}
                onCopy={onCopy}
                onRetry={onRetry}
                menuFor={menuFor}
              />
            ))}
          </div>
        ))}
      </div>
    </section>
  );
}

/** A row redraws only when its own dictation or state changed. */
const MemoRow = memo(Row);

function Row({ row, current, tabbable, fresh, copied, onSelect, onCopy, onRetry, menuFor }: {
  row: SessionRow;
  current: boolean;
  tabbable: boolean;
  fresh: boolean;
  copied: boolean;
  onSelect: (id: string) => void;
  onCopy?: (id: string, text: string) => void;
  onRetry?: (row: SessionRow) => void;
  menuFor?: (row: SessionRow, text: string) => MenuInput;
}) {
  const { recording, job } = row;
  const failed = job?.status.state === 'failed';
  const interrupted = recording.metadata.capture_state === 'interrupted';
  const untranscribed = !job && !row.successfulJob && !row.working;
  const text = rowText(row);
  const clock = clockOf(recording.createdAt);
  const kind = row.mode?.status === 'succeeded' ? row.mode.attempt?.profile.kind : undefined;
  const delivered = deliveryLabel(row.mode);
  // Recognised and still being shaped or pasted, or still being recognised.
  const progress = !row.working ? ''
    : job?.status.state === 'succeeded' ? (row.mode?.status === 'pending' ? `${row.mode.attempt?.profile.name ?? 'Mode'} polishing…` : 'Pasting…')
    : row.uploadProgress || recognitionStatus(job);
  const placeholder = row.working ? progress
    : failed ? 'Couldn’t transcribe — your audio is saved'
    : 'Saved audio, not transcribed yet';
  const pending = !row.working && row.mode?.status === 'pending' ? `${row.mode.attempt?.profile.name ?? 'Mode'} polishing… · showing what you said` : '';

  return (
    <div
      className={`session-row${current ? ' current' : ''}${fresh ? ' fresh' : ''}`}
      // The transcript decides which way its own row reads.
      dir={text ? directionFor(recording.language) : undefined}
      onContextMenu={menuFor ? (event) => openMenu(event, menuFor(row, text)) : undefined}
    >
      {/* One press target covering the row, so the row is a button without
          the copy and retry controls having to live inside it. */}
      <button
        type="button"
        className="row-hit"
        data-id={recording.id}
        aria-current={current}
        tabIndex={tabbable ? 0 : -1}
        onClick={() => onSelect(recording.id)}
      >
        <span className="visually-hidden">
          {`${clock}, ${formatDuration(recording.duration)}, ${languageLabel(recording.language)}. `}
          {text || placeholder}
          {failed && '. Transcription failed; audio saved'}
          {interrupted && '. Interrupted; partial audio kept'}
        </span>
      </button>

      <p className={text ? 'row-text' : `row-text pending${row.working ? ' busy' : ''}`}>
        {!text && row.working && <span className="spinner" style={{ '--spinner-size': '12px' } as React.CSSProperties} aria-hidden />}
        {text || placeholder}
      </p>

      <div className="row-end">
        {/* A 12-hour clock is not part of what was said, so it keeps its own
            order inside a mirrored row. */}
        <span className="row-when" dir="ltr">
          <time dateTime={recording.createdAt.toISOString()}>{clock}</time>
          <span className="mono">{formatDuration(recording.duration)}</span>
        </span>
        {!!text && onCopy && (
          <button
            type="button"
            className="row-copy"
            // Only the row in the Tab order offers its buttons to Tab; the
            // arrow keys move that row, so a long list is one stop, not fifty.
            tabIndex={tabbable ? 0 : -1}
            aria-label={`Copy the ${clock} transcript`}
            title={row.mode?.status === 'pending' ? 'Copy the original while polishing finishes' : 'Copy'}
            onClick={() => onCopy(recording.id, text)}
          >
            {copied ? <Check size={14} aria-hidden /> : <Copy size={14} aria-hidden />}
          </button>
        )}
      </div>

      <div className="row-meta" dir="ltr">
        {row.working && !!text && <span className="row-state busy"><span className="spinner" style={{ '--spinner-size': '11px' } as React.CSSProperties} aria-hidden /> {progress}</span>}
        {!row.working && failed && (
          <span className="row-state danger">
            <AlertTriangle size={13} aria-hidden /> {text ? 'Last attempt failed' : 'Failed'}
            {onRetry && <> · <button type="button" className="row-link" tabIndex={tabbable ? 0 : -1} onClick={() => onRetry(row)}>Retry</button></>}
          </span>
        )}
        {!row.working && untranscribed && (
          <span className="row-state">
            <AudioLines size={13} aria-hidden /> {onRetry ? <button type="button" className="row-link" tabIndex={tabbable ? 0 : -1} onClick={() => onRetry(row)}>Transcribe</button> : 'Not transcribed'}
          </span>
        )}
        {interrupted && <span className="row-state warn"><AlertTriangle size={13} aria-hidden /> Interrupted · partial audio kept</span>}
        {!!text && !failed && !row.working && (
          <>
            <span>{languageCode(recording.language)}</span>
            {kind && kind !== 'regular' && (
              <>
                <i className="sep" aria-hidden />
                <span className="row-mode">{kind === 'super' ? <Sparkles size={12} aria-hidden /> : <CommandIcon size={12} aria-hidden />}{kind === 'super' ? 'Polished' : 'Command'}</span>
              </>
            )}
            {pending && <><i className="sep" aria-hidden /><span>{pending}</span></>}
            {delivered && <><i className="sep" aria-hidden /><span>{delivered}</span></>}
          </>
        )}
      </div>
    </div>
  );
}
