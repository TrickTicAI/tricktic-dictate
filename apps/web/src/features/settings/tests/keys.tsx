import { createRoot } from 'react-dom/client';
import type { Settings } from '@metrix-writer/contracts';
import type { CloudProvider, CredentialOutcome } from '../../desktop-preferences/bridge';
import { Keys } from '../Keys';
import type { CloudProviderId } from '../../transcription-setup/api';
import '../../../styles.css';
import { applyTheme } from '../../shell/theme';
import { themeById } from '../../../themes';

declare global {
  interface Window { navigated: string[]; nativeCalls: string[]; credentialOutcome: CredentialOutcome; credentialFails: boolean }
}
window.navigated = [];
window.nativeCalls = [];
window.credentialOutcome = 'saved';
window.credentialFails = false;

// `?browser` renders the self-hosted client, which has no key dialog.
// `?focus=<provider>` is API keys opened from another page for one provider.
const params = new URLSearchParams(location.search);
const browser = params.has('browser');
applyTheme(themeById(params.get('theme')));
const credentials = {
  prompt: async (provider: CloudProvider) => {
    window.nativeCalls.push(`prompt:${provider}`);
    if (window.credentialFails) throw 'Native credential dialog failed.';
    return window.credentialOutcome;
  },
  remove: async (provider: CloudProvider) => { window.nativeCalls.push(`remove:${provider}`); },
};
// Cleanup runs on OpenRouter, which is the case a user could not set up.
const settings = {
  ui_language: 'en',
  transcription: {
    selection: { provider: 'gemini', model: 'gemini-3.8-flash' }, input_language: 'en', output_language: { mode: 'preserve' },
    vocabulary: { hints: [], corrections: [], snippets: [] },
    cleanup: { mode: 'enabled', selection: { provider: 'openrouter', model: 'anthropic/claude-haiku-4.5' } },
  },
} as unknown as Settings;

createRoot(document.getElementById('root')!).render(
  <div style={{ maxWidth: 760, padding: 24 }}>
    <Keys
      settings={settings}
      credentials={browser ? undefined : credentials}
      onOpenSection={(section) => window.navigated.push(section)}
      focus={params.get('focus') ? { provider: params.get('focus') as CloudProviderId } : null}
    />
  </div>,
);
