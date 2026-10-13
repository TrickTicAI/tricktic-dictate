import { isMac } from '../settings/platform';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { Cloud, Cpu, Download, KeyRound, ShieldCheck } from 'lucide-react';
import type { ProviderId, Settings } from '@metrix-writer/contracts';
import { preferencesBridge, type CloudProvider } from '../desktop-preferences/bridge';
import { describeRuntimeProblem, describeWeightsError, describeWorkerError, formatBytes, licenseName, transcribes, type CloudProviderId, type ModelView, type SetupApi, type SetupView } from '../transcription-setup/api';
import { describeKey, KeyPill, providerName as PROVIDER_NAME, useKeyChecks } from '../transcription-setup/CloudKey';
import { recommendModel } from './recommend';
import { PRODUCT_NAME } from '../../product';

/**
 * Whether the selected engine can actually transcribe now.
 *
 * A saved choice is required, so the guide cannot be walked past on a default
 * that happens to match a key left over from an earlier install. Local means a
 * worker that has really loaded these weights; cloud means a key exists, which
 * is honestly less than proof — the practice step is what proves it.
 */
export function engineReady(view: SetupView | null): boolean {
  if (!view || view.first_run) return false;
  const selection = view.settings.transcription.selection;
  if (selection.provider === 'local') {
    const worker = view.runtime.worker;
    return worker.state === 'ready' && worker.model === selection.model;
  }
  const cloud = view.cloud.find((item) => item.provider === selection.provider);
  return !!cloud && cloud.credential !== 'absent' && cloud.credential !== 'unreadable';
}

function describeWorker(view: SetupView): string {
  const worker = view.runtime.worker;
  switch (worker.state) {
    case 'idle': return 'No model is loaded yet.';
    case 'missing_runtime': return describeRuntimeProblem(worker.detail);
    case 'missing_weights': return `${worker.model} is selected but its weights are not installed.`;
    case 'starting': return `Loading ${worker.model} into memory… the first load can take a minute.`;
    case 'ready': return `${worker.model} is loaded and running on ${worker.device} (${worker.backend}).`;
    case 'failed': return describeWorkerError(worker.error);
    case 'stopping': return `Unloading ${worker.model}…`;
  }
}

export interface EngineProps {
  view: SetupView | null;
  loadError: string;
  api: SetupApi;
  reload: () => void;
  /** Adopts the document an action answered with, without waiting for a poll. */
  apply: (view: SetupView) => void;
  settings: Settings;
  onSelected: (settings: Settings) => void;
  /**
   * Injected so the guide can be exercised without the native key dialog. The
   * real dialog answers `saved` or `canceled`; the guide does not need to know
   * which, because the provider row it reloads into says what is stored now.
   */
  credentials?: { prompt: (provider: CloudProvider) => Promise<unknown> };
}

/**
 * Gets exactly one engine ready.
 *
 * The recommendation is drawn from the catalog this backend publishes and this
 * machine's reported memory and runtimes. It is about download size and whether
 * the model claims the chosen language — never about how accurate it will be on
 * your own speech, which only your own dictation can tell you.
 */
export function Engine(props: EngineProps) {
  const { view, api, reload, apply, settings, onSelected } = props;
  const credentials = props.credentials ?? { prompt: preferencesBridge.promptCredential };
  const language = settings.transcription.input_language;
  const [choice, setChoice] = useState<'local' | 'cloud' | null>(null);
  const [modelId, setModelId] = useState<string | null>(null);
  const [cloudModel, setCloudModel] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState('');

  // An engine chosen on an earlier launch reopens on its own side of the fork.
  useEffect(() => {
    if (choice === null && view && !view.first_run) {
      setChoice(view.settings.transcription.selection.provider === 'local' ? 'local' : 'cloud');
    }
  }, [view, choice]);

  // A different speech language means a different set of compatible weights, so
  // a size picked by hand for the old one must not survive the change.
  useEffect(() => { setModelId(null); }, [language]);

  const recommendation = useMemo(
    () => (view ? recommendModel(view.models, view.runtime, language) : null),
    [view, language],
  );
  const model = view?.models.find((item) => item.id === (modelId ?? recommendation?.best.id));

  async function run(operation: () => Promise<SetupView | void>) {
    setBusy(true); setProblem('');
    try {
      const next = await operation();
      if (next) apply(next); else reload();
    }
    catch (error) { setProblem(error instanceof Error ? error.message : 'That action did not complete.'); }
    finally { setBusy(false); }
  }

  /**
   * Asks the provider whether the stored key is accepted, on the same terms
   * Settings uses: automatically for a key nothing has asked about yet, and on
   * demand from the button beside it. A stored key reads "not checked" after
   * every restart, so without this the one screen whose job is getting an engine
   * ready could not answer "did my key work" with anything but "I have not
   * looked". It stays weaker than the practice step, which needs a transcript.
   */
  const check = useCallback((provider: CloudProviderId) => {
    setProblem('');
    return api.checkCloud(provider).then(apply, (error: unknown) => {
      setProblem(error instanceof Error ? error.message : 'That key could not be checked.');
    });
  }, [api, apply]);
  const { checking, runCheck } = useKeyChecks(view?.cloud ?? null, check);

  const useEngine = (provider: ProviderId, name: string) => run(async () => {
    const persisted = await api.saveSettings({
      ...settings,
      transcription: { ...settings.transcription, selection: { provider, model: name } },
    });
    onSelected(persisted);
  });

  if (props.loadError) {
    return <div className="ob-step">
      <p className="notice alert" role="alert">{props.loadError}</p>
      <button type="button" className="button" onClick={reload}>Try again</button>
    </div>;
  }
  if (!view) {
    return <div className="ob-step"><p className="notice" role="status">Reading what this computer can run…</p></div>;
  }

  const hardware = view.runtime.hardware;

  return (
    <div className="ob-step">
      <div className="ob-head">
        <span className="ob-glyph"><Cpu size={20} aria-hidden /></span>
        <div>
          <h1>Choose what does the transcribing</h1>
          <p>
            Pick one. A local model runs on this PC and needs a one-time download; a cloud provider
            needs a key and sends your audio through {PRODUCT_NAME}’s backend to that company. You can add
            the other one later in Settings → Speech engine.
          </p>
        </div>
      </div>

      <div className="ob-choice">
        <button type="button" className="ob-option" aria-pressed={choice === 'local'} onClick={() => setChoice('local')}>
          <span className="ob-option-head"><Cpu size={15} aria-hidden /><strong>Run on this PC</strong></span>
          <p>
            Nothing leaves the computer. One download now, then it works offline.
            {hardware && ` This machine reports ${hardware.cpu_threads} CPU threads${hardware.available_ram_bytes !== null ? ` and ${(hardware.available_ram_bytes / 2 ** 30).toFixed(1)} GiB free RAM` : ''}.`}
          </p>
        </button>
        <button type="button" className="ob-option" aria-pressed={choice === 'cloud'} onClick={() => setChoice('cloud')}>
          <span className="ob-option-head"><Cloud size={15} aria-hidden /><strong>Use a cloud provider</strong></span>
          <p>
            No download and no GPU needed. Audio is sent to OpenAI or Gemini, and you need an API
            key of your own. {isMac() ? 'Keys are stored in your macOS login Keychain, never by this window.' : 'Keys are stored by Windows, never by this window.'}
          </p>
        </button>
      </div>

      {choice === 'local' && (
        <div className="ob-card">
          {!model && <p role="status">No model in this catalog supports the selected speech language on this computer. Choose another language, or open Settings → Speech engine.</p>}
          {model && <LocalModel
            model={model}
            reason={modelId && modelId !== recommendation?.best.id ? 'Chosen by you.' : (recommendation?.reason ?? '')}
            recommended={!modelId || modelId === recommendation?.best.id}
            busy={busy}
            selected={view.settings.transcription.selection.provider === 'local' && view.settings.transcription.selection.model === model.id}
            onDownload={() => void run(() => api.modelAction(model.id, 'download'))}
            onCancel={() => void run(() => api.modelAction(model.id, 'cancel'))}
            onUse={() => void useEngine('local', model.id)}
          />}
          {recommendation && (recommendation.lighter || recommendation.stronger) && (
            <div className="ob-sizes">
              <span className="small">Other sizes:</span>
              {[recommendation.best, recommendation.lighter, recommendation.stronger]
                .filter((item): item is ModelView => !!item)
                .map((item) => (
                  <button
                    key={item.id}
                    type="button"
                    className="button small"
                    aria-pressed={item.id === model?.id}
                    disabled={busy}
                    onClick={() => setModelId(item.id)}
                  >
                    {item.display_name} · {item.metadata?.precision.toUpperCase() ?? 'F16'} · {formatBytes(item.bytes)}
                  </button>
                ))}
            </div>
          )}
          {/* The worker line is what the Continue button is waiting on, so it
              gets a surface of its own rather than a fourth line of grey prose. */}
          <p className="notice" role="status">{describeWorker(view)}</p>
          {view.runtime.worker.state === 'failed' && (
            <button type="button" className="button ob-retry" disabled={busy} onClick={() => void run(() => api.runtimeAction('retry'))}>Retry loading</button>
          )}
          <p className="ob-source">The full library, imports and CPU/GPU choice live in Settings → Speech engine.</p>
        </div>
      )}

      {choice === 'cloud' && (
        <div className="ob-card">
          <h3><KeyRound size={13} aria-hidden /> Provider key</h3>
          <p>
            The key is typed into a {isMac() ? 'native macOS' : 'Windows'} dialog and stored for your account only. It never
            reaches this page, and there is no route that would let this page send one.
          </p>
          <div className="ob-providers">
            {view.cloud.filter(transcribes).map((cloud) => {
              const present = cloud.credential !== 'absent' && cloud.credential !== 'unreadable';
              const chosen = cloudModel[cloud.provider] ?? (view.settings.transcription.selection.provider === cloud.provider ? view.settings.transcription.selection.model : cloud.models[0]);
              // The provider's own answer about this key, in the same five
              // states Settings shows, rather than the two this row used to have.
              const state = describeKey(cloud, checking.includes(cloud.provider));
              return (
                <div className="ob-provider" key={cloud.provider}>
                  <div className="ob-provider-name">
                    <strong>{PROVIDER_NAME[cloud.provider]}</strong>
                    {/* One mark, not the two Settings can afford: this row is a
                        third narrower, and the pill already says "No key" when
                        there is none, so a "Key stored" badge beside it only
                        wrapped the column onto a second line. */}
                    <KeyPill state={state} />
                  </div>
                  <div className="ob-provider-controls">
                    <select
                      aria-label={`${PROVIDER_NAME[cloud.provider]} model`}
                      value={chosen}
                      disabled={busy}
                      onChange={(event) => setCloudModel((all) => ({ ...all, [cloud.provider]: event.target.value }))}
                    >
                      {cloud.models.map((item) => <option key={item} value={item}>{item}</option>)}
                    </select>
                    <button type="button" className="button" disabled={busy} onClick={() => void run(async () => { await credentials.prompt(cloud.provider as CloudProvider); })}>
                      {present ? 'Replace key…' : 'Add key…'}
                    </button>
                    <button type="button" className="button" disabled={busy || state.checking || !present} onClick={() => void runCheck(cloud.provider as CloudProviderId)}>
                      Check key
                    </button>
                    <button type="button" className="button primary" disabled={busy || !present} onClick={() => void useEngine(cloud.provider, chosen)}>
                      Use {PROVIDER_NAME[cloud.provider]}
                    </button>
                  </div>
                  <p className="ob-provider-note" role="status" aria-label={`${PROVIDER_NAME[cloud.provider]} key`}>
                    {state.text}
                    {present && !cloud.verified && ' The next step is its first real dictation.'}
                  </p>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {engineReady(view) && (
        <p className="notice" role="status"><ShieldCheck size={13} aria-hidden /> Ready: {view.settings.transcription.selection.model}. The next screens teach the shortcut and try it for real.</p>
      )}
      {problem && <p className="notice alert" role="alert">{problem}</p>}
    </div>
  );
}

function LocalModel({ model, reason, recommended, busy, selected, onDownload, onCancel, onUse }: {
  model: ModelView;
  reason: string;
  recommended: boolean;
  busy: boolean;
  selected: boolean;
  onDownload: () => void;
  onCancel: () => void;
  onUse: () => void;
}) {
  const weights = model.weights;
  const installed = weights.state === 'installed';
  return (
    <>
      <div className="row">
        <h3>{model.display_name}</h3>
        {recommended && <span className="badge accent">Suggested</span>}
        {installed && <span className="badge success">Installed</span>}
      </div>
      <p>{model.notes}</p>
      <p className="ob-meta">
        {formatBytes(model.bytes)} · {model.metadata?.precision.toUpperCase() ?? 'F16'} · {model.input_languages.join(', ')} · {licenseName(model.license)}
        {reason && ` · ${reason}`}
      </p>
      <p className="ob-source">From {model.source_name}, checked against its published SHA-256 before it is ever loaded.</p>
      {model.metadata?.attribution && <p className="ob-source">{model.metadata.attribution}</p>}

      {weights.state === 'downloading' && (
        <>
          <div className="ob-progress"><i style={{ width: `${Math.min(100, Math.round((weights.downloaded_bytes / Math.max(1, weights.total_bytes)) * 100))}%` }} /></div>
          <p className="ob-meta" role="status">Downloading {formatBytes(weights.downloaded_bytes)} of {formatBytes(weights.total_bytes)}. You can leave this screen open.</p>
        </>
      )}
      {weights.state === 'verifying' && (
        <>
          <div className="ob-progress indeterminate"><i /></div>
          <p className="ob-meta" role="status">Checking the downloaded file against its published checksum…</p>
        </>
      )}
      {weights.state === 'failed' && <p className="notice alert" role="alert">{describeWeightsError(weights.error)}</p>}

      <div className="row" style={{ marginTop: 12 }}>
        {(weights.state === 'not_installed' || weights.state === 'failed') && (
          <button type="button" className="button primary" disabled={busy} onClick={onDownload}>
            <Download size={13} aria-hidden />
            {weights.state === 'not_installed' && weights.resumable_bytes
              ? `Resume download (${formatBytes(weights.resumable_bytes)} kept)`
              : `Download ${formatBytes(model.bytes)}`}
          </button>
        )}
        {weights.state === 'downloading' && <button type="button" className="button" disabled={busy} onClick={onCancel}>Cancel download</button>}
        {installed && !selected && <button type="button" className="button primary" disabled={busy} onClick={onUse}>Use this model</button>}
        {installed && selected && <span className="badge success">Selected</span>}
      </div>
    </>
  );
}
