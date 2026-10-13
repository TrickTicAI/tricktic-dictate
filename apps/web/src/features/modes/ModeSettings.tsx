import { ContextNotice } from './ContextNotice';
import { useEffect, useState } from 'react';
import { Command as CommandIcon, Mic, Sparkles } from 'lucide-react';
import { textModelName } from '../home/language';
import type { ModelSelection } from '@metrix-writer/contracts';
import { useModeProviders } from './useModeProviders';
import { ShortcutCapture } from '../settings/ShortcutCapture';
import { Group, Row, Segmented, Toggle } from '../settings/controls';
import { Problem } from '../shell/Problem';
import { modeApi, emptyContext, defaultFormatting, reasoningSent, DEFAULT_FALLBACK_MS, FALLBACK_RANGE, polishTimeout, type Formatting, type ModeProfile, type ModeSettings as Settings } from './api';
import { TryMode } from './TryMode';
import { CustomModelsDialog } from './CustomModelsDialog';
import { ComparePrompts } from './ComparePrompts';
import './modes.css';
import { PRODUCT_NAME } from '../../product';
import type { CloudProviderId } from '../transcription-setup/api';
const CONTEXT = [{ key: 'selected_text', label: 'Selected text' }, { key: 'application', label: 'Application context' }, { key: 'clipboard', label: 'Clipboard text' }] as const;
const normalize = (s: Settings): Settings => ({ ...s, profiles: s.profiles.map(p => ({ ...p, formatting: { ...defaultFormatting, ...p.formatting }, process_while_speaking: p.process_while_speaking ?? false, fast_processing: p.fast_processing ?? true })) });

export function ModeSettings({ onOpenSection, onOpenKeys, onDirtyChange, onBusyChange }: { onOpenSection?: (section: 'text') => void; onOpenKeys?: (provider: CloudProviderId) => void; onDirtyChange?: (dirty: boolean) => void; onBusyChange?: (busy: boolean) => void } = {}) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [baseline, setBaseline] = useState<Settings | null>(null);
  const [defaults, setDefaults] = useState<Settings | null>(null);
  const [selected, setSelected] = useState('regular');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [editingShortcut, setEditingShortcut] = useState(false);
  const [customModels, setCustomModels] = useState(false);
  const [comparing, setComparing] = useState(false);
  const [compareDefault, setCompareDefault] = useState(false);
  const [reload, setReload] = useState(0);
  const providers = useModeProviders();
  useEffect(() => { onBusyChange?.(busy); }, [busy, onBusyChange]);
  const dirty = !!settings && JSON.stringify(settings) !== JSON.stringify(baseline);
  useEffect(() => { onDirtyChange?.(dirty); }, [dirty, onDirtyChange]);
  useEffect(() => {
    if (!dirty) return;
    const prevent = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ''; };
    window.addEventListener('beforeunload', prevent);
    return () => window.removeEventListener('beforeunload', prevent);
  }, [dirty]);
  useEffect(() => {
    let mounted = true;
    modeApi.settings().then(s => { if (mounted) { const next = normalize(s); setSettings(next); setBaseline(next); setSelected(next.active); setError(''); } }).catch(e => { if (mounted) setError(e.message); });
    modeApi.defaults().then(s => { if (mounted) setDefaults(s); }).catch(() => {});
    return () => { mounted = false; };
  }, [reload]);
  const profile = settings?.profiles.find(p => p.id === selected);
  const choice = providers.choices.find(c => c.selection.provider === profile?.text_model?.provider && c.selection.model === profile?.text_model?.model);
  const ready = profile?.kind === 'regular' || !!choice?.configured;
  const fallbackReady = providers.choices.some(c => c.configured && c.selection.provider === profile?.fallback_model?.provider && c.selection.model === profile?.fallback_model?.model);
  const defaultPrompt = defaults?.profiles.find(p => p.kind === profile?.kind)?.instructions;
  function update(change: Partial<ModeProfile>) {
    if (settings && profile) { setSettings({ ...settings, profiles: settings.profiles.map(p => p.id === profile.id ? { ...p, ...change } : p) }); setMessage(''); }
  }
  async function checkModel(role: 'primary' | 'fallback') {
    const model = role === 'primary' ? profile?.text_model : profile?.fallback_model;
    if (!profile || !model) return;
    setBusy(true); setError(''); setMessage('');
    try {
      const sample = 'שלום hello i wanted to confirm our meeting tomorrow';
      const base = defaults?.profiles.find(p => p.kind === 'super') ?? { ...profile, kind: 'super' as const };
      await modeApi.preview({ ...base, text_model: model, fallback_model: null, process_while_speaking: false }, sample, emptyContext());
      setMessage(`${role === 'primary' ? 'Primary' : 'Fallback'} sample succeeded with ${model.model}.`);
    } catch (e) { setError(`${role === 'primary' ? 'Primary' : 'Fallback'} check failed: ${(e as Error).message}`); }
    finally { setBusy(false); }
  }
  /** The mode's model list, used for its text model and for its fallback. */
  function modelSelect(label: string, value: ModelSelection | null, empty: string, onChange: (model: ModelSelection | null) => void, cloudOnly = false) {
    const options = providers.choices.filter(c => !cloudOnly || c.execution_location === 'cloud');
    const listed = options.some(c => value && c.selection.provider === value.provider && c.selection.model === value.model);
    return <select aria-label={label} disabled={busy} value={value ? `${value.provider}/${value.model}` : ''} onChange={e => { const [provider, ...model] = e.target.value.split('/'); onChange(model.length ? { provider, model: model.join('/') } as ModelSelection : null); }}>
      <option value="">{empty}</option>
      {value && !listed && <option value={`${value.provider}/${value.model}`}>{value.model} · unavailable</option>}
      {options.map(c => <option key={`${c.selection.provider}/${c.selection.model}`} value={`${c.selection.provider}/${c.selection.model}`}>{c.selection.model} · {c.selection.provider === 'local' ? 'local' : c.selection.provider === 'openrouter' ? 'OpenRouter' : 'cloud'}{c.custom ? ' · custom' : ''}{c.configured ? '' : c.selection.provider === 'local' ? ' · download needed' : ' · key needed'}</option>)}
    </select>;
  }
  function format(change: Partial<Formatting>) { update({ formatting: { ...defaultFormatting, ...profile?.formatting, ...change } }); }
  async function save() {
    if (!settings) return;
    setBusy(true); setMessage(''); setError('');
    try { const next = normalize(await modeApi.save(settings)); setSettings(next); setBaseline(next); setMessage(`Saved. ${next.profiles.find(p => p.id === next.active)?.name} is your default recording mode.`); }
    catch (e) { setError((e as Error).message); }
    finally { setBusy(false); }
  }
  function discard() { if (baseline) { setSettings(baseline); if (!baseline.profiles.some(p => p.id === selected)) setSelected(baseline.active); } setError(''); setMessage('Changes discarded.'); }
  function duplicate() {
    if (!profile || !settings || settings.profiles.length >= 100) return;
    const copy = { ...profile, id: crypto.randomUUID(), revision: 1, shortcut: null, name: `${profile.name} copy` };
    setSettings({ ...settings, profiles: [...settings.profiles, copy] }); setSelected(copy.id);
  }
  function exportSettings() {
    if (!settings) return;
    const url = URL.createObjectURL(new Blob([JSON.stringify({ api_version: '1', data: settings }, null, 2)], { type: 'application/json' }));
    const a = document.createElement('a'); a.href = url; a.download = 'writer-modes.json'; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async function importSettings(file?: File) {
    if (!file || !settings) return;
    try {
      if (file.size > 256 * 1024) throw new Error('Mode settings file exceeds 256 KiB.');
      const doc = JSON.parse(await file.text());
      if (doc.api_version !== '1' || !Array.isArray(doc.data?.profiles)) throw new Error(`Choose a ${PRODUCT_NAME} mode-settings export.`);
      const valid = (p: ModeProfile) => p && typeof p.name === 'string' && typeof p.instructions === 'string' && ['regular', 'super', 'command'].includes(p.kind) && p.context && CONTEXT.every(({ key }) => typeof p.context[key] === 'boolean') && (p.text_model === null || p.text_model && ['openai', 'gemini', 'grok', 'local', 'openrouter'].includes(p.text_model.provider) && typeof p.text_model.model === 'string') && (p.fallback_model === undefined || p.fallback_model === null || p.fallback_model && ['openai', 'gemini', 'grok', 'openrouter'].includes(p.fallback_model.provider) && typeof p.fallback_model.model === 'string') && (p.formatting === undefined || p.formatting && ['light', 'polished', 'concise'].includes(p.formatting.rewrite) && ['automatic', 'single_line'].includes(p.formatting.layout) && ['lists', 'remove_fillers', 'self_corrections', 'spoken_punctuation'].every(k => typeof p.formatting![k as keyof Formatting] === 'boolean')) && (p.process_while_speaking === undefined || typeof p.process_while_speaking === 'boolean') && (p.fast_processing === undefined || typeof p.fast_processing === 'boolean');
      if (doc.data.profiles.length + settings.profiles.length > 100 || !doc.data.profiles.every(valid)) throw new Error('The export contains invalid or excessive mode profiles.');
      // Imported shortcuts and early cloud sending are opt-in on this machine.
      const copies: ModeProfile[] = doc.data.profiles.map((p: ModeProfile) => ({ ...p, id: crypto.randomUUID(), revision: 1, shortcut: null, process_while_speaking: false }));
      setSettings(normalize({ ...settings, profiles: [...settings.profiles, ...copies] })); setMessage('Imported as new profiles. Review and save to apply.');
    } catch (e) { setError((e as Error).message); }
  }
  if (!settings || !profile) return <section className="writer-modes" aria-label="Dictation modes">{error ? <Problem title="Dictation modes could not be loaded" detail={error} keeps="Your recordings and transcripts are retained." onRetry={() => setReload(v => v + 1)} /> : <p role="status">Loading dictation modes…</p>}</section>;
  const formatting = profile.formatting ?? defaultFormatting;
  const active = settings.profiles.find(p => p.id === settings.active)!;
  const activeReady = active.kind === 'regular' || providers.choices.some(c => c.configured && c.selection.provider === active.text_model?.provider && c.selection.model === active.text_model?.model);
  return <section className="writer-modes modes-editor" aria-label="Dictation modes">
    <Group title="Your recording mode">
      <Row title="Default recording mode" detail={dirty && baseline?.active !== settings.active ? `Will switch to ${active.name} when you save.` : 'Used by the main recording shortcut.'}>
        <select aria-label="Default recording mode" disabled={busy} value={settings.active} onChange={e => { setSettings({ ...settings, active: e.target.value }); setSelected(e.target.value); setMessage(''); }}>
          {settings.profiles.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
      </Row>
    </Group>
    {/* Which mode to edit is its own choice: picking a card never changes the
        default, and changing the default above opens that mode here. */}
    <div className="mode-cards" role="radiogroup" aria-label="Mode to edit">
      {settings.profiles.map(p => {
        const Icon = p.kind === 'super' ? Sparkles : p.kind === 'command' ? CommandIcon : Mic;
        return <button key={p.id} type="button" role="radio" aria-checked={p.id === selected} className="mode-card" onClick={() => setSelected(p.id)}>
          <span className="mode-card-icon" aria-hidden><Icon size={18} /></span>
          <span className="mode-card-text">
            <strong>{p.name}</strong>
            <span>{p.kind === 'regular' ? 'Your words, with your dictionary’s fixes' : p.kind === 'command' ? 'An instruction for the selected text' : 'Punctuation and flow'}{p.text_model ? ` · ${textModelName(p.text_model.model)}` : ''}</span>
          </span>
          {p.id === settings.active && <span className="badge">Default</span>}
        </button>;
      })}
    </div>
    <div className="mode-profile-heading"><span>Customize <strong>{profile.name}</strong>{settings.active === profile.id && <span className="badge">Default</span>}</span>

    </div>
    {profile.kind === 'regular' ? <p className="group-note">Regular keeps your dictation with vocabulary corrections and exact snippets. Choose Super for polished writing, or Command to edit selected text with a spoken instruction.</p> : <Group>
      <Row title="Text model" detail={ready ? `${choice?.selection.provider === 'local' ? 'On this computer’s server · Installed' : 'Cloud processing · Key saved; use Check primary to verify a response'}` : 'Choose a model and add its key to use this mode.'}>
        {modelSelect('Text-processing model', profile.text_model ?? null, 'Choose a text model', text_model => update({ text_model }))}
        <button type="button" className="linklike" disabled={busy} onClick={() => setCustomModels(true)}>Manage custom models</button>
        {onOpenSection && <button type="button" className="linklike" onClick={() => onOpenSection('text')}>Manage models</button>}
        <button type="button" className="linklike" disabled={busy || !ready || !profile.text_model} onClick={() => void checkModel('primary')}>Check primary</button>
      </Row>
      <Row title="Fallback model" detail={profile.fallback_model
        ? `It starts immediately if the text model fails, or once the text model is clearly slower than usual: twice its typical time over recent dictations, and not before the wait below. The first valid answer is used. Only then are both requests charged. ${fallbackReady ? 'Key saved; check a sample to verify it.' : 'Its provider key is missing.'}${profile.fallback_model.provider === profile.text_model?.provider ? ' Both models share a provider; account limits can affect both.' : ''}`
        : 'Optional. A second cloud model answers when the first one is slow or fails, so a slow host does not hold up your dictation.'}>
        {modelSelect('Fallback model', profile.fallback_model ?? null, 'No fallback', fallback_model => update({ fallback_model }), true)}
        {profile.fallback_model && <button type="button" className="linklike" disabled={busy || !fallbackReady} onClick={() => void checkModel('fallback')}>Check fallback</button>}
      </Row>
      {profile.fallback_model && <Row title="Wait before the fallback" detail="The shortest time a slow primary gets before the backup joins. A primary that usually takes longer gets twice its usual time instead. A failed or invalid response starts the backup immediately.">
        <input type="number" aria-label="Wait before the fallback" min={FALLBACK_RANGE.min / 1000} max={FALLBACK_RANGE.max / 1000} step={0.25} disabled={busy}
          value={(profile.fallback_after_ms ?? DEFAULT_FALLBACK_MS) / 1000}
          onChange={e => { const seconds = Number(e.target.value); if (Number.isFinite(seconds)) update({ fallback_after_ms: Math.min(FALLBACK_RANGE.max, Math.max(FALLBACK_RANGE.min, Math.round(seconds * 1000))) }); }} />
        <span className="small muted">seconds</span>
      </Row>}
      {profile.kind === 'super' && <Row title="Maximum polishing wait" detail={`Up to ${(polishTimeout(profile) / 1000).toFixed(2)} seconds after transcription. If neither model returns valid text, use the original with your corrections and snippets. The backup gets at least 2 seconds after it joins, so this wait grows when the primary is usually slower.`}>
        <input type="number" aria-label="Maximum polishing wait" min={1} max={30} step={0.25} disabled={busy} value={polishTimeout(profile) / 1000}
          onChange={e => { const seconds = Number(e.target.value); if (Number.isFinite(seconds)) update({ polish_timeout_ms: Math.min(30000, Math.max(1000, Math.round(seconds * 1000))) }); }} />
        <span className="small muted">seconds</span>
      </Row>}
    </Group>}
    {profile.kind === 'super' && <>
      <Group title="Writing style">
        <Row title="Rewrite strength"><Segmented label="Rewrite strength" disabled={busy} value={formatting.rewrite} onChange={rewrite => format({ rewrite })} options={[{ id: 'light', label: 'Light' }, { id: 'polished', label: 'Polished' }, { id: 'concise', label: 'Concise' }]} /></Row>
        <Row title="Layout"><Segmented label="Layout" disabled={busy} value={formatting.layout} onChange={layout => format({ layout })} options={[{ id: 'automatic', label: 'Natural paragraphs' }, { id: 'single_line', label: 'One line' }]} /></Row>
        <div className="mode-options">
          {([{ key: 'remove_fillers', label: 'Remove fillers and false starts' }, { key: 'self_corrections', label: 'Resolve spoken corrections' }, { key: 'lists', label: 'Format lists automatically' }, { key: 'spoken_punctuation', label: 'Recognize spoken punctuation' }] as const).map(({ key, label }) => <label key={key}><input type="checkbox" checked={formatting[key]} disabled={busy || key === 'lists' && formatting.layout === 'single_line'} onChange={e => format({ [key]: e.target.checked })} />{label}</label>)}
        </div>
      </Group>
      <Group title="Speed" note="Available in the desktop app with local speech recognition and a cloud text model. Nothing is sent while you speak. When you release, one request with the newest preview starts while the final transcription runs. If the final transcript matches, that request is the only one; if not, the final transcript is sent as well. When recent previews have rarely matched, the early request is skipped until they match again.">
        <Row title="Process while speaking" detail="Prepare formatting at release to reduce the wait."><Toggle label="Process while speaking" checked={profile.process_while_speaking ?? false} disabled={busy || profile.text_model?.provider === 'local'} onChange={process_while_speaking => update({ process_while_speaking })} /></Row>
      </Group>
    </>}
    {profile.kind !== 'regular' && <TryMode key={profile.id} profile={profile} ready={ready} />}
    <ContextNotice profile={profile} />
    <details className="mode-disclosure"><summary>Advanced settings <span>Instructions, context and shortcuts</span></summary>
      <Group>
        <Row title="Name"><input aria-label="Name" value={profile.name} maxLength={160} disabled={busy} onChange={e => update({ name: e.target.value })} /></Row>
        {!['regular', 'super', 'command'].includes(profile.id) && <Row title="Behavior" detail="Custom dictation polishes speech; Command transforms selected text."><select aria-label="Behavior" disabled={busy} value={profile.kind} onChange={e => update({ kind: e.target.value as ModeProfile['kind'] })}><option value="regular">Regular dictation</option><option value="super">Custom dictation</option><option value="command">Command</option></select></Row>}
        <Row title="Mode shortcut" detail="Record directly into this profile."><span className="small muted">{profile.shortcut ?? 'None'}</span><button type="button" className="button" disabled={busy} onClick={() => setEditingShortcut(true)}>Set mode shortcut…</button>{profile.shortcut && <button type="button" className="button" disabled={busy} onClick={() => update({ shortcut: null })}>Remove shortcut</button>}</Row>
        {profile.kind !== 'regular' && <>
          <Row title="Instructions" detail="Your style instructions. The writing controls take precedence; language and exact-snippet protections always apply." stacked><textarea aria-label="Instructions" rows={7} disabled={busy} value={profile.instructions} maxLength={16384} onChange={e => update({ instructions: e.target.value })} />
            {defaultPrompt !== undefined && <button type="button" className="linklike" onClick={() => setCompareDefault(v => !v)}>Compare with new default</button>}
            <button type="button" className="linklike" disabled={busy || !ready} onClick={() => setComparing(true)}>Compare prompt versions{profile.prompt_versions?.length ? ` (${profile.prompt_versions.length} saved)` : ''}</button>
            {compareDefault && <div className="mode-default"><p>{defaultPrompt}</p><button type="button" className="button" disabled={busy} onClick={() => { update({ instructions: defaultPrompt! }); setCompareDefault(false); }}>Use default instructions</button></div>}
          </Row>
          <Row title="Model speed" detail={choice?.custom ? `${choice.selection.model} is a custom model: Fast sends ${reasoningSent(choice.custom.fast)} and Balanced sends ${reasoningSent(choice.custom.balanced)}. Change this in Manage custom models.` : `Luna, Grok 4.3 and ${PRODUCT_NAME}’s OpenRouter models disable reasoning for Fast and use medium for Balanced. Gemini and Grok 4.6 use low for Fast and medium for Balanced. GPT-4.1 mini has no reasoning step. Custom models send what you chose for them in Manage custom models.`}><Segmented label="Model speed" disabled={busy} value={profile.fast_processing === false ? 'balanced' : 'fast'} options={[{ id: 'fast', label: 'Fast' }, { id: 'balanced', label: 'Balanced' }]} onChange={value => update({ fast_processing: value === 'fast' })} /></Row>
          <Row title="Allowed context" detail="Only enabled context is collected and sent with your transcript. Selected text on macOS requires Accessibility access." stacked><div className="mode-context">{CONTEXT.map(({ key, label }) => <label className="checkbox" key={key}><input type="checkbox" disabled={busy} checked={profile.context[key]} onChange={e => update({ context: { ...profile.context, [key]: e.target.checked } })} />{label}</label>)}</div></Row>
        </>}
      </Group>
    </details>
    <details className="mode-disclosure"><summary>Manage profiles <span>Duplicate, import and export</span></summary><div className="mode-transfer"><label>Edit a profile without activating it<select aria-label="Editing profile" disabled={busy} value={selected} onChange={e => { setSelected(e.target.value); setCompareDefault(false); }}>{settings.profiles.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}</select></label><button type="button" className="button" disabled={busy || settings.profiles.length >= 100} onClick={duplicate}>Duplicate mode</button><button type="button" className="button" disabled={busy} onClick={exportSettings}>Export modes</button><label>Import as new profiles<input type="file" aria-label="Import modes" accept="application/json,.json" disabled={busy} onChange={e => { void importSettings(e.target.files?.[0]); e.target.value = ''; }} /></label></div></details>
    {message && <p className="notice" role="status">{message}</p>}
    {!dirty && error && <p role="alert">{error}</p>}
    {dirty && <div className="mode-savebar" role="region" aria-label="Unsaved mode changes"><div><strong>{busy ? 'Saving…' : 'Unsaved changes'}</strong>{error && <p role="alert">{error} <button type="button" className="linklike" disabled={busy} onClick={() => setReload(v => v + 1)}>Reload and discard draft</button></p>}{!activeReady && <p>Choose a ready text model for {active.name} before activating it.</p>}</div><button type="button" className="button" disabled={busy} onClick={discard}>Discard changes</button><button type="button" className="button primary" disabled={busy || !activeReady} onClick={() => void save()}>Save changes</button></div>}
    {comparing && <ComparePrompts profile={profile} onClose={() => setComparing(false)}
      onUse={version => { update({ instructions: version.instructions, formatting: version.formatting, fast_processing: version.fast_processing }); setComparing(false); setMessage(`Using “${version.name}”. Save to apply it to ${profile.name}.`); }}
      onSaveVersion={version => update({ prompt_versions: [...(profile.prompt_versions ?? []), version] })} />}
    {customModels && <CustomModelsDialog choices={providers.choices} provider={profile.text_model?.provider} onClose={() => setCustomModels(false)} onChanged={providers.refresh} onOpenKeys={onOpenKeys && (provider => { setCustomModels(false); onOpenKeys(provider); })} />}
    {editingShortcut && <ShortcutCapture title="mode" current={profile.shortcut ?? 'none'} otherChord="" otherLabel="another shortcut" onCommit={shortcut => { update({ shortcut }); setEditingShortcut(false); }} onCancel={() => setEditingShortcut(false)} onReset={() => { update({ shortcut: null }); setEditingShortcut(false); }} />}
  </section>;
}
