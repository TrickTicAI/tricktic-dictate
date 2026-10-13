import type { InputLanguage, Settings } from '@metrix-writer/contracts';
import { Languages, Mic } from 'lucide-react';
import { applyLanguagePreset, languageCatalog } from '../language';
import { Segmented } from '../settings/controls';
import { Microphone } from '../settings/Microphone';
import { PRODUCT_NAME } from '../../product';

/**
 * Speech language and input device.
 *
 * Language comes first because it decides which models can be offered at all:
 * Hebrew-only weights never receive English or mixed speech, so the next step
 * has to know this answer before it recommends anything.
 */
export function Voice({ settings, onSettings, disabled }: {
  settings: Settings;
  onSettings: (settings: Settings) => void;
  disabled: boolean;
}) {
  const language = settings.transcription.input_language;
  const preset = languageCatalog.presets.find((item) => item.id === language);

  return (
    <div className="ob-step">
      <div className="ob-head">
        <span className="ob-glyph"><Languages size={20} aria-hidden /></span>
        <div>
          <h1>What will you dictate in?</h1>
          <p>
            This is the language you speak, not the language of the text you get back and not the
            language of this window. Those are separate settings, and translation only ever happens
            when you ask for it.
          </p>
        </div>
      </div>

      <div className="ob-card">
        <h3>Speech language</h3>
        <p>{preset?.description}</p>
        <div className="row">
          <Segmented
            label="Speech language"
            value={language}
            disabled={disabled}
            options={languageCatalog.presets.map((item) => ({ id: item.id as InputLanguage, label: item.label }))}
            onChange={(id) => onSettings(applyLanguagePreset(settings, id))}
          />
        </div>
        <p className="ob-source">Change it any time in Settings → Speech engine, or from the tray menu.</p>
      </div>

      <div className="ob-card">
        <h3><Mic size={13} aria-hidden /> Microphone</h3>
        <p>{PRODUCT_NAME} records with this device. System default follows the current system input.</p>
        <div className="settings-card" style={{ marginTop: 10 }}>
          <Microphone disabled={disabled} />
        </div>
      </div>
    </div>
  );
}
