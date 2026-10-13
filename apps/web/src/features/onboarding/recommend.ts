import type { InputLanguage } from '@metrix-writer/contracts';
import type { ModelView, RuntimeView } from '../transcription-setup/api';

/**
 * Picks one local model to offer first during setup.
 *
 * The choice is made only from the catalog the backend actually exposes, and
 * only on facts the backend reports: which input languages a model declares,
 * whether this build can run it at all, its download size, and how much memory
 * and which runtimes this machine has right now. It is a size-and-fit
 * suggestion, never an accuracy claim; the setup step says so and the full
 * library stays one click away in Settings → Transcription.
 */
export interface Recommendation {
  best: ModelView;
  /** A meaningfully smaller download, for a slow link or a tight disk. */
  lighter?: ModelView;
  /** The preference this machine's memory ruled out, offered anyway. */
  stronger?: ModelView;
  /** The machine facts the size budget came from, shown verbatim. */
  reason: string;
}

/**
 * Preferred starting points per speech language, most preferred first. Each is
 * taken only if the backend still publishes it and it fits the memory budget,
 * so removing one from the catalog degrades to the next rather than breaking.
 */
const PREFERRED: Record<InputLanguage, string[]> = {
  he: ['ivrit-turbo', 'whisper-large-v3-turbo-q5-0', 'whisper-small-q5-1', 'whisper-base-q5-1'],
  en: ['whisper-large-v3-turbo-q5-0', 'whisper-medium-en-q5-0', 'whisper-small-en-q5-1', 'whisper-base-en-q5-1'],
  mixed: ['whisper-large-v3-turbo-q5-0', 'whisper-small-q5-1', 'whisper-base-q5-1'],
};

const GIB = 2 ** 30;

/**
 * Weights have to sit in memory alongside the runtime's own working set, so a
 * CPU budget is a fraction of what is free at this moment, never all of it. A
 * present Vulkan runtime lifts the ceiling because the weights land on the GPU,
 * but a runtime file is not a working device: the guide still has to prove it.
 */
function budgetBytes(runtime: RuntimeView): { bytes: number; reason: string } {
  const available = runtime.hardware?.available_ram_bytes ?? null;
  const free = available === null ? null : `${(available / GIB).toFixed(1)} GiB RAM free right now`;
  if (runtime.execution?.gpu_available) {
    return { bytes: 2 * GIB, reason: ['GPU (Vulkan) runtime installed', free].filter(Boolean).join(' · ') };
  }
  if (available !== null) {
    return { bytes: Math.max(available * 0.4, 1e8), reason: `CPU runtime · ${free}` };
  }
  return { bytes: 6e8, reason: 'Memory not reported, so a modest download is offered first' };
}

export function recommendModel(models: ModelView[], runtime: RuntimeView, language: InputLanguage): Recommendation | null {
  const usable = models
    .filter((model) => !model.compatibility_issue && model.input_languages.includes(language))
    .sort((a, b) => a.bytes - b.bytes);
  if (!usable.length) return null;

  const preferred = PREFERRED[language]
    .map((id) => usable.find((model) => model.id === id))
    .filter((model): model is ModelView => !!model);

  /**
   * `lighter` is a real step down — at most half the download — rather than
   * whatever happens to be one size smaller, which for a 1.6 GB pick can be
   * 1.5 GB and no help at all. `stronger` is only ever the preference this
   * machine's memory ruled out, so it can never suggest an older architecture
   * merely because its file is bigger.
   */
  const bracket = (best: ModelView, reason: string): Recommendation => ({
    best,
    lighter: [...usable].reverse().find((model) => model.bytes <= best.bytes / 2)
      ?? [...usable].reverse().find((model) => model.bytes < best.bytes),
    stronger: preferred.find((model) => model.bytes > best.bytes),
    reason,
  });

  // Anything already on disk beats a download, whatever the preference order.
  const installed = usable.filter((model) => model.weights.state === 'installed');
  if (installed.length) {
    const best = installed.find((model) => model.selected) ?? installed[installed.length - 1];
    return bracket(best, 'Already downloaded and verified on this computer');
  }

  const { bytes, reason } = budgetBytes(runtime);
  return bracket(preferred.find((model) => model.bytes <= bytes) ?? usable[0], reason);
}
