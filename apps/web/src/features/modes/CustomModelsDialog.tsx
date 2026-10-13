import { useEffect, useState } from 'react';
import type { ModelSelection } from '@metrix-writer/contracts';
import { Dialog } from '../shell/Dialog';
import { providerName } from '../transcription-setup/CloudKey';
import { customModelApi, reasoningLabel, type CustomModel, type ModelHost, type Reasoning } from './api';
import type { TextChoice } from './useModeProviders';
import type { CloudProviderId } from '../transcription-setup/api';

type Provider = ModelSelection['provider'];
const PROVIDERS: Provider[] = ['openai', 'gemini', 'grok', 'openrouter'];
const REASONING = Object.keys(reasoningLabel) as Reasoning[];
const example: Record<string, string> = { openai: 'gpt-5.6-terra', gemini: 'gemini-3.5-flash-lite', grok: 'grok-4.20', openrouter: 'publisher/model' };
interface Draft { original: CustomModel | null; provider: Provider; model: string; fast: Reasoning; balanced: Reasoning; only_hosts: string[]; ignore_hosts: string[] }
const blank = (provider: Provider): Draft => ({ original: null, provider, model: '', fast: 'provider_default', balanced: 'provider_default', only_hosts: [], ignore_hosts: [] });
const routed = (draft: Draft) => draft.provider === 'openrouter' && draft.model.includes('/');
/** How a model's hosts are chosen: OpenRouter's own routing, a pinned set, or a set to avoid. */
type HostRule = 'any' | 'only' | 'avoid';
const hostRule = (m: { only_hosts: string[]; ignore_hosts: string[] }): HostRule => m.only_hosts.length ? 'only' : m.ignore_hosts.length ? 'avoid' : 'any';
const hostSummary = (m: CustomModel) => m.only_hosts.length ? ` · only ${m.only_hosts.join(', ')}` : m.ignore_hosts.length ? ` · avoiding ${m.ignore_hosts.join(', ')}` : '';
const same = (a: CustomModel, b: CustomModel) => a.provider === b.provider && a.model === b.model;

/**
 * Cloud text models added by their API ID, opened from a mode's text model.
 *
 * Writer cannot know what an arbitrary model accepts, so each one also says
 * what a mode's Fast and Balanced speeds send as reasoning. Provider default
 * sends nothing and works everywhere; anything else is the provider's to accept.
 * The model uses its provider's existing key, which is set in Text processing.
 */
export function CustomModelsDialog({ choices, provider, onClose, onChanged, onOpenKeys }: {
  /** The mode page's model list, read here for which providers have a key. */
  choices: TextChoice[];
  /** Where the form starts: the provider of the mode's current cloud model. */
  provider?: Provider;
  onClose: () => void;
  /** The list changed, so the mode page's model list should be read again. */
  onChanged: () => void;
  /** Opens Settings › API keys on this provider. */
  onOpenKeys?: (provider: CloudProviderId) => void;
}) {
  const start = provider && PROVIDERS.includes(provider) ? provider : 'openai';
  const [models, setModels] = useState<CustomModel[] | null>(null);
  const [loadError, setLoadError] = useState('');
  const [reload, setReload] = useState(0);
  const [draft, setDraft] = useState<Draft>(blank(start));
  const [removing, setRemoving] = useState<CustomModel | null>(null);
  const [busy, setBusy] = useState(false);
  /** OpenRouter's hosts for the model being edited, read only when the section is opened. */
  const [hosts, setHosts] = useState<{ model: string; list: ModelHost[] | null; error: string } | null>(null);
  const [error, setError] = useState('');
  const [note, setNote] = useState('');

  useEffect(() => {
    let live = true;
    setLoadError('');
    customModelApi.list().then(list => { if (live) setModels(list); }).catch(e => { if (live) setLoadError((e as Error).message); });
    return () => { live = false; };
  }, [reload]);

  const keyed = (p: Provider) => choices.some(c => c.selection.provider === p && c.configured);

  async function save(next: CustomModel[], done: string) {
    setBusy(true); setError(''); setNote('');
    try {
      setModels(await customModelApi.save(next));
      setNote(done);
      onChanged();
      return true;
    } catch (e) { setError((e as Error).message); return false; }
    finally { setBusy(false); }
  }
  async function commit() {
    if (!models) return;
    const pinned = routed(draft);
    const entry: CustomModel = { provider: draft.provider, model: draft.model.trim(), fast: draft.fast, balanced: draft.balanced, only_hosts: pinned ? draft.only_hosts : [], ignore_hosts: pinned ? draft.ignore_hosts : [] };
    const next = draft.original ? models.map(m => same(m, draft.original!) ? entry : m) : [...models, entry];
    if (await save(next, draft.original ? `${entry.model} saved.` : `${entry.model} added. Choose it in the text model list.`)) setDraft(blank(entry.provider));
  }
  async function remove(model: CustomModel) {
    if (!models) return;
    setRemoving(null);
    if (await save(models.filter(m => !same(m, model)), `${model.model} removed.`) && draft.original && same(draft.original, model)) setDraft(blank(model.provider));
  }
  const reasoning = (label: string, value: Reasoning, change: (value: Reasoning) => void) => <label className="field">
    <span>{label}</span>
    <select aria-label={label} value={value} disabled={busy} onChange={e => change(e.target.value as Reasoning)}>
      {REASONING.map(r => <option key={r} value={r}>{reasoningLabel[r]}</option>)}
    </select>
  </label>;

  return <Dialog title="Custom text models" onClose={onClose}>
    <div className="dialog-body custom-models">
      <p className="dialog-note">Add any model a cloud provider serves, by the ID its API uses. It uses that provider’s key and appears in every mode’s text model list.</p>

      {loadError && <p className="custom-models-error" role="alert">{loadError} <button type="button" className="linklike" onClick={() => setReload(v => v + 1)}>Retry</button></p>}
      {!models && !loadError && <p className="dialog-note" role="status">Loading custom models…</p>}
      {models && (models.length
        ? <ul className="custom-models-list" aria-label="Your custom models">
          {models.map(m => <li key={`${m.provider}/${m.model}`} aria-label={m.model}>
            <div className="custom-models-item">
              <span className="custom-models-id" dir="ltr">{m.model}</span>
              <span className="dialog-note">{providerName[m.provider]} · Fast: {reasoningLabel[m.fast]} · Balanced: {reasoningLabel[m.balanced]}{hostSummary(m)}{keyed(m.provider) ? '' : ' · key needed'}</span>
            </div>
            <div className="custom-models-actions">
              <button type="button" className="button" disabled={busy} onClick={() => { setDraft({ original: m, provider: m.provider, model: m.model, fast: m.fast, balanced: m.balanced, only_hosts: m.only_hosts, ignore_hosts: m.ignore_hosts }); setError(''); setNote(''); }}>Edit</button>
              <button type="button" className="button danger" disabled={busy} onClick={() => setRemoving(m)}>Remove</button>
            </div>
            {removing && same(removing, m) && <div className="notice alert custom-models-confirm" role="group" aria-label={`Confirm removing ${m.model}`}>
              <p>Remove {m.model}? Modes that use it keep it selected, and their text processing fails until you add it again.</p>
              <div className="dialog-actions">
                <button type="button" className="button" disabled={busy} onClick={() => setRemoving(null)}>Keep model</button>
                <button type="button" className="button danger" disabled={busy} onClick={() => void remove(m)}>Remove model</button>
              </div>
            </div>}
          </li>)}
        </ul>
        : <p className="dialog-note">No custom models yet.</p>)}

      {models && <form className="custom-models-form" aria-label={draft.original ? `Edit ${draft.original.model}` : 'Add a custom model'} onSubmit={e => { e.preventDefault(); void commit(); }}>
        <h3>{draft.original ? `Edit ${draft.original.model}` : 'Add a custom model'}</h3>
        <label className="field">
          <span>Provider</span>
          <select aria-label="Provider" value={draft.provider} disabled={busy} onChange={e => setDraft({ ...draft, provider: e.target.value as Provider })}>
            {PROVIDERS.map(p => <option key={p} value={p}>{providerName[p]}</option>)}
          </select>
        </label>
        <label className="field">
          <span>Model ID</span>
          <input type="text" aria-label="Model ID" value={draft.model} placeholder={example[draft.provider]} maxLength={128} spellCheck={false} autoComplete="off" dir="ltr" disabled={busy} onChange={e => setDraft({ ...draft, model: e.target.value })} />
        </label>
        {reasoning('Reasoning when Fast', draft.fast, fast => setDraft({ ...draft, fast }))}
        {reasoning('Reasoning when Balanced', draft.balanced, balanced => setDraft({ ...draft, balanced }))}
        {routed(draft) && <HostRules
          key={draft.original ? `${draft.original.provider}/${draft.original.model}` : 'new'}
          model={draft.model.trim()}
          initial={hostRule(draft)}
          chosen={draft.only_hosts.length ? draft.only_hosts : draft.ignore_hosts}
          busy={busy}
          hosts={hosts && hosts.model === draft.model.trim() ? hosts : null}
          onLoad={() => {
            const model = draft.model.trim();
            setHosts({ model, list: null, error: '' });
            customModelApi.hosts(model)
              .then(list => setHosts(current => current?.model === model ? { model, list, error: '' } : current))
              .catch(e => setHosts(current => current?.model === model ? { model, list: [], error: (e as Error).message } : current));
          }}
          onChange={(rule, chosen) => setDraft({ ...draft, only_hosts: rule === 'only' ? chosen : [], ignore_hosts: rule === 'avoid' ? chosen : [] })}
        />}
        {!keyed(draft.provider) && <p className="custom-models-key" role="status">
          No {providerName[draft.provider]} key is stored yet. You can add the model now; it works once the key is set in API keys.
          {onOpenKeys && <> <button type="button" className="linklike" onClick={() => onOpenKeys(draft.provider as CloudProviderId)}>Set up {providerName[draft.provider]} key</button></>}
        </p>}
        <p className="dialog-note">
          Use the ID exactly as {providerName[draft.provider]}’s API names it{draft.provider === 'openrouter' ? ', in the form publisher/model' : ''}.
          A mode’s Model speed picks which reasoning is sent. Provider default sends no reasoning setting, so every model
          accepts it; a level the model does not support is rejected. Try the mode to check speed and formatting.
        </p>
        {error && <p className="custom-models-error" role="alert">{error}</p>}
        <div className="dialog-actions">
          {draft.original && <button type="button" className="button" disabled={busy} onClick={() => { setDraft(blank(draft.provider)); setError(''); }}>Cancel edit</button>}
          <button type="submit" className="button primary" disabled={busy || !draft.model.trim()}>{draft.original ? 'Save model' : 'Add model'}</button>
        </div>
      </form>}

      {note && <p className="notice" role="status">{note}</p>}
      <div className="dialog-actions custom-models-done">
        <button type="button" className="button" onClick={onClose}>Done</button>
      </div>
    </div>
  </Dialog>;
}

/**
 * Which hosts may serve an OpenRouter model.
 *
 * One model is served by many hosts at different speeds and quantizations, and
 * OpenRouter's latency routing keeps a host that has since become slow. Pinning
 * the good ones, or ruling out a bad one, is what makes a model dependable.
 * The list is read from OpenRouter only when this section is opened.
 */
function HostRules({ model, initial, chosen, hosts, busy, onLoad, onChange }: {
  model: string;
  /** The rule the saved model already has; kept here because choosing a rule comes before choosing hosts. */
  initial: HostRule;
  chosen: string[];
  hosts: { list: ModelHost[] | null; error: string } | null;
  busy: boolean;
  onLoad: () => void;
  onChange: (rule: HostRule, chosen: string[]) => void;
}) {
  const [open, setOpen] = useState(false);
  const [rule, setRule] = useState<HostRule>(initial);
  useEffect(() => { if (open && !hosts) onLoad(); }, [open, hosts, onLoad]);
  const toggle = (tag: string) => onChange(rule, chosen.includes(tag) ? chosen.filter(t => t !== tag) : [...chosen, tag]);
  return <details className="custom-models-hosts" open={open} onToggle={e => setOpen((e.currentTarget as HTMLDetailsElement).open)}>
    <summary>Hosts <span>{rule === 'any' ? 'Any host, fastest first' : rule === 'only' ? `Only ${chosen.length} chosen` : `Avoiding ${chosen.length}`}</span></summary>
    <div role="group" aria-label={`Hosts for ${model}`}>
      <label className="field">
        <span>Which hosts may serve it</span>
        <select aria-label="Which hosts may serve it" value={rule} disabled={busy} onChange={e => { const next = e.target.value as HostRule; setRule(next); onChange(next, next === 'any' ? [] : chosen); }}>
          <option value="any">Any host, fastest first</option>
          <option value="only">Only the hosts I choose</option>
          <option value="avoid">Any host except the ones I choose</option>
        </select>
      </label>
      {rule !== 'any' && <>
        {!hosts && <p className="dialog-note" role="status">Loading hosts…</p>}
        {hosts?.error && <p className="custom-models-error" role="alert">{hosts.error}</p>}
        {hosts?.list && (hosts.list.length
          ? <ul className="custom-models-hostlist">{hosts.list.map(host => <li key={host.tag}>
            <label className="checkbox">
              <input type="checkbox" checked={chosen.includes(host.tag)} disabled={busy} onChange={() => toggle(host.tag)} />
              <span>{host.name}<span className="dialog-note"> {host.tag}{host.quantization ? ` · ${host.quantization}` : ''}{host.uptime === null ? '' : ` · ${host.uptime.toFixed(1)}% up`}</span></span>
            </label>
          </li>)}</ul>
          : <p className="dialog-note">OpenRouter lists no hosts for this model.</p>)}
        {chosen.filter(tag => !hosts?.list?.some(host => host.tag === tag)).map(tag => <label className="checkbox" key={tag}>
          <input type="checkbox" checked disabled={busy} onChange={() => toggle(tag)} />
          <span>{tag}<span className="dialog-note"> no longer listed</span></span>
        </label>)}
      </>}
      <p className="dialog-note">Pinning also keeps the quantization steady, which is part of why the same model can answer differently from one dictation to the next.</p>
    </div>
  </details>;
}
