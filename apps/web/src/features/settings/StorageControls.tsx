import { useEffect, useState } from 'react';
import { RefreshCw, Trash2 } from 'lucide-react';
import { invoke } from '../desktop-session/runtime/native';
import { deleteAttempt } from '../transcripts/storage';
import { PRODUCT_NAME } from '../../product';
import { isMac } from './platform';
import { Group } from './controls';

interface Usage { data_bytes: number; data_limit_bytes: number; audio_bytes: number; models_bytes: number; state_bytes: number }
interface Preview { id: string; recordings: number; jobs: number; history: boolean; before_ms: number | null }
type What = 'audio' | 'history';
type From = 'before-today' | 'everything';

/** Sizes the way Windows Explorer and Finder say them: one decimal, never "0.0". */
export function size(value: number): string {
  const units: [number, string][] = [[1024 ** 3, 'GB'], [1024 ** 2, 'MB'], [1024, 'KB']];
  for (const [unit, label] of units) {
    if (value >= unit) return `${(value / unit).toFixed(value >= 100 * unit ? 0 : 1)} ${label}`;
  }
  return value > 0 ? 'Under 1 KB' : '0 KB';
}

/** What each choice deletes and what it keeps, said before anything is reviewed. */
export function consequence(what: What, from: From): string {
  const when = from === 'before-today' ? 'from before today' : 'from every dictation';
  return what === 'audio'
    ? `Deletes the recorded audio ${when}. Transcripts, polished text and their history stay, so you can still read and copy them; that audio can no longer be played or transcribed again.`
    : `Deletes the audio, transcripts and processing history ${when}. Local synced text copies go too, and history sync stops on this device; cloud copies stay in your account until you delete them in Account.`;
}

/*
 * Space on this computer, and one place to free it. The shell measures usage
 * and does the deleting; a deletion is always reviewed first, with the number
 * of recordings it removes, and nothing is deleted while a dictation is still
 * recording or being transcribed. Models, settings, the dictionary and keys
 * are never touched here.
 */
export function StorageControls({ onDeleteEverything }: {
  /** Deleting audio and transcripts from every dictation: the existing full deletion. */
  onDeleteEverything: () => Promise<void>;
}) {
  const [usage, setUsage] = useState<Usage>();
  const [status, setStatus] = useState('');
  const [what, setWhat] = useState<What>('audio');
  const [from, setFrom] = useState<From>('before-today');
  const [preview, setPreview] = useState<Preview>();
  const [confirmAll, setConfirmAll] = useState(false);
  const [busy, setBusy] = useState(false);
  const refresh = () => invoke<Usage>('storage_status').then(setUsage).catch(() => setStatus('Storage usage could not be measured. Try again in a moment.'));
  useEffect(() => { void refresh(); }, []);
  // A new choice withdraws a review made for the previous one.
  useEffect(() => { setPreview(undefined); setConfirmAll(false); setStatus(''); }, [what, from]);

  const idle = () => {
    if (!(window as Window & { writerCanDeleteHistory?: () => boolean }).writerCanDeleteHistory?.()) {
      setStatus('Finish recording and transcription before freeing space.');
      return false;
    }
    return true;
  };
  async function review() {
    if (!idle()) return;
    setStatus('');
    if (what === 'history' && from === 'everything') { setConfirmAll(true); return; }
    setBusy(true);
    const today = new Date(); today.setHours(0, 0, 0, 0);
    try { setPreview(await invoke<Preview>('storage_preview', { beforeMs: from === 'everything' ? null : today.getTime(), history: what === 'history' })); }
    catch (cause) { setStatus(message(cause)); }
    finally { setBusy(false); }
  }
  async function clear() {
    if (!preview || !idle()) return;
    setBusy(true); setStatus('');
    try {
      const result = await invoke<{ recording_ids: string[]; history: boolean }>('storage_clear', { previewId: preview.id });
      if (result.history) { for (const id of result.recording_ids) await deleteAttempt(id); }
      window.location.reload();
    } catch (cause) {
      await invoke('history_resume').catch(() => {});
      setStatus(message(cause)); setBusy(false);
    }
  }
  async function clearEverything() {
    if (!idle()) return;
    setBusy(true); setStatus('');
    try { await onDeleteEverything(); }
    catch (cause) { setStatus(message(cause)); setBusy(false); }
  }

  const here = isMac() ? 'this Mac' : 'this PC';
  const text = usage ? Math.max(0, usage.data_bytes - usage.audio_bytes) + usage.state_bytes : 0;
  const parts = usage ? [
    { id: 'audio', label: 'Recordings', bytes: usage.audio_bytes },
    { id: 'text', label: 'Transcripts and settings', bytes: text },
    { id: 'models', label: 'Downloaded models', bytes: usage.models_bytes },
  ] : [];
  const total = parts.reduce((sum, part) => sum + part.bytes, 0);
  const share = (bytes: number) => (total ? Math.max(bytes ? 0.6 : 0, (bytes / total) * 100) : 0);

  return (
    <>
      <Group title="Storage">
        <div className="storage-usage">
          {usage ? (
            <>
              <div className="storage-head">
                <strong>{size(total)}</strong>
                <span>used by {PRODUCT_NAME} on {here}</span>
                <button type="button" className="icon-button" aria-label="Measure again" title="Measure again" disabled={busy} onClick={() => { setStatus(''); void refresh(); }}>
                  <RefreshCw size={15} aria-hidden />
                </button>
              </div>
              <div className="storage-bar" role="img" aria-label={parts.map((part) => `${part.label} ${size(part.bytes)}`).join(', ')}>
                {parts.map((part) => <i key={part.id} className={part.id} style={{ width: `${share(part.bytes)}%` }} />)}
              </div>
              <ul className="storage-legend">
                {parts.map((part) => (
                  <li key={part.id}><span className={`swatch ${part.id}`} aria-hidden />{part.label}<b>{size(part.bytes)}</b></li>
                ))}
              </ul>
              <p className="storage-note">
                Dictation history uses {size(usage.data_bytes)} of its {size(usage.data_limit_bytes)} limit; audio is most of it, and audio linked from
                another recording is counted once. Models are kept apart and never deleted here.
              </p>
            </>
          ) : <p className="storage-note">Measuring…</p>}
        </div>
      </Group>

      <Group title="Free up space">
        <div className="storage-free">
          <div className="storage-choice">
            <span id="storage-what">What to delete</span>
            <div className="segmented" role="group" aria-labelledby="storage-what">
              <button type="button" aria-pressed={what === 'audio'} disabled={busy} onClick={() => setWhat('audio')}>Audio only</button>
              <button type="button" aria-pressed={what === 'history'} disabled={busy} onClick={() => setWhat('history')}>Audio and transcripts</button>
            </div>
          </div>
          <div className="storage-choice">
            <span id="storage-from">From</span>
            <div className="segmented" role="group" aria-labelledby="storage-from">
              <button type="button" aria-pressed={from === 'before-today'} disabled={busy} onClick={() => setFrom('before-today')}>Before today</button>
              <button type="button" aria-pressed={from === 'everything'} disabled={busy} onClick={() => setFrom('everything')}>Every dictation</button>
            </div>
          </div>
          <p className="storage-consequence">
            {consequence(what, from)} {from === 'before-today' && 'Today’s dictations stay, counted from midnight on this computer. '}Models, settings, the dictionary and provider keys always stay.
          </p>
          {!preview && !confirmAll && (
            <div className="storage-actions">
              <button type="button" className="button danger" disabled={busy} onClick={() => void review()}>
                <Trash2 size={15} aria-hidden /> Review what will be deleted…
              </button>
            </div>
          )}

          {preview && (
            <div className="storage-confirm" role="alert">
              <strong>
                {preview.recordings === 0
                  ? 'Nothing to delete.'
                  : `Delete ${preview.history ? '' : 'the audio of '}${preview.recordings} dictation${preview.recordings === 1 ? '' : 's'}${preview.history ? ` and ${preview.jobs} transcription attempt${preview.jobs === 1 ? '' : 's'}` : ''}${preview.before_ms !== null ? ` from before ${new Date(preview.before_ms).toLocaleDateString()}` : ''}?`}
              </strong>
              {preview.recordings > 0 && <p>Download anything you want to keep first. This cannot be undone.</p>}
              <div className="storage-actions">
                <button type="button" className="button" disabled={busy} onClick={() => setPreview(undefined)}>{preview.recordings === 0 ? 'Close' : 'Keep them'}</button>
                {preview.recordings > 0 && (
                  <button type="button" className="button danger" disabled={busy} onClick={() => void clear()}>{busy ? 'Deleting…' : 'Delete permanently'}</button>
                )}
              </div>
            </div>
          )}

          {confirmAll && (
            <div className="storage-confirm" role="alert">
              <strong>Delete every recording and transcript?</strong>
              <p>
                Deletes saved recordings, raw and polished transcripts, attempt history and captured mode context from this {PRODUCT_NAME} installation. Exports,
                backups, clipboard copies and what providers already received are not erased. Download anything you want to keep first. This cannot be undone.
              </p>
              <div className="storage-actions">
                <button type="button" className="button" disabled={busy} onClick={() => setConfirmAll(false)}>Keep history</button>
                <button type="button" className="button danger" disabled={busy} onClick={() => void clearEverything()}>{busy ? 'Deleting…' : 'Delete permanently'}</button>
              </div>
            </div>
          )}
          {status && <p className="notice alert" role="alert">{status}</p>}
        </div>
      </Group>
    </>
  );
}
function message(cause: unknown) { return typeof cause === 'string' ? cause : cause instanceof Error ? cause.message : 'Space could not be freed. Review and try again after recording and processing finish.'; }
