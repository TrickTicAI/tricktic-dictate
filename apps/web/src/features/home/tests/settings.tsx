import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { DataPrivacy } from '../../settings/DataPrivacy';
import { System } from '../../settings/System';
import type { DesktopPreferences } from '@metrix-writer/contracts/desktop';
import { applyTheme, overlayTheme } from '../../shell/theme';
import { themeById } from '../../../themes';
import '../../../styles.css';
// `?theme=` picks the app theme; Paper, the default, is what the fixtures assert.
const theme = themeById(new URLSearchParams(location.search).get('theme'));
applyTheme(theme);
const initial: DesktopPreferences = { recording: 'Ctrl+Shift+D', cancel: 'Ctrl+Shift+X', toggle: true,
  visibility: 'recording', placement: 'bottom', position: null, scale: 1, reduced_motion: false,
  completion_hold_ms: 1200, auto_dismiss: true, panel_colors: null };
declare global { interface Window { preferenceSaves: DesktopPreferences[] } }
window.preferenceSaves = [];
/** Every save is recorded and becomes the loaded document, as the shell bridge's does. */
/* System now appears in three sections — the floating panel, Microphone &
   sound, and General's app behaviour — so the harness shows all three parts. */
function SystemHarness() {
  const [preferences, setPreferences] = useState(initial);
  return <>{(['panel', 'sound', 'app'] as const).map((part) => <System
    key={part}
    part={part}
    integrated
    preferences={preferences}
    panelTheme={overlayTheme(theme)}
    onSave={(next) => { window.preferenceSaves.push(next); setPreferences(next); }}
    busy={false}
    error=""
    status=""
    onQuit={() => {}}
  />)}</>;
}
createRoot(document.getElementById('root')!).render(new URLSearchParams(location.search).get('section') === 'system'
  ? <SystemHarness />
  : <DataPrivacy integrated />);
