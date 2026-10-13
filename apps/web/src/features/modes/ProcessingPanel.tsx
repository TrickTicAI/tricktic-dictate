import { ContextNotice } from './ContextNotice';
import { useEffect, useRef, useState } from 'react';
import { emptyContext, modeApi, type ContextSnapshot, type ModeSettings, type ProcessRequest, type ProcessingAttempt } from './api';
import './modes.css';

export function ProcessingPanel({ jobId, onChange }: { jobId: string; onChange?: () => void }) {
  const [settings, setSettings] = useState<ModeSettings | null>(null);
  const [selected, setSelected] = useState('');
  const [context, setContext] = useState<ContextSnapshot>(emptyContext);
  const [attempts, setAttempts] = useState<ProcessingAttempt[]>([]);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const uncertain = useRef<ProcessRequest | null>(null);
  const epoch = useRef(0);
  const changed = useRef(onChange); changed.current = onChange;
  const profile = settings?.profiles.find(p => p.id === selected);
  const pending = attempts.some(a => a.state.state === 'pending');
  useEffect(() => {
    const current = ++epoch.current;
    uncertain.current = null; setContext(emptyContext()); setAttempts([]); setError(''); setBusy(false);
    Promise.all([modeApi.settings(), modeApi.attempts(jobId)]).then(([s, a]) => {
      if (epoch.current === current) { setSettings(s); setSelected(s.active); setAttempts(a); }
    }).catch(e => { if (epoch.current === current) setError(e.message); });
    return () => { ++epoch.current; };
  }, [jobId]);
  useEffect(() => {
    if (!pending) return;
    const current = epoch.current;
    const timer = setTimeout(() => {
      modeApi.attempts(jobId).then(a => { if (epoch.current === current) { setAttempts(a); setError(''); changed.current?.(); } }).catch(e => { if (epoch.current === current) setError(e.message); });
    }, 1000);
    return () => clearTimeout(timer);
  }, [jobId, pending, attempts]);
  async function run() {
    if (!profile || busy || pending) return;
    const current = epoch.current;
    setBusy(true); setError('');
    const request = uncertain.current ?? { operation_id: crypto.randomUUID(), profile_id: profile.id, profile_revision: profile.revision, context: { ...context } };
    uncertain.current = request;
    try {
      const attempt = await modeApi.process(jobId, request);
      if (epoch.current !== current) return;
      uncertain.current = null; setContext(emptyContext());
      setAttempts(previous => [...previous.filter(a => a.id !== attempt.id), attempt]);
      changed.current?.();
    } catch (e) { if (epoch.current === current) setError((e as Error).message); }
    finally { if (epoch.current === current) setBusy(false); }
  }
  async function copy(text: string) {
    try { await navigator.clipboard.writeText(text); }
    catch { setError('Clipboard unavailable. Select the result and copy it manually.'); }
  }
  async function readClipboard() {
    try { setContext(c => ({ ...c, clipboard: '' })); const text = await navigator.clipboard.readText(); setContext(c => ({ ...c, clipboard: text })); }
    catch { setError('Clipboard access unavailable. Paste the context into the field.'); }
  }
  return <details className="writer-modes" open={pending}>
    <summary>Process with a mode</summary>
    {profile && <fieldset disabled={busy || pending}>
      <label>Processing mode<select aria-label="Processing mode" value={selected} onChange={e => { setSelected(e.target.value); uncertain.current = null; }}>{settings?.profiles.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}</select></label>
      {profile.kind === 'regular' ? <p>Apply vocabulary and snippets without an AI text request.</p> : <p>{profile.text_model ? `${profile.text_model.model} · ${profile.text_model.provider === 'local' ? 'local' : 'cloud'} text processing` : 'Choose a text model in Settings → Modes first.'}</p>}
      <ContextNotice profile={profile} />
      {profile.kind !== 'regular' && <>
        {profile.context.selected_text && <label>Selected text to {profile.kind === 'command' ? 'transform' : 'use as context'}<textarea rows={4} value={context.selected_text} onChange={e => { setContext({ ...context, selected_text: e.target.value }); uncertain.current = null; }} /></label>}
        {profile.context.application && <label>Application context<textarea rows={2} value={context.application} onChange={e => { setContext({ ...context, application: e.target.value }); uncertain.current = null; }} /></label>}
        {profile.context.clipboard && <label>Clipboard context<textarea rows={2} value={context.clipboard} onChange={e => { setContext({ ...context, clipboard: e.target.value }); uncertain.current = null; }} /><button className="button" onClick={() => void readClipboard()}>Read clipboard</button></label>}
      </>}
      <button className="button primary" disabled={profile.kind !== 'regular' && (!profile.text_model || profile.kind === 'command' && !context.selected_text.trim())} onClick={() => void run()}>{uncertain.current ? 'Retry same request' : 'Process saved transcript'}</button>
    </fieldset>}
    {pending && <p role="status">Processing… the original transcript remains available.</p>}
    {error && <p role="alert">{error}</p>}
    {[...attempts].reverse().map(a => <article key={a.id} aria-label={`${a.profile.name} result`}>
      <p><strong>{a.profile.name}</strong> · revision {a.profile.revision} · {new Date(a.created_at_ms).toLocaleString()}</p>
      {a.state.state === 'succeeded' && <><p className="mode-result" dir="auto">{a.state.text}</p><button className="button" onClick={() => a.state.state === 'succeeded' && void copy(a.state.text)}>Copy result</button></>}
      {a.state.state === 'failed' && <p role="alert">{a.state.error.message}</p>}
      <details><summary>Processing details</summary><p>{a.diagnostics && <>Processing: {(a.diagnostics.processing_ms / 1000).toFixed(2)} s{a.diagnostics.reused_draft ? (a.diagnostics.prepared_at_release ? " · prepared at release" : " · prepared during speech") : ""}. </>}Prompt version {a.prompt_version}; {a.diagnostics?.original_used ? 'Original used after polishing failed or timed out' : a.diagnostics?.fallback_used ? `${a.profile.fallback_model?.model ?? 'fallback model'} (fallback for ${a.profile.text_model?.model ?? 'no text model'})` : a.profile.text_model?.model ?? 'no text model'}{a.diagnostics?.served_by ? ` via ${a.diagnostics.served_by}` : ''}</p><pre className="mode-result">{a.profile.instructions}</pre>{a.diagnostics?.model_attempts?.map((m, i) => <p key={i}>{m.role}: {m.selection.model} · {m.state}{m.local_capacity_exhausted ? ' (local processing busy)' : m.error ? ` (${m.error.replaceAll('_', ' ')})` : ''} · {(m.elapsed_ms / 1000).toFixed(2)} s</p>)}<p>Input transcript</p><pre className="mode-result" dir="auto">{a.input_text}</pre>{Object.entries(a.request.context).filter(([, value]) => value).map(([key, value]) => <div key={key}><p>{key.replaceAll('_', ' ')}</p><pre className="mode-result" dir="auto">{value}</pre></div>)}</details>
    </article>)}
  </details>;
}
