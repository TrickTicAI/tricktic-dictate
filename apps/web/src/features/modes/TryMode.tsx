import { useEffect, useRef, useState } from 'react';
import { modeApi, emptyContext, type ModeProfile, type ModePreview } from './api';
import { attempts, type Attempt } from '../transcripts/storage';
import { invoke, isTauri } from '../desktop-session/runtime/native';

export function TryMode({ profile, ready }: { profile: ModeProfile; ready: boolean }) {
  const [text, setText] = useState('I need these things from the shop apples milk and bread new paragraph let’s meet Tuesday actually Thursday at three');
  const [target, setTarget] = useState('');
  const [source, setSource] = useState('');
  const [saved, setSaved] = useState<Attempt[]>([]);
  const [result, setResult] = useState<ModePreview | null>(null);
  const [tested, setTested] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const epoch = useRef(0);
  useEffect(() => () => { ++epoch.current; }, []);
  const signature = JSON.stringify([profile, text, target, source]);
  async function loadSaved() {
    try {
      const values = isTauri() ? await invoke<Attempt[]>('saved_attempts') : await attempts();
      setSaved(values.filter(a => a.job?.status.state === 'succeeded').sort((a, b) => (b.job?.created_at_ms ?? 0) - (a.job?.created_at_ms ?? 0)).slice(0, 20));
      if (!values.some(a => a.job?.status.state === 'succeeded')) setError('No completed transcripts are available yet. Paste a sample below.');
    } catch { setError('Saved transcripts could not be loaded. You can paste a sample below.'); }
  }
  async function run() {
    const current = ++epoch.current;
    setBusy(true); setError('');
    try {
      const value = await modeApi.preview(profile, text, { ...emptyContext(), selected_text: target }, source || undefined);
      if (epoch.current === current) { setResult(value); setTested(signature); }
    } catch (e) { if (epoch.current === current) setError((e as Error).message); }
    finally { if (epoch.current === current) setBusy(false); }
  }
  return <details className="mode-disclosure">
    <summary>Try this mode <span>Compare a sample before saving</span></summary>
    <div className="mode-try">
      <button type="button" className="button" onClick={() => void loadSaved()}>Choose a saved transcript</button>
      {saved.length > 0 && <label>Sample source<select aria-label="Sample source" value={source} onChange={e => {
        setSource(e.target.value);
        const job = saved.find(a => a.job?.id === e.target.value)?.job;
        if (job?.status.state === 'succeeded') setText(job.status.result.transcript.raw_text);
      }}><option value="">Editable sample</option>{saved.map(a => <option key={a.job!.id} value={a.job!.id}>{new Date(a.job!.created_at_ms).toLocaleString()} · {a.job!.status.state === 'succeeded' ? a.job!.status.result.transcript.raw_text.slice(0, 65) : ''}</option>)}</select></label>}
      <label>Dictated text<textarea aria-label="Preview dictated text" rows={4} maxLength={20000} value={text} onChange={e => { setSource(''); setText(e.target.value); }} dir="auto" /></label>
      {profile.context.selected_text && <label>Selected text<textarea aria-label="Preview selected text" rows={3} value={target} onChange={e => setTarget(e.target.value)} /></label>}
      <p className="small muted">Uses this draft’s settings and selected model. {profile.text_model?.provider !== 'local' && profile.kind !== 'regular' ? 'This sends the sample to your cloud text provider.' : ''} Application and clipboard context are empty in this preview.</p>
      <button type="button" className="button" disabled={busy || !ready || !text.trim() || profile.kind === 'command' && !target.trim()} onClick={() => void run()}>{busy ? 'Processing sample…' : 'Preview result'}</button>
      {error && <p role="alert">{error}</p>}
      {result && <div className="mode-comparison">
        <div><strong>Original</strong><p dir="auto">{result.input_text}</p></div>
        <div><strong>Result · {(result.diagnostics.processing_ms / 1000).toFixed(2)} s</strong><p dir="auto">{result.text}</p></div>
      </div>}
      {result && signature !== tested && <p role="status">Settings or sample changed. Preview again to compare the current draft.</p>}
    </div>
  </details>;
}
