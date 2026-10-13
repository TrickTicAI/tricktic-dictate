import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import type { Settings } from '@metrix-writer/contracts';
import type { DesktopPreferences } from '@metrix-writer/contracts/desktop';
import { SettingsModal } from '../SettingsModal';
import '../../../styles.css';
import { applyTheme } from '../../shell/theme';
import { THEMES, themeById } from '../../../themes';

const settings: Settings = {
  ui_language: 'en',
  transcription: {
    selection: { provider: 'openai', model: 'gpt-transcribe' },
    input_language: 'en', output_language: { mode: 'preserve' },
    vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' },
  },
};

const preferences: DesktopPreferences = {
  recording: 'Ctrl+Shift+F9', cancel: 'Ctrl+Shift+F10', toggle: false,
  visibility: 'recording', placement: 'bottom', position: null, scale: 1.2,
  reduced_motion: false, completion_hold_ms: 1800, auto_dismiss: true, panel_colors: null,
};

/**
 * The modal under a parent that re-renders on a timer, which is what the
 * desktop shell does: the update poll refreshes once a second, and each render
 * hands the modal a brand new `onClose` arrow. Ticking faster here keeps the
 * test quick without changing what is being exercised.
 */
function Harness() {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const timer = window.setInterval(() => setTick((n) => n + 1), 50);
    return () => window.clearInterval(timer);
  }, []);
  (window as unknown as { ticks: number }).ticks = tick;
  return <SettingsModal
    onClose={() => { (window as unknown as { closeCalls: number }).closeCalls += 1; }}
    version="0.0.0-test"
    integrated
    settings={settings}
    onSettings={() => {}}
    settingsDisabled={false}
    providers={[]}
    theme={themeById('paper')}
    themes={THEMES}
    onTheme={() => {}}
    preferences={preferences}
    onPreferences={() => {}}
    preferencesBusy={false}
    preferencesError=""
    preferencesStatus=""
    onQuit={() => {}}
  />;
}

(window as unknown as { closeCalls: number }).closeCalls = 0;
applyTheme(themeById('paper'));
createRoot(document.getElementById('root')!).render(<Harness />);
