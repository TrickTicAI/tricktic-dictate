import type { InputLanguage, ProviderId, Settings } from '@metrix-writer/contracts';
import { TranscriptionSetup } from '../transcription-setup';
import { applyLanguagePreset, languageCatalog } from '../language';
import { preferencesBridge } from '../desktop-preferences/bridge';
import type { CloudProviderId } from '../transcription-setup/api';
import { Group, Row, Segmented } from './controls';

export interface ProviderStatus {
  id: ProviderId;
  configured: boolean;
  available: boolean;
  models?: string[];
}

/**
 * The speech model: what hears you, the language it is told, its weights, the
 * runtime, and the cloud providers that can do it instead.
 *
 * Text processing used to be stacked underneath all of this on one Models page.
 * Two unrelated model libraries, two runtimes and two sets of provider cards in
 * one scroller meant the second one was reachable only by scrolling past the
 * first — so they are two sections now, and this one is only about speech.
 */
export function TranscriptionSettings({ integrated, onSettingsSaved, settings, onSettings, providers, disabled, availabilityReason, capabilityIssue, onOpenKeys }: {
  integrated: boolean;
  onSettingsSaved?: (settings: Settings) => void;
  settings: Settings;
  onSettings: (settings: Settings) => void;
  providers: ProviderStatus[];
  disabled: boolean;
  availabilityReason?: string;
  capabilityIssue?: string;
  /** Opens Settings › API keys on one provider. */
  onOpenKeys?: (provider: CloudProviderId) => void;
}) {
  const selection = settings.transcription.selection;
  const provider = providers.find((item) => item.id === selection.provider);
  const preset = languageCatalog.presets.find((item) => item.id === settings.transcription.input_language);

  // The desktop shell manages weights, the runtime and cloud credentials
  // through the setup API. The self-hosted browser client chooses from the
  // catalog its server already exposes and does not manage weights.
  if (integrated) {
    return (
      <TranscriptionSetup
        onSettingsSaved={onSettingsSaved}
        capabilityIssue={capabilityIssue}
        onOpenKeys={onOpenKeys}
        importWeights={preferencesBridge.importModel}
      />
    );
  }

  return (
    <>
      <Group title="Transcription model" note={capabilityIssue} noteTone={capabilityIssue ? 'alert' : undefined}>
        {/* The desktop panel carries this inside its active-engine strip.
            The browser client has no such strip, and General now points
            here, so the control itself has to live beside the model it
            gates or the self-hosted UI cannot set a spoken language. */}
        <Row title="Speech language" detail={preset?.description} stacked>
          <Segmented
            label="Speech language"
            value={settings.transcription.input_language}
            disabled={disabled}
            options={languageCatalog.presets.map((item) => ({ id: item.id as InputLanguage, label: item.label }))}
            onChange={(id) => onSettings(applyLanguagePreset(settings, id))}
          />
        </Row>
        <Row
          title="Model"
          detail={
            selection.provider === 'local'
              ? 'Runs on your self-hosted server, not in this browser.'
              : 'Audio is sent through your server to this provider.'
          }
        >
          <select
            aria-label="Transcription model"
            disabled={disabled}
            value={`${selection.provider}/${selection.model}`}
            onChange={(event) => {
              const [nextProvider, model] = event.target.value.split('/');
              onSettings({
                ...settings,
                transcription: { ...settings.transcription, selection: { provider: nextProvider as ProviderId, model } },
              });
            }}
          >
            {languageCatalog.models.map((model) => (
              <option
                key={`${model.selection.provider}/${model.selection.model}`}
                value={`${model.selection.provider}/${model.selection.model}`}
              >
                {model.selection.provider} · {model.selection.model}
              </option>
            ))}
          </select>
        </Row>
        <Row
          title="Status"
          detail={availabilityReason
            ?? (provider?.id === 'local'
              ? 'Local runtime and loaded model passed readiness checks.'
              : 'This provider has returned a successful transcript in this server process.')}
        >
          <span className="badge">
            <span className={`dot ${provider?.available ? 'ok' : provider?.configured ? 'off' : 'bad'}`} aria-hidden />
            {provider?.available ? 'Ready' : provider?.configured ? 'Unverified' : 'Not configured'}
          </span>
        </Row>
        {provider?.id === 'local' && !!provider.models?.length && (
          <Row title="Configured local alias" detail={provider.models.join(', ')} />
        )}
      </Group>

      <Group title="Provider API keys">
        <Row
          title="Keys stay on your server"
          detail="This is the self-hosted browser client. Provider keys are read from the server environment; there is no route that would let this page write one."
        />
      </Group>
    </>
  );
}
