import type { ModelSelection } from '@metrix-writer/contracts';
import { isTauri, invoke } from '../desktop-session/runtime/native';
import { request } from '../transcripts/api';
export type ModeKind = 'regular' | 'super' | 'command';
export interface ContextPolicy { application: boolean; selected_text: boolean; clipboard: boolean }
export interface ContextSnapshot { application: string; selected_text: string; clipboard: string }
export interface Formatting { rewrite: 'light' | 'polished' | 'concise'; layout: 'automatic' | 'single_line'; lists: boolean; remove_fillers: boolean; self_corrections: boolean; spoken_punctuation: boolean }
export const defaultFormatting: Formatting = { rewrite: 'polished', layout: 'automatic', lists: true, remove_fillers: true, self_corrections: true, spoken_punctuation: true };
export interface Diagnostics { local_capacity_exhausted?: boolean; prepared_at_release?: boolean; processing_ms: number; first_response_ms?: number | null; response_complete_ms?: number | null; input_tokens?: number | null; output_tokens?: number | null; thought_tokens?: number | null; reused_draft: boolean; served_by?: string | null; fallback_used?: boolean; fallback_attempted?: boolean; original_used?: boolean; original_reason?: string | null; selected_model?: ModelSelection | null; model_attempts?: { local_capacity_exhausted?: boolean; selection: ModelSelection; role: string; state: string; elapsed_ms: number; error: string | null; served_by: string | null }[]; delivery?: {copied:boolean; pasted:boolean; at_ms:number; stop_to_delivery_ms:number|null; app?: string | null} | null }
export interface ModePreview { text: string; input_text: string; diagnostics: Diagnostics }
/** A saved wording of a mode's instructions, with the writing settings it was written for. */
export interface PromptVersion { id: string; name: string; instructions: string; formatting: Formatting; fast_processing: boolean; created_at_ms: number }
export interface ModeProfile { prompt_versions?: PromptVersion[]; formatting?: Formatting; process_while_speaking?: boolean; fast_processing?: boolean; shortcut?: string | null; id: string; revision: number; name: string; kind: ModeKind; instructions: string; text_model: ModelSelection | null; context: ContextPolicy; fallback_model?: ModelSelection | null; fallback_after_ms?: number | null; polish_timeout_ms?: number | null }
/** What a mode waits before its fallback model joins in, when it sets no wait of its own. */
/** The shortest wait before the backup joins; the server waits for twice the chosen model's usual answer time when that is longer. */
export const DEFAULT_FALLBACK_MS = 1500;
export const DEFAULT_POLISH_TIMEOUT_MS = 3000;
export const polishTimeout = (p: ModeProfile) => Math.max(p.polish_timeout_ms ?? DEFAULT_POLISH_TIMEOUT_MS, p.fallback_model ? (p.fallback_after_ms ?? DEFAULT_FALLBACK_MS) + 2000 : 0);
export const FALLBACK_RANGE = { min: 250, max: 15000 };
export interface ModeSettings { revision: number; active: string; profiles: ModeProfile[] }
export interface ProcessRequest { operation_id: string; profile_id: string; profile_revision: number; context: ContextSnapshot }
export interface ProcessingAttempt {
  id: string; job_id: string; created_at_ms: number; profile: ModeProfile; request: ProcessRequest;
  diagnostics?: Diagnostics; raw_text: string; input_text: string; prompt_version: number;
  state: { state: 'pending' } | { state: 'succeeded'; text: string } | { state: 'failed'; error: { message: string; retryable: boolean } };
}
const json = (method: string, data: unknown): RequestInit => ({ method, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ api_version: '1', data }) });
export const modeApi = {
  bind: (recording: string, profile: ModeProfile) => request(`/api/v1/recordings/${encodeURIComponent(recording)}/mode`, json('POST', { profile })),
  defaults: () => request<ModeSettings>('/api/v1/modes/defaults'),
  preview: (profile: ModeProfile, text: string, context: ContextSnapshot, job_id?: string) => request<ModePreview>('/api/v1/modes/preview', json('POST', { profile, text, context, job_id })),
  settings: () => request<ModeSettings>('/api/v1/modes'),
  save: (settings: ModeSettings) => isTauri() ? invoke<ModeSettings>('modes_save', { settings }) : request<ModeSettings>('/api/v1/modes', json('PUT', settings)),
  attempts: (job: string) => request<ProcessingAttempt[]>(`/api/v1/jobs/${encodeURIComponent(job)}/processing`),
  process: (job: string, data: ProcessRequest) => request<ProcessingAttempt>(`/api/v1/jobs/${encodeURIComponent(job)}/processing`, json('POST', data)),
};
export const emptyContext = (): ContextSnapshot => ({ application: '', selected_text: '', clipboard: '' });
/** What a speed sends as reasoning. `provider_default` sends no reasoning setting at all. */
export type Reasoning = 'provider_default' | 'off' | 'minimal' | 'low' | 'medium' | 'high';
export const reasoningLabel: Record<Reasoning, string> = { provider_default: 'Provider default', off: 'Off', minimal: 'Minimal', low: 'Low', medium: 'Medium', high: 'High' };
/** The same choice as a sentence fragment: "Fast sends …". */
export const reasoningSent = (r: Reasoning) => r === 'provider_default' ? 'no reasoning setting' : r === 'off' ? 'reasoning off' : `${r} reasoning`;
/** A cloud text model added by ID, with the reasoning each mode speed sends. */
export interface CustomModel { provider: ModelSelection['provider']; model: string; fast: Reasoning; balanced: Reasoning; only_hosts: string[]; ignore_hosts: string[] }
/** One host OpenRouter can route a model to. `tag` is what pinning names. */
export interface ModelHost { tag: string; name: string; uptime: number | null; quantization: string | null }
export const customModelApi = {
  list: () => request<CustomModel[]>('/api/v1/modes/custom-models'),
  save: (models: CustomModel[]) => request<CustomModel[]>('/api/v1/modes/custom-models', json('PUT', models)),
  hosts: (model: string) => request<ModelHost[]>('/api/v1/modes/custom-models/hosts', json('POST', { model })),
};
