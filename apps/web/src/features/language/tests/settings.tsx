import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import type { Settings } from '@metrix-writer/contracts';
import { LanguageSettings, languageCapabilityIssue } from '../index';
function Harness() {
  const [value, onChange] = useState<Settings>({ ui_language: 'en', transcription: { selection: { provider: 'local', model: 'ivrit-turbo' }, input_language: 'he', output_language: { mode: 'preserve' }, cleanup: { mode: 'off' }, vocabulary: { hints: ['Metrix'], corrections: [], snippets: [] } } });
  const [available, setAvailable] = useState(true);
  return <><LanguageSettings value={value} onChange={onChange} availability={{ available }} /><button onClick={() => setAvailable(false)}>Mark unavailable</button><button onClick={() => { onChange({ ...value, transcription: { ...value.transcription, selection: { provider: 'openai', model: 'gpt-transcribe' } } }); setAvailable(false); }}>Select unverified cloud</button><button disabled={!!languageCapabilityIssue(value.transcription) || (value.transcription.selection.provider === 'local' && !available)}>Transcribe</button><pre data-testid="settings">{JSON.stringify(value)}</pre></>;
}
createRoot(document.getElementById('root')!).render(<Harness />);
