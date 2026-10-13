import { createRoot } from 'react-dom/client';
import { TranscriptionSetup } from '../TranscriptionSetup';

/**
 * Mounts the real panel against routed HTTP. What the settings dialog owns is
 * replaced — the way to API keys and the native weights file picker — and each
 * records what it was asked for so the spec can assert the panel calls them.
 */
declare global {
  interface Window {
    savedSettings?: unknown;
    nativeCalls: string[];
  }
}
window.nativeCalls = [];

createRoot(document.getElementById('root')!).render(
  <TranscriptionSetup
    pollMs={150}
    onOpenKeys={provider => { window.nativeCalls.push(`keys:${provider}`); }}
    importWeights={async (model: string) => { window.nativeCalls.push(`import:${model}`); }}
    onSettingsSaved={settings => { window.savedSettings = settings; }}
  />,
);
