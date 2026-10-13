import { useCallback, useEffect, useRef, useState } from 'react';
import { CircleAlert, CircleCheck, CircleDashed, LoaderCircle, TriangleAlert } from 'lucide-react';
import type { ProviderId } from '@metrix-writer/contracts';
import type { CloudProviderId, CloudView } from './api';
import { isMac } from '../settings/platform';

export const providerName: Record<ProviderId, string> = { openai: 'OpenAI', gemini: 'Gemini', grok: 'Grok', local: 'Local', openrouter: 'OpenRouter' };

export type KeyTone = 'ok' | 'warn' | 'bad' | 'off';
export interface KeyState {
  /** One word, for the pill: what the provider has actually said. */
  label: string;
  tone: KeyTone;
  /** The same answer in full, including when and what it does not prove. */
  text: string;
  checking: boolean;
}

/**
 * What the provider has said about the stored key.
 *
 * Five states, not two, and they are not interchangeable: a key that exists is
 * not a key that was accepted, and a key that was accepted is not a key that
 * has produced a transcript. The pill says which one in a word; the sentence
 * underneath keeps the evidence, because "Connected" on its own would be a
 * claim this page cannot make.
 */
export function describeKey(cloud: CloudView, checking = false): KeyState {
  const name = providerName[cloud.provider];
  if (cloud.credential === 'absent') return { label: 'No key', tone: 'bad', text: 'No key stored.', checking: false };
  if (cloud.credential === 'unreadable') {
    return { label: 'Error', tone: 'bad', text: 'A stored key exists but this backend cannot read it. Set it again.', checking: false };
  }
  const where = cloud.credential === 'environment' ? 'Key read from the server environment.'
    : `Key stored in ${isMac() ? 'your macOS login Keychain' : 'Windows protected storage for this user'}.`;
  if (checking) return { label: 'Checking…', tone: 'off', text: `${where} Asking ${name} whether it accepts the key…`, checking: true };
  if (cloud.verified) return { label: 'Verified', tone: 'ok', text: `${where} Working: a transcript has already succeeded with it.`, checking: false };
  if (cloud.check?.outcome === 'accepted') {
    return {
      label: 'Connected',
      tone: 'ok',
      text: `${where} ${name} accepted it at ${new Date(cloud.check.checked_at_ms).toLocaleTimeString()}. No transcript has run yet.`,
      checking: false,
    };
  }
  if (cloud.check?.outcome === 'rejected') return { label: 'Error', tone: 'bad', text: `${where} ${name} rejected it. Replace the key.`, checking: false };
  if (cloud.check?.outcome === 'unreachable') {
    return { label: 'Unreachable', tone: 'warn', text: `${where} ${name} could not be reached, so the key is still unproven.`, checking: false };
  }
  return { label: 'Not checked', tone: 'off', text: `${where} Not checked yet.`, checking: false };
}

/** The provider's answer, where the key is, in a word. */
export function KeyPill({ state }: { state: KeyState }) {
  const Icon = state.checking ? LoaderCircle
    : state.tone === 'ok' ? CircleCheck
    : state.tone === 'bad' ? CircleAlert
    : state.tone === 'warn' ? TriangleAlert
    : CircleDashed;
  return (
    <span className={`key-pill ${state.tone}`}>
      <Icon size={12} aria-hidden className={state.checking ? 'key-spin' : undefined} />
      {state.label}
    </span>
  );
}

/**
 * Checks a stored key against its provider as soon as the key is on screen,
 * once per provider.
 *
 * The check is remembered against the key's fingerprint by the backend and not
 * on disk, so after a restart every stored key reads "Not checked" until
 * something asks. Leaving that to a button meant the page's own answer to "did
 * my key work" was usually "I have not looked", which is the thing this whole
 * section exists to stop being true.
 */
export function useKeyChecks(cloud: CloudView[] | null, check: (provider: CloudProviderId) => Promise<unknown>) {
  const [checking, setChecking] = useState<string[]>([]);
  const attempted = useRef(new Set<string>());
  const runCheck = useCallback((provider: CloudProviderId) => {
    setChecking(list => (list.includes(provider) ? list : [...list, provider]));
    return Promise.resolve(check(provider)).finally(() => setChecking(list => list.filter(id => id !== provider)));
  }, [check]);

  useEffect(() => {
    if (!cloud) return;
    for (const item of cloud) {
      const present = item.credential === 'environment' || item.credential === 'protected_store';
      if (!present || item.check || item.verified || attempted.current.has(item.provider)) continue;
      attempted.current.add(item.provider);
      void runCheck(item.provider as CloudProviderId);
    }
  }, [cloud, runCheck]);

  return { checking, runCheck };
}
