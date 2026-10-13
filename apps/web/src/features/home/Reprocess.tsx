import { useEffect, useState } from 'react';
import type { Job, Settings } from '@metrix-writer/contracts';
import type { Recording } from '../../recorder';
import type { ModeSettings } from '../modes/api';
import { ensureUploaded, request } from '../transcripts/api';
import { successfulJob } from './statistics';

type Source = { recording_id: string; job_id: string | null; version_id?: string | null };
type Version = { id: string; source: Source; raw_text: string; text: string; reused_from: string | null };
type Item = { source: Source; state: string; unavailable: string | null; error: string | null; previous_text: string | null; version: Version | null; reuse_available: boolean };
type Batch = { id: string; state: string; provider_model: string; paid_work: boolean; likely_paid_items: number; items: Item[]; request: { operation: string; options: Settings['transcription']; profile: { fallback_model?: {provider: string; model: string} | null } | null } };
const post = (data: unknown): RequestInit => ({ method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ api_version: '1', data }) });

/** Historical processing only reads capture/jobs; it has no delivery callback. */
export function Reprocess({ recordings, saved, settings, modes, exportAudio }: {
  recordings: Recording[]; saved: Record<string, { job?: Job; history?: Job[] }>;
  settings: Settings; modes: ModeSettings | null; exportAudio: (id: string) => Promise<Blob>;
}) {
  const [open, setOpen] = useState(false);
  const [chosen, setChosen] = useState<string[]>([]);
  const [operation, setOperation] = useState<'transcribe' | 'cleanup'>('transcribe');
  const [profileId, setProfileId] = useState('');
  const [rawVersions, setRawVersions] = useState<Record<string, string>>({});
  const [batches, setBatches] = useState<Batch[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [paid, setPaid] = useState<Record<string, boolean>>({});
  const profiles = modes?.profiles.filter(p => p.kind !== 'command') ?? [];
  const profile = profiles.find(p => p.id === profileId) ?? profiles.find(p => p.id === modes?.active) ?? profiles[0];
  async function reload() { setBatches(await request<Batch[]>('/api/v1/batches')); }
  useEffect(() => {
    if (!open) return;
    let gone = false;
    const load = () => request<Batch[]>('/api/v1/batches').then(value => { if (!gone) setBatches(value); }).catch(e => { if (!gone) setError(e.message); });
    void load();
    const timer = window.setInterval(() => void load(), 1000);
    return () => { gone = true; window.clearInterval(timer); };
  }, [open]);
  async function action(run: () => Promise<unknown>) {
    setBusy(true); setError('');
    try { await run(); await reload(); } catch (e) { setError((e as Error).message); } finally { setBusy(false); }
  }
  async function plan() {
    const sources: Source[] = [];
    for (const id of chosen) {
      const recording = recordings.find(r => r.id === id);
      if (!recording) continue;
      if (operation === 'transcribe') {
        // Upload remains independent of inference; one missing browser copy does
        // not prevent the server from planning the other selected recordings.
        try { await ensureUploaded(recording.metadata, () => exportAudio(id)); } catch { /* The durable plan reports missing server audio per item. */ }
      }
      const raw = rawVersions[id];
      sources.push({ recording_id: id, job_id: raw?.startsWith('version:') ? null : raw?.startsWith('job:') ? raw.slice(4) : successfulJob(saved[id])?.id ?? null,
        version_id: raw?.startsWith('version:') ? raw.slice(8) : null });
    }
    await request<Batch>('/api/v1/batches', post({ id: crypto.randomUUID(), sources, operation,
      options: settings.transcription, profile: operation === 'cleanup' ? profile : null }));
  }
  return <section className="reprocess" aria-label="Reprocess saved recordings">
    <button type="button" className="btn secondary" aria-expanded={open} onClick={() => setOpen(!open)}>Reprocess saved recordings</button>
    {open && <div className="reprocess-body">
      <p>Select up to 32 items and review a plan. Each output is a separate version; historical results are never pasted automatically. Restarted batches wait for explicit resume.</p>
      <label>Operation <select aria-label="Reprocessing operation" value={operation} onChange={e => setOperation(e.target.value as typeof operation)}>
        <option value="transcribe">Transcribe saved audio</option><option value="cleanup">Clean up saved raw text</option>
      </select></label>
      {operation === 'cleanup' && <label>Cleanup mode <select aria-label="Batch cleanup mode" value={profile?.id ?? ''} onChange={e => setProfileId(e.target.value)}>
        {profiles.map(p => <option key={p.id} value={p.id}>{p.name} · {p.text_model?.model ?? 'dictionary and snippets'}</option>)}
      </select></label>}
      <div className="reprocess-selection" role="group" aria-label="Batch selection">
        {recordings.map(r => <div key={r.id}><label>
          <input type="checkbox" aria-label={`Select recording ${r.id}`} checked={chosen.includes(r.id)} disabled={!chosen.includes(r.id) && chosen.length >= 32}
            onChange={e => setChosen(ids => e.target.checked ? [...ids, r.id] : ids.filter(id => id !== r.id))} />
          {r.createdAt.toLocaleString()} · {successfulJob(saved[r.id])?.status.state === 'succeeded' ? 'Saved text' : 'No successful text'}
          {!r.bytes ? ' · Audio unavailable; cleanup remains possible' : ' · Saved audio; availability verified in plan'}
        </label>
        {operation === 'cleanup' && chosen.includes(r.id) && <label>Raw version <select aria-label={`Raw version for recording ${r.id}`} value={rawVersions[r.id] ?? `job:${successfulJob(saved[r.id])?.id ?? ''}`}
          onChange={e => setRawVersions(values => ({...values, [r.id]:e.target.value}))}>
          {[...new Map([saved[r.id]?.job, ...(saved[r.id]?.history ?? [])].filter((job): job is Job => !!job && job.status.state === 'succeeded').map(job => [job.id, job])).values()].map(job => <option key={job.id} value={`job:${job.id}`}>Saved raw attempt {job.id} · {job.request.options.selection.model}</option>)}
          {batches.flatMap(batch => batch.items).filter(item => item.state === 'succeeded' && item.source.recording_id === r.id && item.version).map(item => <option key={item.version!.id} value={`version:${item.version!.id}`}>Historical raw version {item.version!.id}</option>)}
          {!successfulJob(saved[r.id]) && <option value="job:">No successful original raw version</option>}
        </select></label>}
        </div>)}
      </div>
      <button type="button" className="btn secondary" disabled={busy || !chosen.length || (operation === 'cleanup' && !profile)} onClick={() => void action(plan)}>Review batch plan ({chosen.length})</button>
      {error && <p role="alert">{error}</p>}
      {batches.map(batch => <article key={batch.id} className="reprocess-batch" aria-label={`Batch ${batch.id}`}>
        <h3>{batch.request.operation === 'transcribe' ? 'Saved-audio transcription' : 'Saved-text cleanup'} · {batch.state}</h3>
        <p>{batch.provider_model} · {batch.items.length} selected · {batch.items.filter(i => i.state === 'succeeded').length} completed</p>
        {batch.request.profile?.fallback_model && <p>Fallback: {batch.request.profile.fallback_model.provider} / {batch.request.profile.fallback_model.model}</p>}
        <p>Language: {batch.request.options.input_language} · {batch.request.options.output_language.mode === 'translate' ? 'Explicit translation' : 'Preserve output languages'}</p>
        {batch.paid_work && <p>{batch.likely_paid_items} likely paid items; up to {batch.items.length} selected items may call cloud models. Fallbacks can add requests; provider rates apply. Interrupted requests may already have been billed.</p>}
        {batch.items.some(i => i.reuse_available) && <p>Matching successful stages can be reused. Changed or unverifiable stages run again.</p>}
        {['planned', 'paused'].includes(batch.state) && <>
          {batch.paid_work && <label><input type="checkbox" checked={!!paid[batch.id]} onChange={e => setPaid(p => ({...p, [batch.id]: e.target.checked}))} /> I understand this batch may incur cloud charges</label>}
          <button type="button" className="btn primary" disabled={busy || (batch.paid_work && !paid[batch.id])} onClick={() => void action(() => request(`/api/v1/batches/${batch.id}/start`, post({ acknowledge_paid: !!paid[batch.id] })))}>{batch.state === 'paused' ? 'Resume batch explicitly' : 'Start reviewed batch'}</button>
        </>}
        {['planned', 'paused', 'running'].includes(batch.state) && <button type="button" className="btn secondary" disabled={busy} onClick={() => void action(() => request(`/api/v1/batches/${batch.id}/cancel`, {method: 'POST'}))}>Cancel batch</button>}
        {batch.state !== 'running' && <button type="button" className="btn secondary" disabled={busy} onClick={() => void action(() => request(`/api/v1/batches/${batch.id}`, {method: 'DELETE'}))}>Delete batch versions</button>}
        <ol>{batch.items.map(item => <li key={item.source.recording_id}>
          <strong>{recordings.find(r => r.id === item.source.recording_id)?.createdAt.toLocaleString() ?? item.source.recording_id} · {item.state}</strong>
          {(item.unavailable || item.error) && <p>{item.unavailable || item.error}</p>}
          {item.version && <details><summary>Compare version {item.version.id}{item.version.reused_from ? ' · reused successful stage' : ''}</summary>
            <p>Source recording: {item.version.source.recording_id} · source raw version: {item.version.source.version_id ?? item.version.source.job_id ?? 'saved audio'}</p>
            <h4>Earlier result</h4><pre dir="auto">{item.previous_text ?? 'No earlier text'}</pre>
            <h4>Raw text retained in this version</h4><pre dir="auto">{item.version.raw_text}</pre>
            <h4>New result</h4><pre dir="auto">{item.version.text}</pre>
          </details>}
        </li>)}</ol>
      </article>)}
    </div>}
  </section>;
}
