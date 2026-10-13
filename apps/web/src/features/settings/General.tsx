import type { InputLanguage, Settings, TextLanguage } from '@metrix-writer/contracts';
import { applyLanguagePreset, languageCapabilityIssue, languageCatalog, modelCapabilities } from '../language';
import { useCleanupProviders } from '../cleanup/useCleanup';
import { providerName } from '../transcription-setup/CloudKey';
import { Group, Row, Segmented } from './controls';
import type { DesktopPreferences } from '@metrix-writer/contracts/desktop';
import type { CloudProviderId } from '../transcription-setup/api';

export function General({ value, onChange, disabled, onOpenSection, onOpenKeys }: {
  value: Settings;
  onChange: (settings: Settings) => void;
  disabled: boolean;
  /** Opens another settings section; the modal supplies it. */
  onOpenSection?: (section: 'engine' | 'keys') => void;
  /** Opens API keys on one provider, with a way back here. */
  onOpenKeys?: (provider?: CloudProviderId) => void;
}) {
  const options = value.transcription;
  const preset = languageCatalog.presets.find((item) => item.id === options.input_language);
  const model = modelCapabilities(options.selection);
  const issue = languageCapabilityIssue(options);
  const cleanupProviders = useCleanupProviders();
  const cleanupSelection = options.cleanup.mode === 'enabled' ? options.cleanup.selection : null;

  // What the chosen model does with these languages, and where it runs. It
  // belongs to the language group rather than floating between cards, because
  // it is the consequence of the three controls directly above it.
  const languageNote = issue
    ?? (model
      ? `${model.execution_location === 'cloud' ? 'Cloud inference.' : 'Inference on the self-hosted server.'} ${model.language_control}`
      : undefined);

  return (
    <>
      <Group title="Language" note={languageNote} noteTone={issue ? 'alert' : undefined}>
        {/* Speech language is not here. It decides which models can be selected
            at all, and whisper is told it outright, so it lives beside the model
            in Settings → Transcription. Output and interface language are
            genuinely separate preferences and stay. */}
        <Row
          title="Speech language"
          detail={`You dictate in ${preset?.label ?? options.input_language}. Chosen with the speech model, which has to understand it.`}
        >
          {onOpenSection && (
            <button type="button" className="button" onClick={() => onOpenSection('engine')}>Change in Speech engine</button>
          )}
        </Row>

        <Row
          title="Output"
          detail="Preserve keeps the spoken language. Translation is available only when the selected adapter supports it; changing the speech language restores Preserve."
        >
          <select
            aria-label="Output language"
            value={options.output_language.mode === 'preserve' ? 'preserve' : options.output_language.language}
            disabled={disabled}
            onChange={(event) => {
              const id = event.target.value;
              onChange({
                ...value,
                transcription: {
                  ...options,
                  output_language: id === 'preserve' ? { mode: 'preserve' } : { mode: 'translate', language: id as TextLanguage },
                },
              });
            }}
          >
            <option value="preserve">Preserve spoken language</option>
            {([{ id: 'en', label: 'English' }, { id: 'he', label: 'Hebrew' }] as const).map(target => {
              const available = model?.translation_targets.includes(target.id) ?? false;
              return <option key={target.id} value={target.id} disabled={!available}>
                Translate to {target.label}{available ? '' : ' (unavailable for this model)'}
              </option>;
            })}
          </select>
        </Row>

        <Row
          title="Interface language"
          detail="Sets the direction of the interface. Interface text is not translated yet, so the labels stay in English while the layout mirrors."
        >
          <Segmented
            label="Interface language"
            value={value.ui_language}
            disabled={disabled}
            options={[{ id: 'en' as TextLanguage, label: 'English' }, { id: 'he' as TextLanguage, label: 'עברית' }]}
            onChange={(id) => onChange({ ...value, ui_language: id })}
          />
        </Row>
      </Group>

      <Group title="Transcript cleanup" note={cleanupProviders.error} noteTone={cleanupProviders.error ? 'alert' : undefined}>
        <Row
          title="Cleanup pass"
          detail="Adjusts punctuation, capitalization and spacing while keeping every word and language. The raw transcript is always kept separately, and cleanup runs only when you ask for it on a transcript."
        >
          <select
            aria-label="Cleanup model"
            value={cleanupSelection ? `${cleanupSelection.provider}/${cleanupSelection.model}` : 'off'}
            disabled={disabled}
            onChange={(event) => {
              const choice = cleanupProviders.choices.find((item) => `${item.selection.provider}/${item.selection.model}` === event.target.value);
              onChange({
                ...value,
                transcription: {
                  ...options,
                  cleanup: choice ? { mode: 'enabled', selection: choice.selection } : { mode: 'off' },
                },
              });
            }}
          >
            <option value="off">Off</option>
            {cleanupSelection && !cleanupProviders.choices.some((item) => item.selection.provider === cleanupSelection.provider && item.selection.model === cleanupSelection.model) && (
              <option value={`${cleanupSelection.provider}/${cleanupSelection.model}`}>{cleanupSelection.model} · unavailable</option>
            )}
            {cleanupProviders.choices.map((choice) => (
              <option key={`${choice.selection.provider}/${choice.selection.model}`} value={`${choice.selection.provider}/${choice.selection.model}`}>
                {choice.selection.model} · {providerName[choice.selection.provider]}{choice.configured ? '' : ' · key needed'}
              </option>
            ))}
          </select>
        </Row>
        {/* The list above can offer a model whose provider has no key yet; the key is one click away, not a hunt. */}
        {(() => {
          const chosen = cleanupSelection && cleanupProviders.choices.find((item) => item.selection.provider === cleanupSelection.provider && item.selection.model === cleanupSelection.model);
          const missing = chosen && !chosen.configured;
          return <Row
            title="Provider key"
            detail={missing
              ? `${providerName[cleanupSelection!.provider]} has no key yet, so cleanup cannot run. Add it in API keys.`
              : 'Cleanup uses the same provider keys as transcription and text processing.'}
          >
            {onOpenKeys && <button type="button" className={`button ${missing ? 'primary' : ''}`} onClick={() => onOpenKeys(cleanupSelection?.provider === 'local' ? undefined : cleanupSelection?.provider as CloudProviderId | undefined)}>
              {missing ? `Add ${providerName[cleanupSelection!.provider]} key…` : 'Manage API keys'}
            </button>}
          </Row>;
        })()}
      </Group>
    </>
  );
}

/**
 * One Animations setting for the whole app: this window and the floating panel.
 * It is Writer's own, not the operating system's, so turning Windows' animation
 * effects off no longer freezes progress indicators half-way. Reduced keeps
 * anything that reports progress moving, in steps.
 */
export function Animations({ preferences, onPreferences, busy }: {
  preferences: DesktopPreferences | null;
  onPreferences?: (next: DesktopPreferences) => void;
  busy?: boolean;
}) {
  return (
    <Group title="Motion">
      <Row
        title="Animations"
        detail={preferences
          ? 'Reduced stops decoration and keeps progress indicators moving in small steps, in this window and in the floating panel. It does not follow the system animation setting.'
          : 'Follows your system’s reduced-motion setting.'}
      >
        {preferences && (
          <Segmented
            label="Animations"
            value={preferences.reduced_motion ? 'reduced' : 'full'}
            disabled={busy || !onPreferences}
            options={[{ id: 'full', label: 'Full' }, { id: 'reduced', label: 'Reduced' }]}
            onChange={(id) => onPreferences?.({ ...preferences, reduced_motion: id === 'reduced' })}
          />
        )}
      </Row>
    </Group>
  );
}
