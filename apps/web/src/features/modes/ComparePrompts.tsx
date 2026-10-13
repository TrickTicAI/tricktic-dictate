import { useEffect, useRef, useState } from 'react';
import { Dialog } from '../shell/Dialog';
import { attempts, type Attempt } from '../transcripts/storage';
import { invoke, isTauri } from '../desktop-session/runtime/native';
import { emptyContext, modeApi, defaultFormatting, type ModeProfile, type PromptVersion } from './api';
import { median, score, type Score } from './scoring';

const SAMPLES = [3, 5, 10, 20] as const;
const DRAFT = 'draft';
interface Run { version: string; job: string; ms: number; text: string; input: string; score: Score }

/** The draft as a version, so the thing being changed can be compared with what is saved. */
const asVersion = (profile: ModeProfile): PromptVersion => ({
  id: DRAFT,
  name: 'Current draft',
  instructions: profile.instructions,
  formatting: profile.formatting ?? defaultFormatting,
  fast_processing: profile.fast_processing ?? true,
  created_at_ms: 0,
});

/**
 * Runs prompt versions over dictations that were really recorded.
 *
 * A prompt is only as good as what it does to the way one particular person
 * speaks, so the samples here are that person's own saved transcripts rather
 * than invented ones. Each version is sent through the same processing the mode
 * uses, and what comes back is scored against the transcript it came from: words
 * gone, questions gone, fillers left. Nothing is applied until it is chosen.
 */
export function ComparePrompts({ profile, onClose, onUse, onSaveVersion }: {
  profile: ModeProfile;
  onClose: () => void;
  /** Adopt a version into the mode being edited; saving stays the mode page's job. */
  onUse: (version: PromptVersion) => void;
  onSaveVersion: (version: PromptVersion) => void;
}) {
  const versions = [asVersion(profile), ...(profile.prompt_versions ?? [])];
  const [chosen, setChosen] = useState<string[]>(versions.slice(0, 2).map(v => v.id));
  const [count, setCount] = useState<number>(5);
  const [saved, setSaved] = useState<Attempt[] | null>(null);
  const [runs, setRuns] = useState<Run[]>([]);
  const [running, setRunning] = useState(0);
  const [error, setError] = useState('');
  const [naming, setNaming] = useState('');
  const [expanded, setExpanded] = useState<string | null>(null);
  const live = useRef(true);
  useEffect(() => () => { live.current = false; }, []);
  useEffect(() => {
    (isTauri() ? invoke<Attempt[]>('saved_attempts') : attempts())
      .then(values => { if (live.current) setSaved(values.filter(a => a.job?.status.state === 'succeeded').sort((a, b) => (b.job?.created_at_ms ?? 0) - (a.job?.created_at_ms ?? 0))); })
      .catch(() => { if (live.current) { setSaved([]); setError('Saved transcripts could not be read, so there is nothing to compare against.'); } });
  }, []);
  const samples = (saved ?? []).slice(0, count);
  const picked = versions.filter(v => chosen.includes(v.id));

  async function run() {
    setError(''); setRuns([]); setRunning(picked.length * samples.length);
    const queue = picked.flatMap(version => samples.map(sample => ({ version, sample })));
    const results: Run[] = [];
    // Two at a time: the preview route runs two, and a third would only queue.
    await Promise.all(Array.from({ length: Math.min(2, queue.length) }, async () => {
      for (;;) {
        const next = queue.shift();
        if (!next || !live.current) return;
        const job = next.sample.job!;
        const variant: ModeProfile = { ...profile, instructions: next.version.instructions, formatting: next.version.formatting, fast_processing: next.version.fast_processing, process_while_speaking: false };
        try {
          const preview = await modeApi.preview(variant, '', emptyContext(), job.id);
          results.push({ version: next.version.id, job: job.id, ms: preview.diagnostics.processing_ms, text: preview.text, input: preview.input_text, score: score(preview.input_text, preview.text) });
        } catch (e) {
          if (live.current) setError((e as Error).message);
        }
        if (live.current) { setRuns([...results]); setRunning(n => n - 1); }
      }
    }));
    if (live.current) setRunning(0);
  }

  const summary = (id: string) => {
    const mine = runs.filter(r => r.version === id);
    if (!mine.length) return null;
    return {
      edits: mine.length,
      ms: median(mine.map(r => r.ms)) ?? 0,
      lost: median(mine.map(r => r.score.lost)) ?? 0,
      dropped: mine.filter(r => r.score.lostQuestion || r.score.lostEnding).length,
      leftovers: mine.reduce((sum, r) => sum + r.score.leftovers, 0),
      language: mine.filter(r => r.score.changedLanguage).length,
    };
  };

  return <Dialog title="Compare prompt versions" onClose={onClose}>
    <div className="dialog-body compare-prompts">
      <p className="dialog-note">Runs each version over your own recent dictations through this mode’s model, and scores what came back against the transcript it came from. Nothing changes until you choose a version. Each run is a paid request to your provider.</p>

      <div className="compare-setup">
        <fieldset>
          <legend>Versions to compare</legend>
          {versions.map(version => <label className="checkbox" key={version.id}>
            <input type="checkbox" checked={chosen.includes(version.id)} disabled={!!running}
              onChange={() => setChosen(current => current.includes(version.id) ? current.filter(id => id !== version.id) : [...current, version.id])} />
            <span>{version.name}<span className="dialog-note"> {version.formatting.rewrite} · {version.fast_processing ? 'fast' : 'balanced'} · {version.instructions.length} characters</span></span>
          </label>)}
        </fieldset>
        <label className="field">
          <span>Dictations to test</span>
          <select aria-label="Dictations to test" value={count} disabled={!!running} onChange={e => setCount(Number(e.target.value))}>
            {SAMPLES.map(n => <option key={n} value={n}>Last {n}</option>)}
          </select>
        </label>
      </div>
      <p className="dialog-note">{saved === null ? 'Reading your saved transcripts…' : `${samples.length} of your ${saved.length} saved transcripts, newest first. ${picked.length * samples.length} requests.`}</p>
      {error && <p className="custom-models-error" role="alert">{error}</p>}

      <div className="dialog-actions">
        <button type="button" className="button primary" disabled={!!running || picked.length < 1 || !samples.length} onClick={() => void run()}>
          {running ? `Running… ${runs.length} of ${runs.length + running}` : 'Run comparison'}
        </button>
      </div>

      {runs.length > 0 && <table className="compare-results">
        <thead><tr><th>Version</th><th>Typical</th><th>Words lost</th><th>Dropped</th><th>Fillers left</th><th><span className="visually-hidden">Actions</span></th></tr></thead>
        <tbody>
          {picked.map(version => {
            const totals = summary(version.id);
            if (!totals) return null;
            return <tr key={version.id}>
              <td>{version.name}<span className="dialog-note"> {totals.edits} edits</span></td>
              <td>{(totals.ms / 1000).toFixed(2)} s</td>
              <td>{(100 * totals.lost).toFixed(1)}%</td>
              <td>{totals.dropped}{totals.language ? ` · ${totals.language} language` : ''}</td>
              <td>{totals.leftovers}</td>
              <td>{version.id !== DRAFT && <button type="button" className="button" onClick={() => onUse(version)}>Use this version</button>}</td>
            </tr>;
          })}
        </tbody>
      </table>}
      {runs.length > 0 && <p className="dialog-note">Words lost counts meaningful words from the transcript that the result does not have; some of that is filler removal doing its job. Dropped counts results that lost a question or most of their last sentence. Fillers left counts “um” and repeated words still in the result.</p>}

      {runs.length > 0 && samples.map(sample => {
        const job = sample.job!;
        const mine = runs.filter(r => r.job === job.id);
        if (!mine.length) return null;
        return <details key={job.id} className="compare-sample" open={expanded === job.id} onToggle={e => setExpanded((e.currentTarget as HTMLDetailsElement).open ? job.id : null)}>
          <summary>{new Date(job.created_at_ms).toLocaleString()}<span> {mine[0].input.slice(0, 60)}…</span></summary>
          <p className="dialog-note" dir="auto">{mine[0].input}</p>
          {mine.map(runResult => <div key={runResult.version} className="compare-output">
            <strong>{versions.find(v => v.id === runResult.version)?.name} · {(runResult.ms / 1000).toFixed(2)} s · {(100 * runResult.score.lost).toFixed(0)}% lost{runResult.score.lostQuestion ? ' · lost a question' : ''}{runResult.score.lostEnding ? ' · lost the ending' : ''}</strong>
            <p dir="auto">{runResult.text}</p>
          </div>)}
        </details>;
      })}

      <form className="compare-save" onSubmit={e => {
        e.preventDefault();
        const name = naming.trim();
        if (!name) return;
        const version: PromptVersion = { id: crypto.randomUUID(), name, instructions: profile.instructions, formatting: profile.formatting ?? defaultFormatting, fast_processing: profile.fast_processing ?? true, created_at_ms: Date.now() };
        onSaveVersion(version);
        // A version is saved in order to compare it, so it joins the comparison.
        setChosen(current => [...current, version.id]);
        setNaming('');
      }}>
        <label className="field">
          <span>Save the current draft as a version</span>
          <input type="text" aria-label="Version name" value={naming} maxLength={80} placeholder="Keeps every question" onChange={e => setNaming(e.target.value)} />
        </label>
        <button type="submit" className="button" disabled={!naming.trim() || (profile.prompt_versions?.length ?? 0) >= 10}>Save version</button>
      </form>

      <div className="dialog-actions custom-models-done">
        <button type="button" className="button" onClick={onClose}>Done</button>
      </div>
    </div>
  </Dialog>;
}
