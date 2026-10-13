import { useCallback, useEffect, useRef, useState } from 'react';
import { ExternalLink } from 'lucide-react';
import type { Settings } from '@metrix-writer/contracts';
import type { CredentialActions } from '../desktop-preferences/bridge';
import { describeKey, KeyPill, providerName, useKeyChecks } from '../transcription-setup/CloudKey';
import { setupApi, transcribes, type CloudProviderId, type CloudView } from '../transcription-setup/api';
import { Group } from './controls';
import { ProviderLogo } from '../shell/ProviderLogo';
import { invoke } from '../desktop-session/runtime/native';
import { modeApi, type ModeSettings } from '../modes/api';
import '../transcription-setup/setup.css';

/**
 * Every provider key in one place (MW-SETTINGS-1).
 *
 * Keys were only reachable from the page that needed them: speech keys under
 * Transcription and the OpenRouter key behind Text processing's Cloud tab, while
 * searching for "API key" landed on a sentence in Data & Privacy. This page
 * lists every provider with what its key is used for and the same controls
 * those pages have. It writes the same credential through the same native
 * dialog, so the key still never passes through this window.
 *
 * It is also the only settings page that changes a key (MW-SETTINGS-9): the
 * other pages show each key's state and link here for that provider, and the
 * dialog's header offers the way back to them.
 */
export function Keys({ settings, credentials, onOpenSection, focus }: {
  settings: Settings;
  /** The native key dialog; absent in the self-hosted browser client, which has no key write route. */
  credentials?: CredentialActions;
  onOpenSection?: (section: 'engine' | 'text' | 'general') => void;
  /** The provider another page sent you here for; scrolled to, marked and focused once. */
  focus?: { provider: CloudProviderId } | null;
}) {
  const [cloud, setCloud] = useState<CloudView[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState('');
  const [error, setError] = useState('');
  const [confirming, setConfirming] = useState<CloudProviderId | null>(null);
  const [modes, setModes] = useState<ModeSettings | null>(null);
  useEffect(() => { void modeApi.settings().then(setModes).catch(() => { /* "Used by" then names transcription and cleanup only. */ }); }, []);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  const load = useCallback(async () => {
    try { const next = await setupApi.load(); if (mounted.current) { setCloud(next.cloud); setError(''); } }
    catch (e) { if (mounted.current) setError((e as Error).message); }
  }, []);
  useEffect(() => {
    if (!credentials) return;
    void load();
    const timer = setInterval(() => void load(), 5000);
    return () => clearInterval(timer);
  }, [credentials, load]);
  const check = useCallback(async (provider: CloudProviderId) => {
    try { const next = await setupApi.checkCloud(provider); if (mounted.current) setCloud(next.cloud); }
    catch { /* The pill keeps saying unchecked. */ }
  }, []);
  const { checking, runCheck } = useKeyChecks(cloud, check);

  const list = useRef<HTMLUListElement>(null);
  const shown = useRef<object | null>(null);
  const loaded = cloud !== null;
  useEffect(() => {
    if (!focus || !loaded || shown.current === focus) return;
    shown.current = focus;
    const row = list.current?.querySelector<HTMLElement>(`[data-provider="${focus.provider}"]`);
    row?.scrollIntoView({ block: 'nearest' });
    row?.querySelector<HTMLButtonElement>('.setup-actions button:not(:disabled)')?.focus({ preventScroll: true });
  }, [focus, loaded]);

  if (!credentials) {
    return <Group title="Provider keys" note="This is the self-hosted browser client: provider keys are read from the server environment, and no page here can write one.">
      <div className="setting-row"><div className="setting-text"><strong>Keys stay on your server</strong><p>Set them in the server's environment, then restart it.</p></div></div>
    </Group>;
  }

  const cleanup = settings.transcription.cleanup.mode === 'enabled' ? settings.transcription.cleanup.selection.provider : null;
  /** What would stop working without this key: the speech engine, cleanup and every mode whose model it serves. */
  function uses(view: CloudView): string[] {
    const byModes = (modes?.profiles ?? []).filter((profile) =>
      profile.text_model?.provider === view.provider || profile.fallback_model?.provider === view.provider).map((profile) => profile.name);
    return [view.selected ? 'the speech engine' : '', cleanup === view.provider ? 'transcript cleanup' : '', ...byModes].filter(Boolean);
  }
  async function getKey(provider: CloudProviderId) {
    try { await invoke<void>('legal_open', { kind: `key-${provider}` }); }
    catch (e) { setError(typeof e === 'string' ? e : `The ${providerName[provider]} key page could not be opened.`); }
  }

  async function setKey(provider: CloudProviderId) {
    setBusy(true); setNote(''); setError('');
    try {
      const outcome = await credentials!.prompt(provider);
      if (!mounted.current) return;
      await load();
      if (outcome === 'saved' && mounted.current) {
        setNote(`${providerName[provider]} key stored. Checking it with ${providerName[provider]}…`);
        await runCheck(provider);
        if (mounted.current) setNote(`${providerName[provider]} key stored.`);
      }
    } catch (e) { if (mounted.current) setError(typeof e === 'string' ? e : (e as Error).message); }
    finally { if (mounted.current) setBusy(false); }
  }
  async function removeKey(provider: CloudProviderId) {
    setBusy(true); setNote(''); setError(''); setConfirming(null);
    try {
      await credentials!.remove(provider);
      if (!mounted.current) return;
      await load();
      if (mounted.current) setNote(`${providerName[provider]} key removed.`);
    } catch (e) { if (mounted.current) setError(typeof e === 'string' ? e : (e as Error).message); }
    finally { if (mounted.current) setBusy(false); }
  }

  return <section className="setup-panel" aria-label="API keys">
    <p className="group-note">Your own key for each cloud provider. One key serves transcription, text processing and cleanup. It is typed into a native dialog and kept in your system's protected storage; it never passes through this window.</p>
    <section className="settings-group">
      <h3>Providers</h3>
      <ul className="setup-cloud" aria-label="Provider keys" ref={list}>
        {(cloud ?? []).filter((view) => view.provider !== 'local').map((view) => {
          const provider = view.provider as CloudProviderId;
          const name = providerName[provider];
          const state = describeKey(view, checking.includes(provider));
          const present = view.credential === 'environment' || view.credential === 'protected_store';
          const used = uses(view);
          return <li key={provider} className={`setup-cloud-row ${focus?.provider === provider ? 'selected' : ''}`} aria-label={`${name} key`} data-provider={provider}>
            <div className="setup-model-head">
              <span className="logo-tile" aria-hidden><ProviderLogo id={provider} size={22} /></span>
              <strong>{name}</strong>
              <KeyPill state={state} />
            </div>
            <p className="setup-model-facts">
              {transcribes(view) ? 'Speech and text models' : 'Text models'}
              {used.length > 0 ? <> · Used by <b>{used.join(', ')}</b></> : ' · Not used yet'}
            </p>
            <p className="setup-state" role="status" aria-label={`${name} key state`}>{state.text}</p>
            {!present && used.length > 0 && <p className="notice alert" role="alert">{name} is chosen for {used.join(' and ')} but has no key.</p>}
            <div className="setup-actions">
              <button type="button" className={`button ${present ? '' : 'primary'}`} disabled={busy} onClick={() => void setKey(provider)}>
                {present ? 'Replace key…' : 'Add key…'}
              </button>
              <button type="button" className="button" disabled={busy || state.checking || !present} onClick={() => void runCheck(provider)}>Check key</button>
              {present && view.credential !== 'environment' && (
                <button type="button" className="button danger" disabled={busy} onClick={() => setConfirming(provider)}>Remove key…</button>
              )}
              {!present && (
                <button type="button" className="button ghost" onClick={() => void getKey(provider)}>
                  Get a key <ExternalLink size={13} aria-hidden />
                </button>
              )}
            </div>
            {confirming === provider && <div className="notice alert setup-confirm" role="group" aria-label={`Confirm ${name} key removal`}>
              <p>Remove the {name} key?{used.length > 0 ? ` ${used.join(' and ')} will stop working until you add one again.` : ''} Saved audio and transcripts stay.</p>
              <div className="row">
                <button type="button" className="button" disabled={busy} onClick={() => setConfirming(null)}>Keep key</button>
                <button type="button" className="button danger" disabled={busy} onClick={() => void removeKey(provider)}>Remove key</button>
              </div>
            </div>}
          </li>;
        })}
      </ul>
      {!cloud && !error && <p className="notice" role="status">Loading provider keys…</p>}
    </section>
    {note && <p className="notice" role="status">{note}</p>}
    {error && <p className="notice alert" role="alert">{error}</p>}
    {onOpenSection && <p className="group-note">
      Choose where keys are used in <button type="button" className="linklike" onClick={() => onOpenSection('engine')}>Speech engine</button>,{' '}
      <button type="button" className="linklike" onClick={() => onOpenSection('text')}>Text models</button> and{' '}
      <button type="button" className="linklike" onClick={() => onOpenSection('general')}>Transcript cleanup</button>.
    </p>}
  </section>;
}
