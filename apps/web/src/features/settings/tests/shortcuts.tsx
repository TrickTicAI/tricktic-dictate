import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import type { DesktopPreferences } from '@metrix-writer/contracts/desktop';
import { Shortcuts } from '../Shortcuts';
import '../../../styles.css';
import { applyTheme } from '../../shell/theme';
import { themeById } from '../../../themes';

const initial: DesktopPreferences = {
  recording: 'Ctrl+Shift+F9', cancel: 'Ctrl+Shift+F10', toggle: false,
  visibility: 'recording', placement: 'bottom', position: null, scale: 1.2,
  reduced_motion: false, completion_hold_ms: 1800, auto_dismiss: true, panel_colors: null,
};

/** Mounts the real section with an in-memory shell, so no Tauri host is needed. */
function Harness() {
  const [preferences, setPreferences] = useState(initial);
  const [saves, setSaves] = useState<DesktopPreferences[]>([]);
  (window as unknown as { saves: DesktopPreferences[] }).saves = saves;
  return <Shortcuts
    integrated
    preferences={preferences}
    busy={false}
    error=""
    status=""
    onSave={(next) => { setSaves((all) => [...all, next]); setPreferences(next); }}
  />;
}

applyTheme(themeById('paper'));
createRoot(document.getElementById('root')!).render(<Harness />);
