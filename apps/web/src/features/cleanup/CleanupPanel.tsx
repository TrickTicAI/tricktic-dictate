import { apiFetch } from '../auth/session';
import { useEffect, useRef, useState } from 'react';
import type { Job, ModelSelection } from '@metrix-writer/contracts';
import './cleanup.css';

type Choice = { selection: ModelSelection; configured: boolean; execution_location: 'cloud' };
export type CleanupPanelProps = { job: Job; onJobChange?: (job: Job) => void; showOriginal?: boolean };

async function request<T>(url: string, options: RequestInit = {}): Promise<T> {
  const response = await apiFetch(url, options);
  const body = await response.json();
  if (!response.ok || body.api_version !== '1' || !('data' in body)) {
    throw new Error(body.error?.message || 'Cleanup request failed. Your original transcript remains available.');
  }
  return body.data as T;
}

/** Only a button press starts a billable cleanup call. Mounting/reloading only reads status. */
export function CleanupPanel(props: CleanupPanelProps) {
  return <Panel key={props.job.id} {...props} />;
}
function Panel({ job, onJobChange, showOriginal = true }: CleanupPanelProps) {
  const [current, setCurrent] = useState(job);
  const [choices, setChoices] = useState<Choice[]>([]);
  const [selected, setSelected] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState('');
  const [copyStatus, setCopyStatus] = useState('');
  const callback = useRef(onJobChange);
  const mounted = useRef(true);
  callback.current = onJobChange;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { setCurrent(job); }, [job]);
  useEffect(() => {
    const controller = new AbortController();
    void request<Choice[]>('/api/v1/cleanup/providers', { signal: controller.signal }).then((values) => {
      setChoices(values);
      setSelected(values.find((value) => value.configured)?.selection.provider ?? values[0]?.selection.provider ?? '');
    }).catch(() => { if (!controller.signal.aborted) setError('Could not load cleanup providers. Your original transcript is available.'); });
    return () => controller.abort();
  }, []);
  const pending = current.status.state === 'succeeded' && current.status.result.cleanup.state === 'pending';
  useEffect(() => {
    if (!pending) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const updated = await request<Job>(`/api/v1/jobs/${current.id}`, { signal: controller.signal });
        if (controller.signal.aborted) return;
        setCurrent(updated);
        callback.current?.(updated);
        setError('');
        if (updated.status.state !== 'succeeded' || updated.status.result.cleanup.state !== 'pending') return;
      } catch {
        if (controller.signal.aborted) return;
        setError('Cleanup status could not be refreshed. Your original transcript is available; retry is safe.');
      }
      timer = setTimeout(() => void poll(), 1000);
    }
    timer = setTimeout(() => void poll(), 500);
    return () => { controller.abort(); clearTimeout(timer); };
  }, [current.id, pending]);

  if (current.status.state !== 'succeeded') return null;
  const { transcript, cleanup } = current.status.result;
  const choice = choices.find((value) => value.selection.provider === selected);
  async function start() {
    if (!choice || sending) return;
    setSending(true); setError(''); setCopyStatus('');
    try {
      const updated = await request<Job>(`/api/v1/jobs/${current.id}/cleanup`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ api_version: '1', data: choice.selection }),
      });
      if (!mounted.current) return;
      setCurrent(updated); callback.current?.(updated);
    } catch (cause) {
      if (mounted.current) setError(cause instanceof Error ? cause.message : 'Cleanup failed. The original transcript is available.');
    } finally { if (mounted.current) setSending(false); }
  }
  async function copy(text: string) {
    try { await navigator.clipboard.writeText(text); setCopyStatus('Cleaned text copied.'); }
    catch { setCopyStatus('Clipboard unavailable. Select and copy the cleaned text below.'); }
  }
  return <section className="cleanup-panel" aria-label="Optional transcript cleanup">
    {showOriginal && <label>Original transcript<textarea aria-label="Original transcript" readOnly dir="auto" value={transcript.raw_text} /></label>}
    <h3>Optional cleanup</h3>
    <p>Adjust punctuation, capitalization and spacing while keeping every word and language. Your original transcript stays available.</p>
    <p>Cleanup sends editable text to the selected cloud provider. Exact snippets and email tokens stay on this server.</p>
    <label>Cleanup provider<select value={selected} onChange={(event) => setSelected(event.target.value)} disabled={sending}>
      {choices.map((value) => <option key={value.selection.provider} value={value.selection.provider}>
        {value.selection.provider === 'openai' ? 'OpenAI' : 'Gemini'} · {value.selection.model}{value.configured ? '' : ' (server key missing)'}
      </option>)}
    </select></label>
    {choice && !choice.configured && <p>Configure this provider’s API key on the server to enable cleanup.</p>}
    <button type="button" disabled={sending || !choice?.configured} onClick={() => void start()}>
      {sending ? 'Requesting cleanup…' : cleanup.state === 'off' ? 'Clean up transcript' : 'Retry cleanup'}
    </button>
    {pending && <p role="status">Cleanup pending. If the server restarted or this was interrupted, retry cleanup. Retrying an active attempt does not start a second call.</p>}
    {cleanup.state === 'failed' && <p role="alert">Cleanup failed: {cleanup.error.message}</p>}
    {error && <p role="alert">{error}</p>}
    {cleanup.state === 'succeeded' && <div>
      <label>Cleaned transcript<textarea aria-label="Cleaned transcript" readOnly dir="auto" value={cleanup.cleaned_text} /></label>
      <button type="button" onClick={() => void copy(cleanup.cleaned_text)}>Copy cleaned text</button>
      <p>Review formatting before using this version.</p>
    </div>}
    {copyStatus && <p role="status">{copyStatus}</p>}
  </section>;
}
