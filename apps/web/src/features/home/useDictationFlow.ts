import { useEffect, useRef, useState } from 'react';
import type { Job } from '@metrix-writer/contracts';
import type { Snapshot } from '../desktop-session/contracts/protocol';
import type { SessionOwner } from '../desktop-session/runtime/owner';
import type { DeliveryResult } from '../desktop-session/runtime/delivery';
import type { Recording } from '../../recorder';

/**
 * Where the dictation started from this window or by the shortcut is, as one
 * word every part of Home agrees on: the dock, the live stage and the reader.
 *
 * - `opening`: the microphone is being opened; nothing is recorded yet.
 * - `recording`: audio is being captured.
 * - `transcribing`: saved, and waiting for or inside speech recognition.
 * - `processing`: recognised; the mode is shaping the text and it is being
 *   delivered (pasted or copied) where the cursor was.
 * - `done` / `failed`: the outcome, shown for a few seconds and then let go.
 *
 * Only the live dictation drives it. Transcribing an older recording again
 * from history is shown on that recording's row and in the reader, and never
 * takes the stage over.
 */
export type Flow = 'idle' | 'opening' | 'recording' | 'transcribing' | 'processing' | 'done' | 'failed';

export interface DictationFlow {
  flow: Flow;
  /** The dictation the flow is about; null at rest. */
  sessionId: string | null;
  /** What happened to the finished text (desktop only). */
  delivery?: DeliveryResult;
  /** Milliseconds from letting go to the outcome, measured here. */
  turnaroundMs?: number;
  errorCode?: string | null;
  /** Saved without transcription, because no speech model was ready. */
  recordOnly?: boolean;
}

/** How long an outcome stays in the dock before it returns to rest. */
export const OUTCOME_MS = 4500;

/* The live session outlives Home: switching to Statistics and back while a
   dictation is still being transcribed must not forget which one it was. */
let liveSession: string | null = null;

function fromSnapshot(snapshot: Snapshot): Flow {
  if (snapshot.state === 'requesting' || snapshot.state === 'capturing') liveSession = snapshot.session_id;
  if (!snapshot.session_id || snapshot.session_id !== liveSession) return 'idle';
  switch (snapshot.state) {
    case 'requesting': return 'opening';
    case 'capturing': return 'recording';
    case 'saving': case 'queued': return 'transcribing';
    case 'transcribing': return snapshot.progress === 'processing_text' ? 'processing' : 'transcribing';
    case 'success': return 'done';
    case 'error': return 'failed';
    // `saved` here is a deliberate record-only dictation (no model is ready),
    // which is an outcome too; `canceled` was discarded and has none.
    case 'saved': return 'done';
    default: return 'idle';
  }
}

/** The desktop build: the session owner is the single source of truth. */
function useOwnerFlow(owner: SessionOwner | undefined): DictationFlow {
  const [state, setState] = useState<DictationFlow>({ flow: 'idle', sessionId: null });
  useEffect(() => {
    if (!owner) return;
    let current: DictationFlow = { flow: 'idle', sessionId: null };
    let stoppedAt: number | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const commit = (next: DictationFlow) => { current = next; setState(next); };
    const read = (snapshot: Snapshot) => {
      const flow = fromSnapshot(snapshot);
      if (flow === 'recording' || flow === 'opening') stoppedAt = null;
      else if (flow === 'transcribing' && stoppedAt === null) stoppedAt = performance.now();
      const sessionId = flow === 'idle' ? null : snapshot.session_id;
      if (current.flow === flow && current.sessionId === sessionId) return;
      clearTimeout(timer);
      const outcome = flow === 'done' || flow === 'failed';
      if (outcome) timer = setTimeout(() => { if (current.flow === flow) commit({ flow: 'idle', sessionId: null }); }, OUTCOME_MS);
      commit({
        flow,
        sessionId,
        delivery: outcome ? owner.delivery : undefined,
        turnaroundMs: outcome && stoppedAt !== null ? performance.now() - stoppedAt : undefined,
        errorCode: snapshot.error_code,
        recordOnly: snapshot.state === 'saved',
      });
    };
    read(owner.snapshot);
    const offSnapshot = owner.subscribe((event) => { if (event.kind === 'snapshot') read(event); });
    // Delivery settles just after the session reports success; the outcome
    // shown is the one that actually happened, not "Processing the captured mode…".
    const offDelivery = owner.subscribeDelivery(() => { if (current.flow === 'done') commit({ ...current, delivery: owner.delivery }); });
    return () => { clearTimeout(timer); offSnapshot(); offDelivery(); };
  }, [owner]);
  return state;
}

/**
 * The browser build has no session owner: the recorder reports capture, and
 * the newest recording's job reports the rest. It never pastes anything.
 */
function useBrowserFlow(active: boolean, captureState: 'idle' | 'requesting' | 'recording' | 'stopping', recordings: Recording[], saved: Record<string, { job?: Job }>, working: Record<string, boolean>): DictationFlow {
  const before = useRef<Set<string> | null>(null);
  const [outcome, setOutcome] = useState<{ id: string; flow: 'done' | 'failed' } | null>(null);
  const settled = useRef(new Set<string>());
  if (active && captureState === 'requesting' && !before.current) before.current = new Set(recordings.map((item) => item.id));
  const fresh = before.current ? recordings.find((item) => !before.current!.has(item.id)) : undefined;
  const job = fresh ? saved[fresh.id]?.job : undefined;
  const finished = !!fresh && captureState === 'idle' && !working[fresh.id] && (job?.status.state === 'succeeded' || job?.status.state === 'failed');
  useEffect(() => {
    if (!active || !fresh || !finished || settled.current.has(fresh.id)) return;
    settled.current.add(fresh.id);
    before.current = null;
    setOutcome({ id: fresh.id, flow: job?.status.state === 'failed' ? 'failed' : 'done' });
    const timer = setTimeout(() => setOutcome(null), OUTCOME_MS);
    return () => clearTimeout(timer);
  }, [active, fresh, finished, job]);
  if (!active) return { flow: 'idle', sessionId: null };
  if (captureState === 'requesting') return { flow: 'opening', sessionId: null };
  if (captureState === 'recording') return { flow: 'recording', sessionId: null };
  if (captureState === 'stopping') return { flow: 'transcribing', sessionId: null };
  if (fresh && working[fresh.id]) return { flow: 'transcribing', sessionId: fresh.id };
  if (outcome) return { flow: outcome.flow, sessionId: outcome.id };
  return { flow: 'idle', sessionId: null };
}

export function useDictationFlow({ owner, captureState, recordings, saved, working }: {
  owner?: SessionOwner;
  captureState: 'idle' | 'requesting' | 'recording' | 'stopping';
  recordings: Recording[];
  saved: Record<string, { job?: Job }>;
  working: Record<string, boolean>;
}): DictationFlow {
  const desktop = useOwnerFlow(owner);
  const browser = useBrowserFlow(!owner, captureState, recordings, saved, working);
  return owner ? desktop : browser;
}
