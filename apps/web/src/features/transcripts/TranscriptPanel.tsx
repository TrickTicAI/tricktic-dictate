import { useEffect, useState } from 'react';
import type { Job } from '@metrix-writer/contracts';
import { CleanupPanel } from '../cleanup';
import { request } from './api';
import { PRODUCT_SLUG } from '../../product';

export function TranscriptPanel({ job, onJobChange }: { job: Job; onJobChange?: (job: Job) => void }) {
  const [copyStatus, setCopyStatus] = useState('');
  const [processed, setProcessed] = useState<string | null>(null);
  const [processedError, setProcessedError] = useState(false);
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    if (job.status.state !== 'succeeded') return;
    const controller = new AbortController();
    request<{ text: string }>(`/api/v1/jobs/${job.id}/processed`, { signal: controller.signal }).then(value => {
      if (!controller.signal.aborted) { setProcessed(value.text); setProcessedError(false); }
    }).catch(() => { if (!controller.signal.aborted) setProcessedError(true); });
    return () => controller.abort();
  }, [job.id, job.status.state, refresh]);
  const result = job.status.state === 'succeeded' ? job.status.result : null;
  async function copy(text: string, label = 'original') {
    try { await navigator.clipboard.writeText(text); setCopyStatus(`Copied ${label} text`); }
    catch { setCopyStatus('Clipboard unavailable. Select the original text and copy it manually.'); }
  }
  function download(text: string) {
    const url = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }));
    const link = document.createElement('a'); link.href = url; link.download = `${PRODUCT_SLUG}-${job.id}.txt`; link.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  return <section className="transcript-result" aria-label="Transcription attempt">
    <p className="attempt-label">{job.request.options.selection.provider} · {job.request.options.selection.model} · {job.request.options.input_language} · {new Date(job.created_at_ms).toLocaleString()}</p>
    <p role="status">{job.status.state === 'queued' ? 'Queued for transcription' : job.status.state === 'transcribing' ? 'Transcribing saved audio…' : job.status.state === 'failed' ? 'Transcription failed. Audio is saved for retry.' : 'Transcription complete'}</p>
    {job.status.state === 'failed' && <p className="error-message" role="alert">{job.status.error.message}</p>}
    {result && <><h3>Original transcript</h3><textarea aria-label="Original transcript" dir="auto" readOnly value={result.transcript.raw_text} /><div className="text-actions"><button onClick={() => void copy(result.transcript.raw_text)}>Copy original</button><button onClick={() => download(result.transcript.raw_text)}>Download text</button></div><p role="status">{copyStatus}</p></>}
  {result && processed !== null && processed !== result.transcript.raw_text && <><h3>With corrections and snippets</h3><textarea aria-label="Processed transcript" dir="auto" readOnly value={processed} /><div className="text-actions"><button onClick={() => void copy(processed, 'processed')}>Copy processed text</button><button onClick={() => download(processed)}>Download processed text</button></div></>}
    {processedError && <p role="alert">Could not load corrections and snippets. Original text remains available. <button onClick={() => setRefresh(value => value + 1)}>Reload processed text</button></p>}
    {result && onJobChange && <CleanupPanel job={job} onJobChange={onJobChange} showOriginal={false} />}
    {result && !onJobChange && result.cleanup.state === 'succeeded' && <><h3>Previous cleaned text</h3><textarea aria-label="Previous cleaned transcript" dir="auto" readOnly value={result.cleanup.cleaned_text} /></>}
  </section>;
}
