/** Desktop control protocol v1. Audio, credentials and transcript bodies never travel here. */
export const protocolVersion = 1;
export const maxMessageBytes = 4096;
export const maxLevelHz = 20;
export const states = ['idle', 'requesting', 'capturing', 'saving', 'saved', 'queued', 'transcribing', 'success', 'canceled', 'error'] as const;
export type SessionState = typeof states[number];
export type CommandAction = 'start' | 'stop' | 'cancel' | 'retry' | 'snapshot';
// IDs are UUIDs. session_id is the existing recording ID, allocated before permission.
// epoch is a new UUID per owner-document lifetime; revision is monotonic within it.
export interface Command {
  version: 1; kind: 'command'; epoch: string; command_id: string;
  action: CommandAction; session_id: string | null;
}
export interface Snapshot {
  version: 1; kind: 'snapshot'; epoch: string; revision: number;
  session_id: string | null; job_id: string | null; state: SessionState;
  audio_saved: boolean; error_code: string | null;
  progress?: 'waiting_for_capacity' | 'retrying_1' | 'retrying_2' | 'processing_text' | null;
}
export interface Level {
  version: 1; kind: 'level'; epoch: string; revision: number;
  session_id: string; sequence: number; rms: number; peak: number;
}
export interface Ack {
  version: 1; kind: 'ack'; epoch: string; command_id: string;
  outcome: 'accepted' | 'duplicate' | 'rejected'; error_code: string | null;
}
export type DesktopMessage = Command | Snapshot | Level | Ack;
export interface DesktopSession {
  readonly snapshot: Snapshot;
  dispatch(command: Command): Promise<Ack>;
  subscribe(listener: (event: Snapshot | Level) => void): () => void;
  dispose(): Promise<void>; // stop/drain; never delete
}
export interface ShortcutEdge {
  gesture_id: string; mode: 'hold' | 'toggle'; edge: 'down' | 'up' | 'cancel'; repeat: boolean;
}
const transitions: Record<SessionState, readonly SessionState[]> = {
  idle: ['requesting'], requesting: ['capturing', 'canceled', 'error'],
  capturing: ['saving', 'error'], saving: ['saved', 'canceled', 'error'],
  saved: ['queued', 'error', 'requesting'], queued: ['transcribing', 'success', 'error', 'canceled'],
  transcribing: ['queued', 'success', 'error', 'canceled'], success: ['queued', 'requesting'],
  canceled: ['queued', 'requesting'], error: ['queued', 'requesting'],
};
/** Same-session phase legality only. Identity, durable-write and job evidence are owner checks. */
export function canTransition(from: SessionState, to: SessionState): boolean {
  return from === to || transitions[from].includes(to);
}
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const code = /^[a-z][a-z0-9_]{0,63}$/;
const id = (v: unknown): v is string => typeof v === 'string' && uuid.exec(v)?.[0] === v;
const nullableId = (v: unknown) => v === null || id(v);
const counter = (v: unknown) => Number.isSafeInteger(v) && (v as number) >= 0;
const errorCode = (v: unknown) => v === null || typeof v === 'string' && code.exec(v)?.[0] === v;
const keys = (v: Record<string, unknown>, expected: string[]) => Object.keys(v).sort().join(',') === expected.sort().join(',');
/** Reject unknown fields/kinds/versions before dispatch. The transport must check origin too. */
export function parseMessage(json: string): DesktopMessage {
  if (new TextEncoder().encode(json).length > maxMessageBytes) throw new Error('message_too_large');
  const v: unknown = JSON.parse(json);
  if (!v || typeof v !== 'object' || Array.isArray(v)) throw new Error('invalid_message');
  const m = v as Record<string, unknown>;
  const base = ['version', 'kind', 'epoch'];
  let valid = m.version === 1 && id(m.epoch);
  switch (m.kind) {
    case 'command': valid &&= keys(m, [...base, 'command_id', 'action', 'session_id']) && id(m.command_id) &&
      ['start', 'stop', 'cancel', 'retry', 'snapshot'].includes(m.action as string) &&
      (m.action === 'snapshot' ? m.session_id === null : id(m.session_id)); break;
    case 'snapshot': valid &&= keys(m, [...base, 'revision', 'session_id', 'job_id', 'state', 'audio_saved', 'error_code', ...('progress' in m ? ['progress'] : [])]) &&
      (m.progress == null || ['queued', 'transcribing'].includes(m.state as string) && ['waiting_for_capacity', 'retrying_1', 'retrying_2', 'processing_text'].includes(m.progress as string)) &&
      counter(m.revision) && nullableId(m.session_id) && nullableId(m.job_id) && states.includes(m.state as SessionState) &&
      typeof m.audio_saved === 'boolean' && errorCode(m.error_code) &&
      (m.state === 'idle' ? m.session_id === null && m.job_id === null && !m.audio_saved : id(m.session_id)) &&
      (!['queued', 'transcribing', 'success'].includes(m.state as string) || id(m.job_id) && m.audio_saved === true) &&
      (m.state !== 'saved' || m.audio_saved === true) && (m.state === 'error' ? m.error_code !== null : m.error_code === null); break;
    case 'level': valid &&= keys(m, [...base, 'revision', 'session_id', 'sequence', 'rms', 'peak']) && counter(m.revision) &&
      id(m.session_id) && counter(m.sequence) && typeof m.rms === 'number' && typeof m.peak === 'number' &&
      Number.isFinite(m.rms) && Number.isFinite(m.peak) && m.rms >= 0 && m.rms <= m.peak && m.peak <= 1; break;
    case 'ack': valid &&= keys(m, [...base, 'command_id', 'outcome', 'error_code']) && id(m.command_id) &&
      ['accepted', 'duplicate', 'rejected'].includes(m.outcome as string) && errorCode(m.error_code) &&
      (m.outcome === 'rejected' ? m.error_code !== null : m.error_code === null); break;
    default: valid = false;
  }
  if (!valid) throw new Error('invalid_message');
  return m as unknown as DesktopMessage;
}
/** Call only with parsed messages; level samples never change the session phase. */
export function acceptsLevel(snapshot: Snapshot, level: Level, lastSequence: number): boolean {
  return snapshot.state === 'capturing' && snapshot.epoch === level.epoch && snapshot.session_id === level.session_id &&
    snapshot.revision === level.revision && level.sequence > lastSequence;
}
