import { useState } from 'react';
import { request } from '../transcripts/api';

/** Downloads only the API's bounded timing projection, never the history objects. */
export function LatencyExport() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  async function download() {
    setBusy(true); setError('');
    try {
      const measurements = await request<{ records: unknown[] }>('/api/v1/statistics/latency');
      const url = URL.createObjectURL(new Blob([JSON.stringify(measurements, null, 2)], { type: 'application/json' }));
      const link = document.createElement('a'); link.href = url; link.download = 'dictation-stage-timings.json'; link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (failure) { setError(failure instanceof Error ? failure.message : 'Timing export unavailable.'); }
    finally { setBusy(false); }
  }
  return <div className="statistics-latency-export">
    <button type="button" className="btn" disabled={busy} onClick={() => void download()}>{busy ? 'Exporting…' : 'Export stage timings'}</button>
    <p className="muted">Latest 256 measured attempts, with missing stages kept empty. Includes timing and model identifiers; no audio, transcript or application context.</p>
    {error && <p role="alert">{error}</p>}
  </div>;
}
