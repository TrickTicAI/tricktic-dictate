import { useEffect, useRef, useState } from 'react';
import { Check, PenLine, Star, Trash2 } from 'lucide-react';
import { removeReference, saveReference, type Reference } from '../transcripts/references';
import { learningApi, suggestionKey, type Suggestion } from '../vocabulary/learning';
import { LearnFromCorrection } from './LearnFromCorrection';

/**
 * "Correct transcript": what the user actually said, saved beside what was heard.
 *
 * The raw transcript is never replaced; the correction is a reference for the
 * personal accuracy set, and Gold marks the clips that count in it. A drafted
 * reference whose agent drafts disagreed arrives here to be checked.
 */
export function CorrectTranscript({ recordingId, jobId, heard, dir, reference, editing, onEditing, onSaved }: {
  recordingId: string;
  jobId: string;
  /** The raw transcript, the starting text when nothing was saved yet. */
  heard: string;
  dir: 'ltr' | 'rtl' | 'auto';
  reference?: Reference;
  editing: boolean;
  onEditing: (editing: boolean) => void;
  onSaved: (reference: Reference | null) => void;
}) {
  const [text, setText] = useState('');
  const [gold, setGold] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [suggestions, setSuggestions] = useState<Suggestion[]>([]);
  const field = useRef<HTMLTextAreaElement>(null);

  const shown = useRef(recordingId);
  shown.current = recordingId;
  useEffect(() => setSuggestions([]), [recordingId]);

  useEffect(() => {
    if (!editing) return;
    setText(reference?.text ?? heard);
    setGold(reference?.gold ?? true);
    setError('');
    window.setTimeout(() => field.current?.focus(), 0);
  }, [editing, reference, heard]);

  async function run(action: () => Promise<Reference | null>) {
    setBusy(true); setError('');
    try { onSaved(await action()); onEditing(false); return true; }
    catch (failure) { setError(`${(failure as Error).message} What was heard is unaffected.`); return false; }
    finally { setBusy(false); }
  }
  const save = (value: string, asGold: boolean) => run(() => saveReference(recordingId, jobId, value, asGold));
  /** A typed correction may teach new words; asking is optional and never blocks the save. */
  async function correct(value: string, asGold: boolean) {
    if (!await save(value, asGold)) return;
    const asked = recordingId;
    learningApi.suggest(heard, value).then((found) => { if (asked === shown.current) setSuggestions(found); }, () => setSuggestions([]));
  }

  if (editing) {
    return (
      <form className="correction" aria-label="Correct transcript" onSubmit={(event) => { event.preventDefault(); void correct(text, gold); }}>
        <header><PenLine size={15} aria-hidden /> <strong>Correct transcript</strong></header>
        <p className="small muted">Type what you actually said. What was heard stays saved as it was; this is kept beside it.</p>
        {reference?.needs_check && reference.alternative && (
          <div className="correction-alternative">
            <span className="small muted">The other draft:</span>
            <p dir={dir}>{reference.alternative}</p>
            <button type="button" className="button small ghost" onClick={() => setText(reference.alternative ?? '')}>Use this draft</button>
          </div>
        )}
        <textarea ref={field} dir={dir} value={text} rows={Math.min(12, Math.max(3, Math.ceil(text.length / 70)))} aria-label="What you said" onChange={(event) => setText(event.target.value)} />
        <label className="check">
          <input type="checkbox" checked={gold} onChange={(event) => setGold(event.target.checked)} /> Gold: use this clip to measure accuracy
        </label>
        {error && <p className="notice alert" role="alert">{error}</p>}
        <div className="correction-actions">
          <button type="submit" className={`button primary${busy ? ' busy' : ''}`} disabled={busy || !text.trim()}><Check size={15} aria-hidden /> Save correction</button>
          <button type="button" className="button ghost" disabled={busy} onClick={() => onEditing(false)}>Cancel</button>
        </div>
      </form>
    );
  }
  if (!reference) return null;
  return (
    <section className={`correction${reference.needs_check ? ' to-check' : ''}`} aria-label="Corrected transcript">
      <header>
        <PenLine size={15} aria-hidden /> <strong>{reference.needs_check ? 'Draft to check' : reference.source === 'draft' ? 'Drafted reference' : 'Your correction'}</strong>
        {reference.gold && <span className="chip plain"><Star size={12} aria-hidden /> Gold</span>}
      </header>
      {reference.needs_check && <p className="small muted">Two drafts of this clip disagree. Listen, then correct or confirm it.</p>}
      <p className="transcript-text" dir={dir}>{reference.text}</p>
      {error && <p className="notice alert" role="alert">{error}</p>}
      <div className="correction-actions">
        <button type="button" className={`button small${reference.needs_check ? ' primary' : ''}`} disabled={busy} onClick={() => onEditing(true)}>
          <PenLine size={14} aria-hidden /> {reference.needs_check ? 'Check now' : 'Edit'}
        </button>
        {!reference.needs_check && (
          <button type="button" className="button small ghost" aria-pressed={reference.gold} disabled={busy} onClick={() => void save(reference.text, !reference.gold)}>
            <Star size={14} aria-hidden /> {reference.gold ? 'Remove from gold' : 'Mark gold'}
          </button>
        )}
        <button type="button" className="button small ghost" disabled={busy} onClick={() => void run(() => removeReference(recordingId))}>
          <Trash2 size={14} aria-hidden /> Remove
        </button>
      </div>
      <LearnFromCorrection
        suggestions={suggestions}
        onDecided={(done) => setSuggestions((all) => all.filter((s) => suggestionKey(s) !== suggestionKey(done)))}
      />
    </section>
  );
}
