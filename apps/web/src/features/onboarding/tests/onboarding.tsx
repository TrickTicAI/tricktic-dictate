import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import type { Settings } from '@metrix-writer/contracts';
import type { DesktopPreferences } from '@metrix-writer/contracts/desktop';
import type { SessionState } from '../../desktop-session/contracts/protocol';
import { Onboarding } from '../Onboarding';
import { useOnboardingGate, type HotkeyEdge, type OnboardingHost, type SystemSettings } from '../host';
import { applyTheme } from '../../shell/theme';
import { themeById } from '../../../themes';
import '../../../styles.css';

/**
 * Mounts the real guide, the real gate and the real setup HTTP client against
 * an in-page shell. Only the four things that genuinely need a native host are
 * replaced: completion storage, system settings, the key dialog and the
 * physical chord events. The API is exercised over routed HTTP by the spec.
 */
interface DictationState {
  state: SessionState;
  sessionId: string | null;
  transcript: string | null;
  problem: string | null;
  levels: number[];
}
declare global {
  interface Window {
    ob: {
      hotkey(chord: HotkeyEdge['chord'], pressed: boolean): void;
      dictate(next: Partial<DictationState>): void;
      completed: boolean | null;
      system: SystemSettings;
      starts: number;
      stops: number;
      credentialPrompts: string[];
      settings: Settings;
      preferences: DesktopPreferences;
    };
  }
}

const system: SystemSettings = { launch_at_login: false, sounds: false, duck_audio: false };
let fire: (edge: HotkeyEdge) => void = () => { /* nothing listening yet */ };
let completedVersion = 0;

window.ob = {
  hotkey: (chord, pressed) => fire({ chord, pressed }),
  dictate: () => { /* replaced on mount */ },
  completed: null,
  system: { ...system },
  starts: 0,
  stops: 0,
  credentialPrompts: [],
  settings: {} as Settings,
  preferences: {} as DesktopPreferences,
};

const host: OnboardingHost = {
  status: async () => ({ completed_version: completedVersion, current_version: 1 }),
  setCompleted: async (completed) => {
    completedVersion = completed ? 1 : 0;
    window.ob.completed = completed;
    return { completed_version: completedVersion, current_version: 1 };
  },
  system: async () => ({ ...system }),
  saveSystem: async (next) => { Object.assign(system, next); window.ob.system = { ...system }; },
  onHotkey: (listener) => { fire = listener; return () => { fire = () => { /* detached */ }; }; },
};

const initialSettings: Settings = {
  ui_language: 'en',
  transcription: {
    selection: { provider: 'openai', model: 'gpt-transcribe' },
    input_language: 'en',
    output_language: { mode: 'preserve' },
    vocabulary: { hints: [], corrections: [], snippets: [] },
    cleanup: { mode: 'off' },
  },
};
const initialPreferences: DesktopPreferences = {
  recording: 'Ctrl+Shift+F9', cancel: 'Ctrl+Shift+F10', toggle: false,
  visibility: 'recording', placement: 'bottom', position: null, scale: 1,
  reduced_motion: false, completion_hold_ms: 1200, auto_dismiss: true, panel_colors: null,
};

function Harness() {
  const gate = useOnboardingGate(true, host);
  const [settings, setSettings] = useState(initialSettings);
  const [preferences, setPreferences] = useState(initialPreferences);
  const [dictation, setDictation] = useState<DictationState>({
    state: 'idle', sessionId: null, transcript: null, problem: null, levels: [],
  });
  window.ob.dictate = (next) => setDictation((previous) => ({ ...previous, ...next }));
  window.ob.settings = settings;
  window.ob.preferences = preferences;

  if (!gate.open) {
    return <main style={{ padding: 24 }}>
      <h1>Workspace</h1>
      <button type="button" onClick={() => void gate.replay()}>Run setup guide</button>
    </main>;
  }
  return (
    <div className="shell">
      <div />
      <Onboarding
        settings={settings}
        onSettings={setSettings}
        preferences={preferences}
        onPreferences={setPreferences}
        preferencesBusy={false}
        pollMs={150}
        host={host}
        credentials={{ prompt: async (provider) => { window.ob.credentialPrompts.push(provider); } }}
        dictation={{
          ...dictation,
          start: () => { window.ob.starts++; },
          stop: () => { window.ob.stops++; },
        }}
        onFinish={gate.finish}
      />
    </div>
  );
}

applyTheme(themeById('paper'));
createRoot(document.getElementById('root')!).render(<Harness />);
