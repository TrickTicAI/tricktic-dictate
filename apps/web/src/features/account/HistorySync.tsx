import { useEffect, useRef, useState } from 'react';
import type { RemoteRecord } from '@metrix-writer/contracts/account-sync';
import { invoke, listen } from '../desktop-session/runtime/native';
import { Group, Row } from '../settings/controls';
import { Dialog } from '../shell/Dialog';
import { portableMessage } from './portable';

type Mode = 'future_only' | 'include_existing';
type Preview = { id: string; mode: Mode; local_records: number; trust: string };
type Status = { not_synced: number; joined: boolean; pending: number; copies: number; error: string | null; last_success_ms: number | null };
type Page = { records: RemoteRecord[]; next: string | null };
const TRUST = 'firebase-service-readable-v1';

/** This view accepts text records only; it never constructs a recording or job. */
export function HistorySync({ signedIn, available }: { signedIn: boolean; available: boolean }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [setup, setSetup] = useState(false);
  const [mode, setMode] = useState<Mode>('future_only');
  const [preview, setPreview] = useState<Preview | null>(null);
  const [ack, setAck] = useState(false);
  const [records, setRecords] = useState<RemoteRecord[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const viewing = useRef(false);
  const [source, setSource] = useState<'local' | 'cloud'>('local');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const reload = () => { void invoke<Status | null>('transcript_status').then(setStatus).catch(() => {}); };
  useEffect(() => {
    reload(); let disposed = false; let stop: (() => void) | undefined;
    void Promise.resolve().then(() => listen<{ kind: string }>('writer-sync', event => {
      reload();
      if (event.kind === 'history' && viewing.current) {
        void invoke<Page>('transcript_page', { after: null, restore: false }).then(p => {
          setRecords(p.records); setNext(p.next); setSource('local');
        }).catch(() => {});
      }
    })).then(fn => { if (disposed) fn(); else stop = fn; }).catch(() => {});
    return () => { disposed = true; stop?.(); };
  }, []);
  const run = async (action: () => Promise<void>) => {
    setBusy(true); setError('');
    try { await action(); } catch (e) { setError(portableMessage(e)); }
    finally { setBusy(false); reload(); }
  };
  const page = async (restore: boolean, more = false) => {
    const result = await invoke<Page>('transcript_page', { after: more ? next : null, restore });
    viewing.current = true;
    setSource(restore ? 'cloud' : 'local');
    setRecords(old => more ? [...new Map([...old, ...result.records].map(r => [r.id, r])).values()] : result.records);
    setNext(result.next);
  };
  const joined = status?.joined ?? false;
  return <>
    <Group title="Transcript history">
      <Row title={joined ? 'Transcript sync is on' : 'Transcript sync is off'}
        detail={<>History has its own consent. Audio, provider keys and application context are never included. Text is stored in Google Firebase and is readable by Google and authorized service administrators; it is not end-to-end encrypted.
          {status?.not_synced ? ` ${status.not_synced} attempts exceed the sync text limits or use an unsupported model identifier; their saved text stays local.` : ''}
          {status?.pending ? ` ${status.pending} text changes waiting to upload.` : ''}
          {status?.last_success_ms ? ` Last upload check: ${new Date(status.last_success_ms).toLocaleString()}.` : ''}
          {status?.error && <span role="alert"> {portableMessage(status.error)}</span>}</>}>
        {available && signedIn && <>
          <button className="button" disabled={busy} onClick={() => { setSetup(true); setPreview(null); setAck(false); }}>Set up history…</button>
          {joined && <button className="button" disabled={busy} onClick={() => void run(async () => { await invoke('transcript_turn_off'); })}>Turn off history sync</button>}
        </>}
      </Row>
      <Row title="Read saved text" detail="Restore one page at a time. Restoring alone uploads nothing. Turning sync off leaves cloud text in place; use account deletion controls to remove it. Text copies have no audio and cannot retry transcription, play a recording or paste automatically.">
        <button className="button" disabled={busy} onClick={() => void run(() => page(false))}>View local text copies</button>
        {available && signedIn && <button className="button" disabled={busy} onClick={() => void run(() => page(true))}>Restore a page from account</button>}
      </Row>
      {error && <p className="notice alert" role="alert">{error}</p>}
      {records.map(r => {
        if (r.category !== 'transcripts' || r.content.state !== 'live') return null;
        const t = r.content.data;
        const original = t.kind === 'original';
        return <Row key={r.id} stacked title={`${t.kind === 'original' ? 'Original attempt' : t.kind === 'edited' ? 'Edited version' : t.kind === 'corrected' ? 'Dictionary result' : 'Cleaned result'} · text only — no audio`}
          detail={<><time>{new Date(original ? t.recorded_at_ms : t.created_at_ms).toLocaleString()}</time> · Source device: {r.origin_device_id}
            {!original && <span> · Original: {t.original_record_id} · Parent: {t.parent_record_id}</span>}
            <span dir="auto" style={{ display: 'block', whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{t.text}</span></>}>
          <button className="button" onClick={() => void run(() => navigator.clipboard.writeText(t.text))}>Copy text</button>
          <button className="button" disabled title="Audio was not backed up">Play audio</button>
          <button className="button" disabled title="Audio was not backed up">Retry transcription</button>
          {joined && <>
            <button className="button" disabled={busy} onClick={() => setEditing({ id: r.id, text: t.text })}>Save an edited version…</button>
            <button className="button danger" disabled={busy} onClick={() => setDeleting(r.id)}>Delete synced {original ? 'attempt and versions' : 'version'}…</button>
          </>}
        </Row>;
      })}
      {next && <button className="button" disabled={busy} onClick={() => void run(() => page(source === 'cloud', true))}>Load next page</button>}
    </Group>
    {setup && <Dialog title="Set up transcript history" onClose={() => setSetup(false)}><div className="dialog-body">
      {!preview ? <>
        <p>Choose which local text to upload. Other devices keep their own enrollment boundary. No audio is uploaded.</p>
        <label className="checkbox"><input type="radio" name="history-mode" checked={mode === 'future_only'} onChange={() => setMode('future_only')} />Future attempts only</label>
        <label className="checkbox"><input type="radio" name="history-mode" checked={mode === 'include_existing'} onChange={() => setMode('include_existing')} />Include existing history shown in the preview</label>
        <button className="button" disabled={busy} onClick={() => void run(async () => setPreview(await invoke<Preview>('transcript_preview', { mode })))}>Preview history upload</button>
      </> : <>
        <p>{preview.local_records} existing text records will be enrolled. Future completed attempts will also sync. Raw, corrected, cleaned and edited text stay separate; concurrent edited versions are all kept.</p>
        <label className="checkbox"><input type="checkbox" checked={ack} onChange={e => setAck(e.target.checked)} />I agree to upload transcript text to Firebase, where Google and authorized service administrators can read it. This is not end-to-end encrypted.</label>
        <button className="button primary" disabled={busy || !ack || preview.trust !== TRUST} onClick={() => void run(async () => { await invoke('transcript_confirm', { id: preview.id, trust: TRUST }); setSetup(false); })}>Enable transcript sync</button>
      </>}
      {error && <p role="alert">{error}</p>}
    </div></Dialog>}
    {editing && <Dialog title="Save a separate edited version" onClose={() => setEditing(null)}><div className="dialog-body">
      <p>The original and every other version remain available.</p>
      <textarea aria-label="Edited text" dir="auto" value={editing.text} onChange={e => setEditing({ ...editing, text: e.target.value })} />
      <button className="button primary" disabled={busy} onClick={() => void run(async () => { await invoke('transcript_edit', { parent: editing.id, text: editing.text }); setEditing(null); await page(false); })}>Save version</button>
      {error && <p role="alert">{error}</p>}
    </div></Dialog>}
    {deleting && <Dialog title="Delete synced text?" onClose={() => setDeleting(null)}><div className="dialog-body">
      <p>Deleting an original also removes all its synced versions. Other devices receive the deletion when they refresh history. Local recordings and original job results stay on their source device. Service recovery records may retain text until you use Delete cloud data in Account settings.</p>
      <button className="button danger" disabled={busy} onClick={() => void run(async () => { await invoke('transcript_delete', { id: deleting }); setDeleting(null); await page(false); })}>Delete synced text</button>
      {error && <p role="alert">{error}</p>}
    </div></Dialog>}
  </>;
}
