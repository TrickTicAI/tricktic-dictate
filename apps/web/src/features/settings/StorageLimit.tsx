import { useEffect, useState } from 'react';
import { invoke } from '../desktop-session/runtime/native';
import { size, STORAGE_LIMIT_CHANGED } from './storageBudget';

/** The shell's storage limit: in force, default, fixed by the environment, free disk. */
export interface Limit { limit_bytes: number; default_bytes: number; fixed_by_environment: boolean; free_bytes: number | null }
const GB = 1024 ** 3;
export const LIMIT_CHOICES = [5, 10, 20, 50, 100];

/*
 * How much saved dictation data may use. A choice applies at once and is
 * kept across restarts. A limit below what is saved only makes storage read
 * as full; nothing is deleted. One larger than the drive can hold is allowed,
 * with a warning.
 */
export function StorageLimit({ used, onChanged }: { used: number; onChanged: () => void }) {
  const [limit, setLimit] = useState<Limit>();
  const [custom, setCustom] = useState(false);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    invoke<Limit>('storage_limit').then((value) => {
      setLimit(value);
      const gb = value.limit_bytes / GB;
      if (!LIMIT_CHOICES.includes(gb)) { setCustom(true); setDraft(Number.isInteger(gb) ? String(gb) : ''); }
    }).catch(() => setError('The storage limit could not be read.'));
  }, []);
  if (!limit) return error ? <p className="notice alert" role="alert">{error}</p> : null;

  const current = limit.limit_bytes / GB;
  const fixed = limit.fixed_by_environment;
  async function choose(gigabytes: number) {
    setBusy(true); setError('');
    try {
      setLimit(await invoke<Limit>('storage_limit_set', { gigabytes }));
      window.dispatchEvent(new Event(STORAGE_LIMIT_CHANGED));
      onChanged();
    } catch (cause) {
      setError(typeof cause === 'string' ? cause : 'The storage limit could not be saved. Try again.');
    } finally { setBusy(false); }
  }
  const typed = Number(draft);
  const validDraft = /^\d+$/.test(draft.trim()) && typed >= 1;
  const tight = limit.free_bytes !== null && limit.limit_bytes > limit.free_bytes + used;

  return (
    <div className="storage-limit">
      <div className="storage-choice">
        <span id="storage-limit">Storage limit</span>
        <div className={`segmented${fixed ? ' disabled' : ''}`} role="group" aria-labelledby="storage-limit">
          {LIMIT_CHOICES.map((gb) => (
            <button key={gb} type="button" aria-pressed={!custom && current === gb} disabled={fixed || busy}
              onClick={() => { setCustom(false); if (current !== gb) void choose(gb); }}>
              {gb} GB{gb * GB === limit.default_bytes ? ' (default)' : ''}
            </button>
          ))}
          <button type="button" aria-pressed={custom} disabled={fixed || busy} onClick={() => { setCustom(true); setDraft(Number.isInteger(current) ? String(current) : ''); }}>Custom</button>
        </div>
      </div>
      {custom && !fixed && (
        <form className="storage-choice" onSubmit={(event) => { event.preventDefault(); if (validDraft) void choose(typed); }}>
          <label htmlFor="storage-limit-custom">Custom limit</label>
          <input id="storage-limit-custom" type="number" min={1} step={1} inputMode="numeric" value={draft} disabled={busy}
            onChange={(event) => setDraft(event.target.value)} aria-describedby="storage-limit-unit" />
          <span id="storage-limit-unit">GB</span>
          <button type="submit" className="button" disabled={busy || !validDraft || typed === current}>Set limit</button>
        </form>
      )}
      <p className="storage-note">{size(used)} of {size(limit.limit_bytes)} used.</p>
      {fixed && <p className="storage-note">Set by WRITER_DEVICE_STORAGE_BYTES on this computer, so it can’t be changed here.</p>}
      {!fixed && used > limit.limit_bytes && <p className="storage-note">This is below what is already saved, so storage reads as full until you free space. Nothing is deleted.</p>}
      {tight && <p className="notice warn" role="status">This drive has only {size(limit.free_bytes!)} free, so it may fill up before this limit.</p>}
      {error && <p className="notice alert" role="alert">{error}</p>}
    </div>
  );
}
