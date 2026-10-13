import { useCallback, useEffect, useRef, useState } from 'react';
import { Check, ChevronRight, KeyRound } from 'lucide-react';
import { request } from '../transcripts/api';
import { invoke } from '../desktop-session/runtime/native';
import { Row, Segmented } from '../settings/controls';
import { describeKey, providerName, useKeyChecks, KeyPill } from '../transcription-setup/CloudKey';
import { formatBytes, licenseName, setupApi, type CloudProviderId, type CloudView, type WeightsState } from '../transcription-setup/api';
import { useModeProviders } from './useModeProviders';
// The rows here are the setup panel's own objects, so the panel's stylesheet is
// this section's too; it is no longer mounted alongside it to bring it in.
import '../transcription-setup/setup.css';
import { PRODUCT_NAME } from '../../product';

type Backend = 'automatic' | 'cpu' | 'gpu';
type Side = 'local' | 'cloud';
interface TextModel { id: string; display_name: string; bytes: number; sha256: string; source_url: string; license: string; notes: string; weights: WeightsState }
interface View { supported: boolean; cpu_available: boolean; gpu_available: boolean; requested: Backend; last_execution: { backend: string; fallback: boolean } | null; models: TextModel[] }
const base = '/api/v1/modes/local-models';
/** OpenRouter's key serves this page alone: Writer registers no OpenRouter speech model. */
const textOnly = (provider: CloudProviderId) => provider === 'openrouter';

/**
 * Text models for Super and Command, local and cloud, on their own page.
 *
 * Two things were wrong before. The section listed only the local model, so a
 * perfectly good cloud option that already existed looked as if it did not —
 * the choice was in Settings → Modes and nothing here mentioned it. And there
 * was exactly one local model with no indication of who published it, which is
 * part of the decision and cannot be read off a file name.
 *
 * Unlike the speech model, nothing here is "selected": a text model is assigned
 * per mode. This section installs them and reports what is usable; Modes is
 * where one gets picked, and both pages now say so.
 */
export function TextModels({ onOpenSection, onOpenKeys }: {
  onOpenSection?: (section: 'modes') => void;
  /**
   * Opens Settings › API keys on one provider. The page that reports a missing
   * key is one step from fixing it, but the key itself is changed only there
   * (MW-SETTINGS-9). Absent in the browser client, which has no key write route.
   */
  onOpenKeys?: (provider: CloudProviderId) => void;
} = {}) {
  const [view, setView] = useState<View | null>(null);
  const [side, setSide] = useState<Side>('local');
  const [expanded, setExpanded] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  /** Cloud key state, read from the setup view. Null where there is no such route. */
  const [keys, setKeys] = useState<CloudView[] | null>(null);
  const providers = useModeProviders();
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    let live = true;
    const refresh = () => request<View>(base).then(v => { if (live) setView(v); }).catch(e => { if (live) setMessage(e.message); });
    void refresh(); const timer = setInterval(() => void refresh(), 1500);
    return () => { live = false; clearInterval(timer); };
  }, []);
  const loadKeys = useCallback(async () => {
    try { const next = await setupApi.load(); if (mounted.current) setKeys(next.cloud); }
    catch { if (mounted.current) setKeys(null); }
  }, []);
  // Only while the cloud side is open: this is the speech panel's document, and
  // nothing on the local side of this page depends on it.
  useEffect(() => {
    if (side !== 'cloud') return;
    void loadKeys();
    const timer = setInterval(() => void loadKeys(), 5000);
    return () => clearInterval(timer);
  }, [side, loadKeys]);
  const checkKey = useCallback(async (provider: CloudProviderId) => {
    try { const next = await setupApi.checkCloud(provider); if (mounted.current) setKeys(next.cloud); }
    catch { /* The pill keeps saying unchecked. */ }
  }, []);
  const { checking } = useKeyChecks(keys, checkKey);

  async function action(model: TextModel, name: string) {
    setBusy(true); setMessage('');
    try { setView(await request<View>(`${base}/${encodeURIComponent(model.id)}/${name}`, { method: 'POST' })); }
    catch (e) { setMessage((e as Error).message); }
    finally { setBusy(false); }
  }
  async function execution(backend: Backend) {
    setBusy(true); setMessage('');
    try { setView(await request<View>(`${base}/execution`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ api_version: '1', data: backend }) })); }
    catch (e) { setMessage((e as Error).message); }
    finally { setBusy(false); }
  }
  async function importModel(model: TextModel) {
    setBusy(true); setMessage('');
    try { await invoke('import_model', { model: model.id }); setView(await request<View>(base)); }
    catch (e) { setMessage(typeof e === 'string' ? e : (e as Error).message); }
    finally { setBusy(false); }
  }

  const cloud = providers.choices.filter(choice => choice.execution_location === 'cloud');

  return <section className="writer-modes setup-panel" aria-label="Text models">
    <section className="settings-group">
      <h3>Text models for Super and Command</h3>
      <div className="setup-sides">
        <Segmented
          label="Where text processing runs"
          value={side}
          disabled={busy}
          options={[{ id: 'local' as Side, label: 'Local' }, { id: 'cloud' as Side, label: 'Cloud' }]}
          onChange={setSide}
        />
      </div>

      {side === 'local' ? <>
        {view && !view.supported && (
          <div className="settings-card">
            <Row title="Not available on this platform" detail="Packaged local text inference is not available on this platform in this build." />
          </div>
        )}
        <ul className="setup-models">
          {view?.models.map(model => <TextModelRow
            key={model.id}
            model={model}
            busy={busy}
            expanded={expanded === model.id}
            onExpand={() => setExpanded(current => current === model.id ? null : model.id)}
            onAction={name => void action(model, name)}
            onImport={() => void importModel(model)}
          />)}
        </ul>
        {view && (
          <div className="settings-card setup-text-footer">
            <Row
              title="Processor for text processing"
              detail={<>
                A separate runtime from the one speech recognition uses; this choice does not affect transcription.
                Packaged runtimes: CPU {view.cpu_available ? 'present' : 'missing'} · GPU {view.gpu_available ? 'present' : 'missing'}.
                {view.last_execution && ` Last completed text request: ${view.last_execution.backend === 'cpu' ? 'CPU' : 'GPU (Vulkan)'}${view.last_execution.fallback ? ' after GPU fallback' : ''}.`}
              </>}
            >
              <select aria-label="Processor for text processing" disabled={busy} value={view.requested} onChange={e => void execution(e.target.value as Backend)}>
                <option value="automatic">Automatic (GPU with CPU fallback)</option>
                <option value="cpu">CPU</option>
                <option value="gpu">GPU</option>
              </select>
            </Row>
          </div>
        )}
      </> : <>
        <ul className="setup-cloud">
          {cloud.filter((choice, index) => cloud.findIndex(c => c.selection.provider === choice.selection.provider) === index).map(choice => {
            const provider = choice.selection.provider as CloudProviderId;
            const name = providerName[choice.selection.provider] ?? provider;
            const stored = keys?.find(item => item.provider === choice.selection.provider) ?? null;
            const state = stored ? describeKey(stored, checking.includes(provider)) : null;
            const present = stored ? stored.credential === 'environment' || stored.credential === 'protected_store' : choice.configured;
            return <li className="setup-cloud-row" key={`${choice.selection.provider}/${choice.selection.model}`} aria-label={`${name} text model`}>
              <div className="setup-model-head">
                <KeyRound size={14} aria-hidden className="setup-cloud-glyph" />
                <strong>{name}</strong>
                {/* The same pill the speech page shows, from the same document:
                    one key per provider, so one answer about it. */}
                {state
                  ? <>{present && <span className="badge">Key stored</span>}<KeyPill state={state} /></>
                  : <span className="badge"><span className={`dot ${present ? 'ok' : 'bad'}`} aria-hidden />{present ? 'Key stored' : 'No key'}</span>}
              </div>
              <p className="setup-model-facts">{cloud.filter(c => c.selection.provider === provider && !c.custom).map(c => c.selection.model).join(" · ")}</p>
              <p className="setup-state" role="status" aria-label={`${name} text model`}>
                {present
                  ? `${state ? `${state.text} ` : ''}Ready to assign to a mode. Transcript text and any context you enable are sent to this provider.`
                  : textOnly(provider) ? `No key stored. ${name} is used for text processing only.` : `No key stored. One ${name} key serves both transcription and text processing.`}
              </p>
              {/* One step to the key, as on the speech page. No model select and
                  no "Use": a text model is assigned by a mode, not selected here. */}
              {onOpenKeys && (
                <div className="setup-actions">
                  <button type="button" className={`button ${present ? '' : 'primary'}`} onClick={() => onOpenKeys(provider)}>
                    {present ? 'Manage key' : 'Set up key'}
                  </button>
                </div>
              )}
            </li>;
          })}
        </ul>
        {!cloud.length && <p className="notice" role="status">Loading cloud text providers…</p>}
        <p className="group-note">
          {onOpenKeys
            ? 'One key per provider serves transcription, text processing and cleanup. Keys are added, checked and removed in Settings › API keys.'
            : 'This is the self-hosted browser client: provider keys are read from the server environment.'}
        </p>
      </>}

      <p className="group-note">
        These are separate from the speech model: this one rewrites a transcript, that one hears it. Nothing here is
        selected — a mode picks its own text model, and custom cloud models are added there with Manage custom models.
        {onOpenSection && <> <button type="button" className="linklike" onClick={() => onOpenSection('modes')}>Open Modes</button></>}
      </p>
    </section>

    {message && <p className="notice alert" role="alert">{message}</p>}
  </section>;
}

function TextModelRow({ model, busy, expanded, onExpand, onAction, onImport }: {
  model: TextModel;
  busy: boolean;
  expanded: boolean;
  onExpand: () => void;
  onAction: (name: string) => void;
  onImport: () => void;
}) {
  const [confirmDelete, setConfirmDelete] = useState(false);
  const weights = model.weights;
  const installed = weights.state === 'installed';
  const transferring = weights.state === 'downloading' || weights.state === 'verifying';
  const progress = weights.state === 'downloading' && weights.total_bytes
    ? Math.min(100, Math.round((100 * weights.downloaded_bytes) / weights.total_bytes))
    : null;
  const state = weights.state === 'installed' ? 'Downloaded and verified'
    : weights.state === 'downloading' ? `Downloading ${formatBytes(weights.downloaded_bytes)} of ${formatBytes(weights.total_bytes)}`
    : weights.state === 'verifying' ? 'Verifying…'
    : weights.state === 'failed' ? `Model setup failed: ${weights.error}`
    : weights.resumable_bytes ? `Not installed · ${formatBytes(weights.resumable_bytes)} of a previous download kept`
    : 'Not installed';

  return <li className="setup-model" aria-label={model.display_name}>
    <div className="setup-model-main">
      <button type="button" className="setup-model-open" aria-expanded={expanded} onClick={onExpand}>
        <ChevronRight size={14} aria-hidden className="setup-model-chevron" />
        <span className="setup-model-title">
          <span className="setup-model-head">
            <strong>{model.display_name}</strong>
            {installed && <span className="badge in-use"><Check size={12} aria-hidden />Installed</span>}
          </span>
          {/* Who published the weights is part of the choice, so it is on the
              collapsed row rather than behind a disclosure. */}
          <span className="setup-model-facts">{formatBytes(model.bytes)} · {licenseName(model.license)} · {model.notes.split('.')[0]}</span>
        </span>
      </button>
      <div className="setup-model-action">
        {weights.state === 'downloading' && <button type="button" className="button" disabled={busy} onClick={() => onAction('cancel')}>Cancel download</button>}
        {weights.state === 'verifying' && <span className="badge">Verifying…</span>}
        {!installed && !transferring && <button type="button" className="button" disabled={busy} onClick={() => onAction('download')}>
          {weights.resumable_bytes
            ? `Resume download (${formatBytes(weights.resumable_bytes)} kept)`
            : `Download ${formatBytes(model.bytes)}`}
        </button>}
      </div>
    </div>

    <p className="setup-state" role="status" aria-label={`${model.display_name} weights`}>{state}</p>
    {progress !== null && <div className="progress" aria-hidden><i style={{ width: `${progress}%` }} /></div>}

    {expanded && <div className="setup-model-detail">
      <p className="setup-notes">{model.notes}</p>
      <p className="setup-source">
        {formatBytes(model.bytes)} · SHA-256 {model.sha256.slice(0, 12)}… ·{' '}
        <a href={model.source_url.replace('/resolve/', '/blob/')} target="_blank" rel="noreferrer">Pinned source</a>
      </p>
      <div className="setup-actions">
        <button type="button" className="button" disabled={busy || transferring} onClick={onImport}>Import GGUF…</button>
        <button type="button" className="button" disabled={busy || transferring} onClick={() => onAction('verify')}>Verify</button>
        <button type="button" className="button danger" disabled={busy || transferring} onClick={() => setConfirmDelete(true)}>Delete text model</button>
      </div>
      {confirmDelete && <div className="notice alert setup-confirm" role="group" aria-label="Confirm text model deletion">
        <p>Delete {PRODUCT_NAME}’s copy of {model.display_name}? Modes that use it keep the assignment and will need it downloaded again.</p>
        <div className="row">
          <button type="button" className="button" disabled={busy} onClick={() => setConfirmDelete(false)}>Keep model</button>
          <button type="button" className="button danger" disabled={busy} onClick={() => { setConfirmDelete(false); onAction('delete'); }}>Confirm deletion</button>
        </div>
      </div>}
    </div>}
  </li>;
}
