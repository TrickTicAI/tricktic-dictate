import { useCallback, useEffect, useRef, useState } from 'react';
import type { InputLanguage, RecordingMetadata } from '@metrix-writer/contracts';
import { DurableCapture, type CaptureController, type CaptureSnapshot } from './features/recorder/capture';
import type { LocalSession } from './features/recorder/storage';
import { PRODUCT_SLUG } from './product';

export type Language = InputLanguage;
export type Recording = {
  id: string;
  url: string;
  filename: string;
  bytes: number;
  duration: number;
  language: Language;
  createdAt: Date;
  downloaded: boolean;
  metadata: RecordingMetadata;
};

export function recordingFilename(metadata: RecordingMetadata) {
  const extension = metadata.mime_type.includes('wav') ? 'wav' : metadata.mime_type.includes('mp4') ? 'm4a' : metadata.mime_type.includes('ogg') ? 'ogg' : 'webm';
  return `${PRODUCT_SLUG}-${new Date(metadata.created_at_ms).toISOString().replace(/[:.]/g, '-')}.${extension}`;
}

export interface ExternalRecorderOwner {
  capture: CaptureController;
  start(): Promise<unknown>;
  stop(): Promise<unknown>;
}
export function useRecorder(language: Language, owner?: ExternalRecorderOwner) {
  const controller = useRef<CaptureController | null>(null);
  const [capture, setCapture] = useState<CaptureSnapshot>({ state: 'idle', error: null, elapsed: 0 });
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [storageError, setStorageError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [elapsed, setElapsed] = useState(0);
  const selected = useRef<{ id: string; url: string } | null>(null);
  const selectionVersion = useRef(0);
  // A slow startup/manual scan must not overwrite a dictation saved meanwhile.
  const historyReads = useRef(new Set<Map<string, LocalSession | undefined>>());
  const supported = owner?.capture.supported ?? (window.isSecureContext && !!navigator.mediaDevices?.getUserMedia && typeof MediaRecorder !== 'undefined');

  const select = useCallback(async (id: string) => {
    const current = controller.current;
    if (!current) return;
    const version = ++selectionVersion.current;
    try {
      const blob = await current.exportRecording(id);
      if (controller.current !== current || version !== selectionVersion.current) return;
      if (!blob.size) throw new Error('empty');
      const url = URL.createObjectURL(blob);
      if (selected.current) URL.revokeObjectURL(selected.current.url);
      selected.current = { id, url };
      setRecordings((items) => items.map((item) => ({ ...item, url: item.id === id ? url : '' })));
      setStorageError(null);
    } catch {
      if (controller.current === current) setStorageError('Could not read saved audio. It may be empty, unavailable or removed by the browser. Try restoring the recording list.');
    }
  }, []);

  const refresh = useCallback(async (recordingId?: string) => {
    const current = controller.current;
    if (!current) return;
    const updates = recordingId ? undefined : new Map<string, LocalSession | undefined>();
    if (updates) historyReads.current.add(updates);
    setLoading(true);
    try {
      // A native stop changes one journal. Do not enumerate the whole history
      // while its transcription handoff is trying to use the same disk.
      const saved = recordingId ? await current.store.get(recordingId) : undefined;
      let sessions = recordingId ? (saved ? [saved] : []) : await current.list();
      if (controller.current !== current) return;
      if (recordingId) historyReads.current.forEach(pending => pending.set(recordingId, saved));
      if (updates?.size) {
        const merged = new Map(sessions.map(session => [session.id, session]));
        updates.forEach((metadata, id) => { if (metadata) merged.set(id, metadata); else merged.delete(id); });
        sessions = [...merged.values()].sort((a, b) => b.created_at_ms - a.created_at_ms);
      }
      setRecordings((previous) => {
        const updated = sessions.map((metadata) => ({
          id: metadata.id, url: selected.current?.id === metadata.id ? selected.current.url : '',
          filename: recordingFilename(metadata), bytes: metadata.byte_length, duration: metadata.duration_ms,
          language: metadata.input_language, createdAt: new Date(metadata.created_at_ms), metadata,
          downloaded: previous.find((item) => item.id === metadata.id)?.downloaded ?? false,
        }));
        return recordingId
          ? [...updated, ...previous.filter(item => item.id !== recordingId)].sort((a, b) => b.createdAt.getTime() - a.createdAt.getTime())
          : updated;
      });
      setStorageError(null);
      const latest = sessions.find((session) => session.byte_length > 0);
      if (latest && (!selected.current || selected.current.id !== latest.id)) await select(latest.id);
    } catch {
      if (controller.current === current) setStorageError('Could not open browser recording storage. Allow site storage or free space, then restore or retry. Existing saved audio has not been deleted.');
    } finally {
      if (updates) historyReads.current.delete(updates);
      if (controller.current === current) setLoading(false);
    }
  }, [select]);
  const restore = useCallback(() => refresh(), [refresh]);

  useEffect(() => {
    const current = owner?.capture ?? new DurableCapture();
    controller.current = current;
    setCapture(current.snapshot);
    setElapsed(current.elapsed);
    const unsubscribe = current.subscribe(() => {
      setCapture(current.snapshot);
      if (current.snapshot.state === 'idle') { setElapsed(current.snapshot.elapsed); void refresh(current.snapshot.recordingId); }
    });
    void restore();
    return () => {
      unsubscribe();
      controller.current = null;
      selectionVersion.current++;
      if (!owner) current.dispose();
      if (selected.current) URL.revokeObjectURL(selected.current.url);
      selected.current = null;
    };
  }, [restore, refresh, owner]);

  useEffect(() => {
    if (capture.state !== 'recording') return;
    setElapsed(controller.current?.elapsed ?? 0);
    const timer = window.setInterval(() => setElapsed(controller.current?.elapsed ?? 0), 250);
    const warn = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ''; };
    window.addEventListener('beforeunload', warn);
    return () => { window.clearInterval(timer); window.removeEventListener('beforeunload', warn); };
  }, [capture.state]);

  const start = useCallback(() => owner ? owner.start() : controller.current?.start(language), [language, owner]);
  const stop = useCallback(() => owner ? owner.stop() : controller.current?.stop(), [owner]);
  const exportRecording = useCallback(async (id: string) => {
    if (!controller.current) throw new Error('Recorder is not ready.');
    return controller.current.exportRecording(id);
  }, []);
  const deleteRecording = useCallback(async (id: string) => {
    if (!controller.current) return;
    try {
      await controller.current.deleteRecording(id);
      selectionVersion.current++;
      if (selected.current?.id === id) { URL.revokeObjectURL(selected.current.url); selected.current = null; }
      await restore();
    } catch {
      setStorageError('Could not delete this recording. Stop capture first, then retry deleting.');
    }
  }, [restore, owner]);
  const markDownloaded = (id: string) => setRecordings((previous) => previous.map((item) => item.id === id ? { ...item, downloaded: true } : item));
  // An open microphone that has sent no sound yet reads as starting here too,
  // as it does on the panel, so nobody is invited to speak into the silence.
  const state = capture.state === 'recording' && capture.listening === false ? 'requesting' : capture.state;
  return { state, recordings, selectedId: selected.current?.id ?? null,
    error: capture.error ?? storageError, elapsed, supported, loading, start, stop,
    // The capture bar meters this directly in the browser build; the desktop
    // build reads real native levels instead and never sees a stream.
    stream: controller.current?.activeStream,
    restore, list: () => controller.current?.list(), select, exportRecording, deleteRecording, markDownloaded };
}

export function formatDuration(milliseconds: number) {
  const seconds = Math.floor(milliseconds / 1000);
  return `${String(Math.floor(seconds / 60)).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`;
}
