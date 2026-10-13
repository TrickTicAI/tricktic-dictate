import localModels from '../../../../api/src/desktop_settings/model-catalog.json' with { type: 'json' };
import type { ExecutionLocation, InputLanguage, ModelSelection, OutputLanguage, Settings, TextLanguage, TranscriptionOptions } from '@metrix-writer/contracts';
import source from '../../../../../tests/fixtures/language/catalog.json' with { type: 'json' };

export interface LanguagePreset {
  id: InputLanguage;
  label: string;
  input_language: InputLanguage;
  output_language: OutputLanguage;
  task: 'transcribe';
  language_hints: TextLanguage[];
  description: string;
}
export interface ModelCapabilities {
  selection: ModelSelection;
  input_languages: InputLanguage[];
  translation_targets: TextLanguage[];
  execution_location: ExecutionLocation;
  language_control: string;
}
export interface LanguageCatalog { presets: LanguagePreset[]; models: ModelCapabilities[] }
export interface ModelAvailability { available: boolean; reason?: string }
// The Rust route consumes this same checked-in catalog. Availability is supplied separately.
export const languageCatalog: LanguageCatalog = {
  presets: source.presets as LanguagePreset[],
  models: [
    ...(source.models as ModelCapabilities[]).filter(m => m.selection.provider !== 'local'),
    ...localModels.map(m => ({ selection: { provider: 'local' as const, model: m.id }, input_languages: m.input_languages as InputLanguage[], translation_targets: [], execution_location: 'self_hosted_server' as const, language_control: m.notes })),
  ],
};
export function modelCapabilities(selection: ModelSelection): ModelCapabilities | undefined {
  return languageCatalog.models.find(model => model.selection.provider === selection.provider && model.selection.model === selection.model);
}
export function applyLanguagePreset(value: Settings, id: InputLanguage): Settings {
  const preset = languageCatalog.presets.find(item => item.id === id)!;
  return { ...value, transcription: { ...value.transcription, input_language: preset.input_language, output_language: { ...preset.output_language } } };
}
export function languageCapabilityIssue(options: TranscriptionOptions): string | null {
  const model = modelCapabilities(options.selection);
  if (!model) return 'The selected model has no registered language capabilities.';
  if (!model.input_languages.includes(options.input_language)) return 'The selected model does not support this input language. ivrit-turbo supports Hebrew only.';
  if (options.output_language.mode === 'translate' && !model.translation_targets.includes(options.output_language.language)) return 'Translation is explicitly selected but is not supported by this adapter. Select preserved spoken language.';
  return null;
}
/** Display status only: configured cloud adapters may allow a first unverified trial. */
export function languageIssue(options: TranscriptionOptions, availability?: ModelAvailability): string | null {
  const capabilityIssue = languageCapabilityIssue(options);
  if (capabilityIssue) return capabilityIssue;
  if (!availability?.available) return availability?.reason || 'The selected model is unavailable or unverified. Check credentials or the configured GPU server and loaded model.';
  return null;
}
export { LanguageSettings } from './LanguageSettings';
export type { LanguageSettingsProps } from './LanguageSettings';
