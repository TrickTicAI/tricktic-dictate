import { apiFetch } from '../auth/session';
import type { InputLanguage, ProviderId, Settings } from '@metrix-writer/contracts';
import { PRODUCT_NAME } from '../../product';

export type WeightsState =
  | { state: 'not_installed'; resumable_bytes: number | null }
  | { state: 'downloading'; downloaded_bytes: number; total_bytes: number }
  | { state: 'verifying'; bytes: number }
  | { state: 'installed'; bytes: number; sha256: string; verified_at_ms: number; origin: string }
  | { state: 'failed'; error: string; resumable_bytes: number | null };
export type WorkerState =
  | { state: 'idle' }
  | { state: 'missing_runtime'; detail: string }
  | { state: 'missing_weights'; model: string }
  | { state: 'starting'; model: string; since_ms: number }
  | { state: 'ready'; model: string; device: string; backend: string; started_at_ms: number; restarts: number }
  | { state: 'failed'; model: string; error: string; restarts: number }
  | { state: 'stopping'; model: string };
export interface ModelView {
  compatibility_issue?: string | null;
  smoke_tests?: { host: string; date: string; scope: string; tests: { sample: string; language: InputLanguage; backend: string; elapsed_ms: number; word_errors: number; reference_words: number }[] } | null;
  /** `attribution` is the credit the model's license requires wherever it is offered (CC BY 4.0). */
  metadata?: { family: string; precision: string; runtime: string; backends: string[]; revision: string; qualification: string; attribution?: string } | null;
  id: string; display_name: string; file_name: string; bytes: number; sha256: string; source_url: string; source_name: string;
  license: string; notes: string; input_languages: InputLanguage[]; weights: WeightsState; selected: boolean; loaded: boolean;
}
const LICENSE_NAMES: Record<string, string> = { mit: 'MIT License', 'apache-2.0': 'Apache License 2.0', 'cc-by-4.0': 'CC BY 4.0' };
/** A model's license as people read it; the full texts are in About → Open-source licenses. */
export const licenseName = (license: string) => LICENSE_NAMES[license.toLowerCase()] ?? license;
export type CredentialStatus = 'absent' | 'environment' | 'protected_store' | 'unreadable';
/** The provider's answer to an explicit key check. Weaker than `verified`, which needs a real transcript. */
export type CheckOutcome = 'accepted' | 'rejected' | 'unreachable';
export interface CloudCheck { outcome: CheckOutcome; checked_at_ms: number }
export interface CloudView { provider: ProviderId; models: string[]; credential: CredentialStatus; check: CloudCheck | null; verified: boolean; selected: boolean }
export type ExecutionBackend = 'automatic' | 'cpu' | 'gpu';
export interface RuntimeView {
  hardware?: { architecture: string; cpu_threads: number; total_ram_bytes: number | null; available_ram_bytes: number | null; parakeet_cpu_supported: boolean };
  execution?: { requested: ExecutionBackend; cpu_available: boolean; gpu_available: boolean; fallback_reason: string | null };
  worker: WorkerState; problem: string | null; packaged: boolean; worker_executable_present: boolean; vulkan_library_present: boolean;
  ffmpeg_present: boolean; external_server: boolean; models_dir: string | null;
}
export interface SetupView { mode: 'desktop' | 'self_hosted'; first_run: boolean; settings: Settings; runtime: RuntimeView; models: ModelView[]; cloud: CloudView[] }

export type ModelAction = 'download' | 'cancel' | 'verify' | 'delete';
export type RuntimeAction = 'retry' | 'stop';
export type CloudProviderId = 'openai' | 'gemini' | 'grok' | 'openrouter';
/** OpenRouter's row carries a key but no speech model; speech pages list only rows that transcribe. */
export const transcribes = (cloud: CloudView) => cloud.models.length > 0;
export interface SetupApi {
  load(): Promise<SetupView>;
  saveExecution(backend: ExecutionBackend): Promise<SetupView>;
  saveSettings(settings: Settings): Promise<Settings>;
  modelAction(model: string, action: ModelAction): Promise<SetupView>;
  runtimeAction(action: RuntimeAction): Promise<SetupView>;
  /** Asks the server to try the stored key against the provider. No key crosses this call. */
  checkCloud(provider: CloudProviderId): Promise<SetupView>;
}

/**
 * @param routeMessages Messages for codes this particular route gives a more
 * specific meaning than the shared mapping below. `provider_unavailable` is the
 * example: from `/setup` it means the backend is gone, and from a key check it
 * means there is no key to check.
 */
async function request<T>(path: string, init?: RequestInit, routeMessages?: Record<string, string>): Promise<T> {
  const response = await apiFetch(path, init);
  if (!response.ok) {
    // Never surface arbitrary upstream bodies; map the versioned error code only.
    let code = '';
    try { code = String(((await response.json()) as { error?: { code?: string } }).error?.code ?? ''); } catch { /* non-JSON */ }
    throw new Error(routeMessages?.[code] ??
      (code === 'unsupported_capability' ? 'This model does not support the selected speech language. Choose a compatible model or language.' :
      code === 'idempotency_conflict' ? 'Wait for inference, downloads or verification to finish. Cancel a download before deleting its files.' :
      code === 'invalid_request' ? 'The setup request was rejected. Refresh and try again.' :
      response.status === 503 ? `${PRODUCT_NAME}’s backend is unavailable. Saved audio and text remain on this device.` :
      'Transcription setup could not be reached. Your choices are still here; try again.'));
  }
  const payload = (await response.json()) as { api_version?: string; data?: T };
  if (payload.api_version !== '1' || payload.data === undefined) throw new Error('The server returned an invalid setup document.');
  return payload.data;
}
function validate(view: SetupView): SetupView {
  if (!view || !Array.isArray(view.models) || !Array.isArray(view.cloud) || !view.runtime?.worker?.state || !view.settings?.transcription) {
    throw new Error('The server returned an invalid setup document.');
  }
  return view;
}
export const setupApi: SetupApi = {
  saveExecution: async backend => validate(await request<SetupView>('/api/v1/setup/execution', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ api_version: '1', data: backend }) })),
  load: async () => validate(await request<SetupView>('/api/v1/setup')),
  // A conflict here is never about deleting files, which is what the shared
  // mapping says: the only way a save still loses the runtime lock is a model
  // being unloaded at that moment, and waiting a second is the whole remedy.
  saveSettings: (settings) => request<Settings>('/api/v1/settings', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ api_version: '1', data: settings }) }, {
    idempotency_conflict: `${PRODUCT_NAME} is still switching models. Try that again in a moment.`,
  }),
  modelAction: async (model, action) => {
    if (!/^[a-z0-9-]{1,64}$/.test(model)) throw new Error('Unknown model.');
    return validate(await request<SetupView>(`/api/v1/setup/models/${model}/${action}`, { method: 'POST' }));
  },
  runtimeAction: async (action) => validate(await request<SetupView>(`/api/v1/setup/runtime/${action}`, { method: 'POST' })),
  checkCloud: async (provider) => validate(await request<SetupView>(`/api/v1/setup/cloud/${provider}/test`, { method: 'POST' }, {
    provider_unavailable: 'No key is stored for this provider yet, so there is nothing to check.',
  })),
};

export function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(2)} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(1)} MB`;
  return `${Math.round(bytes / 1e3)} KB`;
}
const weightsErrors: Record<string, string> = {
  checksum_mismatch: 'The file did not match the published SHA-256 checksum and was removed. Download or import it again.',
  size_mismatch: 'The file size did not match the published weights. It was removed.',
  weights_file_missing: 'No weights file was found at the expected import location.',
  weights_unreadable: 'The weights file could not be read.',
  download_canceled: 'Download canceled. The partial file was kept and resumes from where it stopped.',
  download_interrupted: 'The download was interrupted. Resume to continue from the saved bytes.',
  download_stalled: 'The download stalled. Resume to continue from the saved bytes.',
  source_unreachable: 'The download source could not be reached. Check the connection and resume.',
  source_rejected: 'The download source rejected the request.',
  source_invalid: 'The download source address is not allowed.',
  storage_failed: `${PRODUCT_NAME} could not write to the models directory. Check free space and permissions.`,
  registry_unwritable: 'Verified weights could not be recorded. Check the state directory permissions and verify again.',
  download_incomplete: 'The download ended before all bytes arrived. Resume to continue.',
  partial_file_invalid: 'The partial file was larger than the published weights and was discarded.',
  unknown_model: 'Unknown model.',
};
export function describeWeightsError(error: string): string { return weightsErrors[error] ?? `Weights problem: ${error}.`; }
const runtimeProblems: Record<string, string> = {
  runtime_not_packaged: 'This build has no bundled inference runtime. Local transcription needs the packaged desktop app or WRITER_RUNTIME_DIR on the server.',
  worker_executable_missing: `The bundled whisper-server executable is missing. Reinstall ${PRODUCT_NAME}.`,
  metal_backend_missing: `The GPU (Metal) runtime is missing. Reinstall ${PRODUCT_NAME} or choose CPU.`,
  vulkan_backend_missing: 'The GPU (Vulkan) runtime is missing. Install it or choose CPU with a CPU runtime installed.',
  cpu_runtime_missing: 'The independent CPU runtime is missing. Install a build containing the CPU runtime.',
  compatible_runtime_missing: 'No compatible CPU or GPU runtime is installed.',
  ffmpeg_missing: `The bundled audio converter (ffmpeg) is missing. Reinstall ${PRODUCT_NAME}.`,
};
export function describeRuntimeProblem(detail: string): string { return runtimeProblems[detail] ?? `Runtime problem: ${detail}.`; }
const workerErrors: Record<string, string> = {
  gpu_inference_failed: 'GPU inference failed. The same model is prepared on CPU; retry the saved recording when ready.',
  gpu_backend_not_used: 'The model loaded without the requested GPU backend. Check the GPU driver or choose Automatic with a CPU runtime installed.',
  cpu_backend_not_used: 'The worker did not confirm CPU execution. Reinstall the CPU runtime and retry.',
  gpu_unavailable: 'No GPU device was found. Choose Automatic or CPU with a CPU runtime installed.',
  loaded_model_mismatch: 'The worker loaded different weights than the verified file. Retry after verifying the weights.',
  worker_exited: `The inference worker stopped unexpectedly. ${PRODUCT_NAME} restarts it automatically up to three times.`,
  worker_exited_during_startup: 'The inference worker exited while loading the model.',
  worker_start_timeout: 'The model did not finish loading in time.',
  worker_start_failed: 'The inference worker could not be started.',
  weights_missing: 'The selected weights are not installed.',
  weights_changed_since_verification: 'The weights file changed since it was verified. Verify it again.',
  loopback_port_unavailable: 'No loopback port was available for the worker.',
  state_directory_unwritable: 'The state directory is not writable.',
};
export function describeWorkerError(error: string): string { return workerErrors[error] ?? `Worker failure: ${error}.`; }
