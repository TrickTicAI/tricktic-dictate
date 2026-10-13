import { useCallback, useEffect, useRef, useState } from 'react';
import type { DesktopPreferences, OverlayThemeMessage } from '@metrix-writer/contracts/desktop';
import { invoke, isTauri } from '../desktop-session/runtime/native';

/*
 * The single typed settings bridge. Shortcut chords, overlay preferences and
 * the resolved theme colours all travel on it. In the browser build every call
 * rejects, and the settings sections that depend on it say so rather than
 * pretending to store anything.
 */

export const supportsDesktopPreferences = () => isTauri();

/**
 * What the native key dialog did. The key itself never crosses the bridge, so
 * this is the only thing the page can know — and it has to know it, because a
 * dismissed dialog and a stored key used to be the same silent success.
 */
export type CredentialOutcome = 'saved' | 'canceled';
export type CloudProvider = 'openai' | 'gemini' | 'grok' | 'openrouter';
export interface CredentialActions {
  prompt(provider: CloudProvider): Promise<CredentialOutcome>;
  remove(provider: CloudProvider): Promise<void>;
}

export const preferencesBridge = {
  get: () => invoke<DesktopPreferences>('preferences_get'),
  save: (preferences: DesktopPreferences) => invoke<void>('preferences_save', { preferences }),
  theme: (theme: OverlayThemeMessage) => invoke<void>('preferences_theme', { theme }),
  importModel: (model: string) => invoke<void>('import_model', { model }),
  promptCredential: (provider: CloudProvider) => invoke<CredentialOutcome>('credential_prompt', { provider }),
  removeCredential: (provider: CloudProvider) => invoke<void>('credential_remove', { provider }),
  openDataFolder: () => invoke<void>('open_data_folder'),
};

/**
 * The native key dialog and protected store, as every page that offers a key
 * consumes them.
 *
 * `credentials.rs` exposes read and status only, and there is deliberately no
 * HTTP write route: adding one would put a provider key in the WebView, the
 * React tree and a request body all at once. One object, because one key per
 * provider serves both Transcription and Text processing — the pages that set
 * it are two, the credential they write is one.
 */
export const nativeCredentials: CredentialActions = {
  prompt: preferencesBridge.promptCredential,
  remove: preferencesBridge.removeCredential,
};

function message(cause: unknown, fallback: string) {
  return typeof cause === 'string' ? cause : cause instanceof Error ? cause.message : fallback;
}

/**
 * Loads the shell's preferences and saves whole documents back. The Rust side
 * is `deny_unknown_fields`, so edits always start from a loaded document rather
 * than a locally assembled one.
 */
export function useDesktopPreferences(enabled: boolean) {
  const [value, setValue] = useState<DesktopPreferences | null>(null);
  const [error, setError] = useState('');
  const [status, setStatus] = useState('');
  const [busy, setBusy] = useState(false);
  const active = useRef(true);
  useEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);

  const reload = useCallback(async () => {
    if (!enabled) return;
    try {
      const loaded = await preferencesBridge.get();
      if (active.current) { setValue(loaded); setError(''); }
    } catch (cause) {
      if (active.current) setError(message(cause, 'Desktop settings could not be read. Recording and saved audio are unaffected.'));
    }
  }, [enabled]);

  useEffect(() => { void reload(); }, [reload]);

  const save = useCallback(async (next: DesktopPreferences) => {
    setBusy(true);
    setStatus('Saving…');
    setError('');
    try {
      await preferencesBridge.save(next);
      if (!active.current) return true;
      setValue(next);
      setStatus('Saved');
      return true;
    } catch (cause) {
      if (active.current) {
        // The shell keeps the previous settings on any rejection, so the
        // in-memory document is reloaded rather than left showing a value the
        // shell did not accept.
        setError(message(cause, 'Settings were not saved. The previous settings are still in effect.'));
        setStatus('');
        void reload();
      }
      return false;
    } finally {
      if (active.current) setBusy(false);
    }
  }, [reload]);

  return { value, error, status, busy, save, reload, setStatus };
}
