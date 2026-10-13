import { audioDigest } from './hash';
import { apiFetch } from '../auth/session';
import type { Job, Recording, RecordingMetadata, TranscriptionRequest } from '@metrix-writer/contracts';

export class ApiFailure extends Error {
  constructor(message: string, public status: number) { super(message); }
}
export async function request<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await apiFetch(url, init);
  const body = await response.json().catch(() => null);
  if (!response.ok) throw new ApiFailure(body?.api_version === '1' && typeof body.error?.message === 'string' ? body.error.message : `Request failed (${response.status}). Saved audio is still available.`, response.status);
  if (body?.api_version !== '1' || !body.data) throw new Error('The server returned an invalid response. Saved audio is still available.');
  return body.data as T;
}
export async function ensureUploaded(metadata: RecordingMetadata, exportAudio: () => Promise<Blob>, progress: (message: string) => void = () => {}) {
  const path = `/api/v1/recordings/${metadata.id}`;
  const existing = async () => {
    const recording = await request<Recording>(path);
    if (recording.metadata.byte_length !== metadata.byte_length || recording.metadata.mime_type !== metadata.mime_type || recording.metadata.created_at_ms !== metadata.created_at_ms || recording.audio.state === 'deleted') throw new Error('Saved server audio does not match this session. Keep your browser copy.');
    return recording.audio.state === 'saved';
  };
  let legacySaved = false;
  try { legacySaved = await existing(); } catch (error) {
    if (!(error instanceof ApiFailure) || error.status !== 404) throw error;
  }
  const audio = await exportAudio();
  if (audio.size !== metadata.byte_length) throw new Error('Browser audio length changed. Restore saved recordings before retrying.');
  progress('Checking saved audio…');
  const sha256 = await audioDigest(audio);
  type Checkpoint = { session_id: string; offset: number; saved: boolean; sha256: string; expires_at_ms: number };
  const { id, created_at_ms, duration_ms, byte_length, mime_type, input_language, capture_state } = metadata;
  const identity = { metadata: { id, created_at_ms, duration_ms, byte_length, mime_type, input_language, capture_state }, sha256 };
  const uploadPath = `${path}/upload`;
  const begin = () => request<Checkpoint>(uploadPath, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ api_version: '1', data: identity }) });
  const validate = (cp: Checkpoint) => {
    if (!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(cp.session_id) || typeof cp.saved !== 'boolean' || !Number.isSafeInteger(cp.expires_at_ms) || cp.sha256 !== sha256 || !Number.isSafeInteger(cp.offset) || cp.offset < 0 || cp.offset > audio.size || (cp.saved && cp.offset !== audio.size)) throw new Error('Server upload checkpoint is invalid. Keep the saved audio.');
    return cp;
  };
  let cp: Checkpoint | undefined;
  try { cp = validate(await begin()); } catch (error) {
    // Older Writer servers expose multipart only. Never present this fallback as byte resume.
    if (!(error instanceof ApiFailure) || ![404, 405].includes(error.status)) throw error;
  }
  if (cp) {
    const resumed = cp.offset > 0 && !cp.saved;
    while (!cp.saved && cp.offset < audio.size) {
      progress(`${resumed ? 'Resuming' : 'Uploading'} saved audio · ${cp.offset.toLocaleString()} / ${audio.size.toLocaleString()} bytes confirmed`);
      const offset = cp.offset;
      const part = audio.slice(offset, offset + 1024 * 1024);
      try {
        cp = validate(await request<Checkpoint>(uploadPath, { method: 'PATCH', headers: { 'Content-Type': 'application/octet-stream', 'Upload-Session': cp.session_id, 'Upload-Offset': String(offset) }, body: part }));
        if (cp.offset !== offset + part.size) throw new Error('Server did not confirm the uploaded chunk. Retry saved audio.');
      } catch (cause) {
        // Query the durable server receipt after a lost acknowledgement. Never guess accepted bytes.
        const confirmed = validate(await begin());
        if (!confirmed.saved && confirmed.offset === offset && confirmed.session_id === cp.session_id) throw cause;
        cp = confirmed;
      }
    }
    if (!cp.saved) {
      progress('Verifying uploaded audio…');
      try { cp = validate(await request<Checkpoint>(`${uploadPath}/finish`, { method: 'POST', headers: { 'Upload-Session': cp.session_id } })); }
      catch (cause) { const confirmed = validate(await begin()); if (!confirmed.saved) throw cause; cp = confirmed; }
    }
    if (!cp.saved) throw new Error('Server has not durably acknowledged this audio. Retry the saved recording.');
    progress('Audio saved on server. Submitting transcription…');
    return;
  }
  if (legacySaved) return;
  progress('Retrying full upload from saved audio · this server does not support byte resume');
  const form = new FormData();
  form.append('metadata', JSON.stringify({ api_version: '1', data: { id, created_at_ms, duration_ms, byte_length, mime_type, input_language, capture_state } }));
  form.append('audio', audio, 'recording');
  try { await request<Recording>('/api/v1/recordings', { method: 'POST', body: form }); }
  catch (error) {
    // A conflict or lost response can mean the server already saved these exact bytes.
    try { if (!await existing()) throw error; } catch { throw error; }
  }
}
export const submit = (data: TranscriptionRequest) => request<Job>('/api/v1/transcriptions', {
  method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ api_version: '1', data }),
});
export const getJob = (id: string) => request<Job>(`/api/v1/jobs/${id}`);
export const cancelRetries = (id: string) => request<Job>(`/api/v1/jobs/${id}/cancel-retries`, { method: 'POST' });
