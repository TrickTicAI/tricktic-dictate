import type { InputLanguage, Settings, TextLanguage } from '@metrix-writer/contracts';
import { applyLanguagePreset, languageCatalog, languageCapabilityIssue, languageIssue, modelCapabilities, type ModelAvailability } from './index';

export interface LanguageSettingsProps {
  value: Settings;
  onChange: (settings: Settings) => void;
  availability?: ModelAvailability;
  disabled?: boolean;
}

/** Controlled, network-free component. The host owns persistence and model selection. */
export function LanguageSettings({ value, onChange, availability, disabled = false }: LanguageSettingsProps) {
  const options = value.transcription;
  const preset = languageCatalog.presets.find(item => item.id === options.input_language)!;
  const model = modelCapabilities(options.selection);
  const issue = languageCapabilityIssue(options);
  const status = languageIssue(options, availability);
  return <fieldset disabled={disabled}>
    <legend>Language settings</legend>
    <label>Speech language
      <select value={options.input_language} onChange={event => onChange(applyLanguagePreset(value, event.target.value as InputLanguage))}>
        {languageCatalog.presets.map(item => <option key={item.id} value={item.id}>{item.label}</option>)}
      </select>
    </label>
    <p>{preset.description}</p>
    <label>Output language
      <select value={options.output_language.mode === 'preserve' ? 'preserve' : options.output_language.language}
        onChange={event => onChange({ ...value, transcription: { ...options, output_language: event.target.value === 'preserve' ? { mode: 'preserve' } : { mode: 'translate', language: event.target.value as TextLanguage } } })}>
        <option value="preserve">Preserve spoken language</option>
        <option value="en">Translate to English (unsupported)</option>
        <option value="he">Translate to Hebrew (unsupported)</option>
      </select>
    </label>
    <p>Translation requires an explicit selection and a supporting adapter. Changing the speech preset restores preservation.</p>
    <label>Interface language
      <select value={value.ui_language} onChange={event => onChange({ ...value, ui_language: event.target.value as TextLanguage })}>
        <option value="en">English</option><option value="he">עברית</option>
      </select>
    </label>
    <p>Interface language is a separate preference; it does not change transcription. Interface translation is not implemented here.</p>
    {model && <p>{model.execution_location === 'cloud' ? 'Cloud inference.' : 'Inference on the self-hosted GPU server.'} {model.language_control}</p>}
    {issue ? <p role="alert">{issue}</p> : <p role="status">{status || "Language settings are compatible. Speech accuracy still requires evaluation."}</p>}
  </fieldset>;
}
