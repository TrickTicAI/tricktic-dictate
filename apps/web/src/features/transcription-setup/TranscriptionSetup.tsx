import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Check, ChevronRight, Cloud, Cpu, KeyRound, Search } from 'lucide-react';
import type { InputLanguage, ModelSelection, Settings } from '@metrix-writer/contracts';
import { applyLanguagePreset, languageCapabilityIssue, languageCatalog, modelCapabilities } from '../language';
import { Group, Row, Segmented } from '../settings/controls';
import { Problem } from '../shell/Problem';
import { recommendModel } from '../onboarding/recommend';
import { describeKey, providerName, useKeyChecks, KeyPill, type KeyState } from './CloudKey';
import { describeRuntimeProblem, describeWeightsError, describeWorkerError, formatBytes, licenseName, setupApi, transcribes, type CloudProviderId, type CloudView, type ExecutionBackend, type ModelView, type SetupApi, type SetupView } from './api';
import './setup.css';
import { PRODUCT_NAME } from '../../product';

export interface TranscriptionSetupProps {
  api?: SetupApi;
  /** Polling interval while downloads, verification or worker startup are active. */
  pollMs?: number;
  onSettingsSaved?: (settings: Settings) => void;
  /** A hard mismatch the surrounding settings already computed. */
  capabilityIssue?: string;
  /**
   * Opens Settings › API keys for one provider, the only page that adds,
   * replaces, checks or removes a key (MW-SETTINGS-9). Absent in the browser
   * client, which has no key write route.
   */
  onOpenKeys?: (provider: CloudProviderId) => void;
  /**
   * Copies a weights file the user already has into the models directory, where
   * the API verifies it against the published size and checksum. Absent without
   * a native host, which is the only thing that can open a file picker here.
   */
  importWeights?: (model: string) => Promise<void>;
}

const languageLabel: Record<InputLanguage, string> = { en: 'English', he: 'עברית', mixed: 'עברית + English' };
type Side = 'local' | 'cloud';

function active(view: SetupView): boolean {
  return view.models.some(m => m.weights.state === 'downloading' || m.weights.state === 'verifying') || ['starting', 'stopping'].includes(view.runtime.worker.state);
}
function backendLabel(backend: string): string {
  return backend === 'metal' ? 'GPU (Metal)' : backend === 'vulkan' ? 'GPU (Vulkan)' : backend === 'cpu' ? 'CPU' : backend;
}

/**
 * Network-bound settings panel. Recording never depends on this panel; the API
 * owns persistence.
 *
 * The page answers three questions in order: what is transcribing right now,
 * what can be switched to, and everything else. Before this shape it opened on
 * a duplicate speech-language control, put the key buttons in a group below the
 * cards whose status they fix, and offered the model catalog through a dropdown
 * that only chose which single card rendered underneath it — so "which model is
 * selected" had no single answer anywhere on the page.
 *
 * Nothing the panel asserts changed: every runtime, checksum, language and
 * readiness claim is the same sentence it was, moved to where it is legible.
 */
export function TranscriptionSetup({ api = setupApi, pollMs = 2000, onSettingsSaved, capabilityIssue, onOpenKeys, importWeights }: TranscriptionSetupProps) {
  const [view, setView] = useState<SetupView | null>(null);
  const [loadError, setLoadError] = useState('');
  const [actionError, setActionError] = useState('');
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState('');
  const [search, setSearch] = useState('');
  const [precision, setPrecision] = useState('any');
  const [fitsLanguage, setFitsLanguage] = useState(true);
  const [side, setSide] = useState<Side | null>(null);
  /**
   * A speech language chosen here that the selected model cannot serve.
   *
   * The API refuses to store an incompatible pair, so the choice is held until
   * a model that supports it is picked, and both are saved together. Nothing is
   * claimed in the meantime: the strip says the old language is still in force.
   */
  const [pendingLanguage, setPendingLanguage] = useState<InputLanguage | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  /** Null until someone opens or closes the catalog; the default depends on the library. */
  const [showAvailable, setShowAvailable] = useState<boolean | null>(null);
  const mounted = useRef(true);
  const load = useCallback(async () => {
    try {
      const next = await api.load();
      if (!mounted.current) return;
      setView(next); setLoadError('');
    } catch (error) { if (mounted.current) setLoadError((error as Error).message); }
  }, [api]);
  useEffect(() => { mounted.current = true; void load(); return () => { mounted.current = false; }; }, [load]);
  useEffect(() => {
    if (!view) return;
    const timer = setTimeout(() => void load(), active(view) ? pollMs : pollMs * 5);
    return () => clearTimeout(timer);
  }, [view, pollMs, load]);
  // The switcher opens on the side the saved engine is already on, so the page
  // never starts by showing you the half you are not using.
  useEffect(() => {
    if (side === null && view) setSide(view.settings.transcription.selection.provider === 'local' ? 'local' : 'cloud');
  }, [view, side]);

  /** Asks the server to try one stored key against its provider. No key crosses this. */
  const checkKey = useCallback(async (provider: CloudProviderId) => {
    try { const next = await api.checkCloud(provider); if (mounted.current) setView(next); }
    catch (error) { if (mounted.current) setActionError((error as Error).message); }
  }, [api]);
  /*
   * Only the keys this page is currently reporting on: every provider while the
   * cloud side is open, and otherwise just the one transcribing, whose state the
   * strip at the top is claiming.
   */
  const reported = useMemo(() => {
    if (!view) return null;
    return side === 'cloud'
      ? view.cloud.filter(transcribes)
      : view.cloud.filter(item => item.provider === view.settings.transcription.selection.provider);
  }, [view, side]);
  const { checking } = useKeyChecks(reported, checkKey);

  async function run(operation: () => Promise<SetupView>) {
    setBusy(true); setActionError(''); setSaved('');
    try { const next = await operation(); if (mounted.current) setView(next); }
    catch (error) { if (mounted.current) setActionError((error as Error).message); }
    finally { if (mounted.current) setBusy(false); }
  }
  /**
   * The saved document is the only document. The panel used to keep an editable
   * draft because it owned a speech-language control; that control belongs to
   * General, and a draft seeded once on mount went stale the moment the language
   * changed there.
   */
  async function save(settings: Settings, selection: ModelSelection) {
    const next: Settings = { ...settings, transcription: { ...settings.transcription, selection } };
    const issue = languageCapabilityIssue(next.transcription);
    if (issue) { setActionError(issue); return; }
    setBusy(true); setActionError('');
    try {
      const persisted = await api.saveSettings(next);
      if (!mounted.current) return;
      setSaved(`Saved. ${PRODUCT_NAME} reuses this choice on the next launch.`); onSettingsSaved?.(persisted);
      await load();
    } catch (error) { if (mounted.current) setActionError((error as Error).message); }
    finally { if (mounted.current) setBusy(false); }
  }
  /**
   * Applies a speech language immediately when the selected model supports it.
   *
   * This control used to live in General, with only a grey sentence here. That
   * cost a real dictation session: the page showed a model that lists Hebrew
   * while the saved language was English, whisper was told `language=en`, and
   * Hebrew speech came back as English text with nothing on screen connecting
   * the two. The language belongs next to the model it gates.
   */
  async function chooseLanguage(settings: Settings, id: InputLanguage) {
    const next = applyLanguagePreset(settings, id);
    if (languageCapabilityIssue(next.transcription)) { setPendingLanguage(id); setSaved(''); setActionError(''); return; }
    setPendingLanguage(null);
    setBusy(true); setActionError(''); setSaved('');
    try {
      const persisted = await api.saveSettings(next);
      if (!mounted.current) return;
      onSettingsSaved?.(persisted);
      await load();
    } catch (error) { if (mounted.current) setActionError((error as Error).message); }
    finally { if (mounted.current) setBusy(false); }
  }

  /** The native picker owns the file; the API owns whether it is acceptable. */
  async function importModel(model: string) {
    if (!importWeights) return;
    setBusy(true); setActionError(''); setSaved('');
    try {
      await importWeights(model);
      if (!mounted.current) return;
      await load();
      if (mounted.current) setSaved('Import finished. Verify the file to confirm its checksum.');
    } catch (error) { if (mounted.current) setActionError(typeof error === 'string' ? error : (error as Error).message); }
    finally { if (mounted.current) setBusy(false); }
  }

  if (loadError) {
    return <section className="setup-panel">
      <Problem
        title="Transcription setup could not be loaded"
        detail={loadError}
        keeps="Recording still works and keeps your audio."
        retryLabel="Retry loading"
        onRetry={() => void load()}
      />
    </section>;
  }
  if (!view) {
    return <section className="setup-panel">
      <Group title="Transcription setup">
        <Row title="Loading" detail={<span role="status">Loading transcription setup…</span>} />
      </Group>
    </section>;
  }

  const settings = view.settings;
  const saved_language = settings.transcription.input_language;
  // What the list is judged against: the language you just asked for, even when
  // it cannot be stored yet, so the rows answer the question you actually asked.
  const language = pendingLanguage ?? saved_language;
  const execution = view.runtime.execution;
  const hardware = view.runtime.hardware;
  const selectedIssue = languageCapabilityIssue(settings.transcription);
  const languageIssue = selectedIssue
    ? `${selectedIssue} Choose a compatible model or change the speech language.`
    : capabilityIssue;

  const terms = search.toLowerCase().split(/\s+/).filter(Boolean);
  const recommended = recommendModel(view.models, view.runtime, language)?.best.id;
  const matches = view.models.filter(model => {
    if (fitsLanguage && !model.input_languages.includes(language)) return false;
    if (precision !== 'any' && !(model.metadata?.precision ?? 'f16').startsWith(precision)) return false;
    const haystack = `${model.display_name} ${model.id} ${model.metadata?.family ?? ''} ${model.metadata?.precision ?? 'f16'} ${formatBytes(model.bytes)}`.toLowerCase();
    return terms.every(term => haystack.includes(term));
  });
  // Two groups, because "what do I already have" and "what could I get" are
  // different questions, and the first is the shorter list. Largest first
  // inside each, with the fitted recommendation pinned above it.
  const order = (a: ModelView, b: ModelView) =>
    (a.id === recommended ? -1 : 0) - (b.id === recommended ? -1 : 0) || b.bytes - a.bytes;
  const installed = matches.filter(m => m.weights.state === 'installed' || m.weights.state === 'downloading' || m.weights.state === 'verifying').sort(order);
  const available = matches.filter(m => !installed.includes(m)).sort(order);
  /*
   * The catalog opens closed once anything is installed. Every model in the
   * library is a card worth reading when you are choosing one and twenty rows
   * of scrolling when you are not — and everything below this list, including
   * the runtime choice, was on the far side of them.
   */
  const filtering = !!terms.length || precision !== 'any';
  const availableOpen = showAvailable ?? (filtering || !installed.length);
  const modelRow = (model: ModelView) => <LocalModelRow
    key={model.id}
    model={model}
    view={view}
    language={language}
    recommended={model.id === recommended}
    busy={busy}
    api={api}
    run={run}
    expanded={expanded === model.id}
    onExpand={() => setExpanded(current => current === model.id ? null : model.id)}
    onUse={() => void save(applyLanguagePreset(settings, language), { provider: 'local', model: model.id })}
    onImport={importWeights ? () => void importModel(model.id) : undefined}
  />;
  const selectedCloud = view.cloud.find(item => item.provider === settings.transcription.selection.provider);
  const selectedKey = selectedCloud ? describeKey(selectedCloud, checking.includes(selectedCloud.provider)) : null;

  return <section className="setup-panel">
    {view.first_run && (
      <div className="setup-intro">
        <span className="badge">First run</span>
        <p>
          Choose how {PRODUCT_NAME} transcribes. Recording already works and keeps your audio; transcription starts once a
          model is ready.
        </p>
      </div>
    )}

    <ActiveEngine
      view={view}
      language={language}
      savedLanguage={saved_language}
      pending={pendingLanguage !== null}
      languageIssue={languageIssue}
      busy={busy}
      api={api}
      run={run}
      keyState={selectedKey}
      onLanguage={id => void chooseLanguage(settings, id)}
    />

    <section className="settings-group">
      <h3>Change engine</h3>
      <div className="setup-sides">
        <Segmented
          label="Where transcription runs"
          value={side ?? 'local'}
          disabled={busy}
          options={[{ id: 'local' as Side, label: 'Local' }, { id: 'cloud' as Side, label: 'Cloud' }]}
          onChange={setSide}
        />
      </div>

      {side !== 'cloud' ? <>
        <div className="settings-card setup-filters">
          <label className="search-field">
            <Search size={13} aria-hidden />
            <span className="visually-hidden">Find a model</span>
            <input type="search" value={search} onChange={e => setSearch(e.target.value)} placeholder="Name, family or size" />
          </label>
          <label className="field setup-filter-field">
            <span>Precision</span>
            <select aria-label="Precision" value={precision} onChange={e => setPrecision(e.target.value)}>
              <option value="any">Any</option>
              <option value="f16">Full (F16)</option>
              <option value="q8">Compressed (Q8)</option>
              <option value="q5">Compressed (Q5)</option>
            </select>
          </label>
          {/* The filter that matters most here, so it is on by default: a model
              that cannot serve the language above is not a candidate. */}
          <label className="checkbox">
            <input type="checkbox" checked={fitsLanguage} onChange={e => setFitsLanguage(e.target.checked)} />
            Only models that support {languageLabel[language]}
          </label>
        </div>

        {!matches.length && <p className="notice">No models match these filters.</p>}
        {!!installed.length && <div className="setup-section">
          <h4 className="setup-section-head">Installed <span>{installed.length}</span></h4>
          <ul className="setup-models">{installed.map(modelRow)}</ul>
        </div>}
        {!!available.length && <div className="setup-section">
          <h4 className="setup-section-head">
            <button type="button" className="setup-section-toggle" aria-expanded={availableOpen} onClick={() => setShowAvailable(!availableOpen)}>
              <ChevronRight size={13} aria-hidden className="setup-model-chevron" />
              Available to download <span>{available.length}</span>
            </button>
          </h4>
          {availableOpen && <ul className="setup-models">{available.map(modelRow)}</ul>}
        </div>}
        <p className="group-note">
          Downloading a model does not switch to it, and loading one is not a measure of how well it hears you.
          Language quality is evaluated separately from successful loading.
        </p>
      </> : <>
        <ul className="setup-cloud">
          {view.cloud.filter(transcribes).map(cloud => <CloudProviderCard
            key={cloud.provider}
            cloud={cloud}
            settings={settings}
            language={language}
            busy={busy}
            desktop={view.mode === 'desktop'}
            state={describeKey(cloud, checking.includes(cloud.provider))}
            onUse={model => void save(applyLanguagePreset(settings, language), { provider: cloud.provider, model })}
            onOpenKey={onOpenKeys && (() => onOpenKeys(cloud.provider as CloudProviderId))}
          />)}
        </ul>
        <p className="group-note">
          Audio is sent through your {PRODUCT_NAME} backend to the provider. Keys are added, checked and removed in
          Settings › API keys, and stay in the native protected store or the server environment.
        </p>
      </>}
    </section>

    <section className="settings-group">
      <details className="settings-disclosure">
        <summary><ChevronRight size={14} aria-hidden /> Advanced</summary>
        <div className="settings-card">
          {execution && (
            <Row
              title="Processor for speech recognition"
              detail={<>
                Automatic keeps the same model if GPU startup or execution fails. CPU and GPU modes require their
                respective runtimes. CPU runtime: {execution.cpu_available ? 'installed' : 'missing'}. GPU runtime:{' '}
                {execution.gpu_available ? 'installed' : 'missing'}. A runtime file does not prove device readiness.
              </>}
            >
              <select aria-label="Processor for speech recognition" disabled={busy || view.runtime.external_server} value={execution.requested}
                onChange={event => void run(() => api.saveExecution(event.target.value as ExecutionBackend))}>
                <option value="automatic">Automatic (GPU, then CPU)</option>
                <option value="cpu">CPU</option>
                <option value="gpu">GPU (Vulkan required)</option>
              </select>
            </Row>
          )}
          <Row
            title="This computer"
            detail={hardware
              ? `${hardware.architecture} · ${hardware.cpu_threads} CPU threads${hardware.available_ram_bytes !== null ? ` · ${(hardware.available_ram_bytes / 2 ** 30).toFixed(1)} GiB RAM currently available` : ''}`
              : 'Hardware details are unavailable on this platform.'}
          />
          {view.runtime.models_dir && (
            <Row title="Models folder" detail="Where verified weights are kept on this server." stacked>
              <code className="setup-path">{view.runtime.models_dir}</code>
            </Row>
          )}
        </div>
      </details>
    </section>

    {actionError && <p className="notice alert" role="alert">{actionError}</p>}
    {saved && <p className="notice" role="status">{saved}</p>}
  </section>;
}

function describeWorker(view: SetupView): string {
  const worker = view.runtime.worker;
  switch (worker.state) {
    case 'idle': return view.runtime.external_server ? `An explicitly configured external whisper.cpp server is in use; ${PRODUCT_NAME} does not manage it.` : 'No local model is loaded.';
    case 'missing_runtime': return 'Local inference runtime unavailable.';
    case 'missing_weights': return `Weights for ${worker.model} are not installed. Download or import them below.`;
    case 'starting': return `Loading ${worker.model}… (warming up; this can take a minute)`;
    case 'ready': return `${worker.model} is loaded on ${worker.device} (${worker.backend}).${worker.restarts ? ` Recovered after ${worker.restarts} restart${worker.restarts === 1 ? '' : 's'}.` : ''}`;
    case 'failed': return `${worker.model} failed to load.`;
    case 'stopping': return `Unloading ${worker.model}…`;
  }
}

/**
 * The one thing the page has to answer first. Local and cloud engines get the
 * same strip so the answer is in the same place whichever is selected, and the
 * only actions offered are the ones that apply to the engine actually in use.
 */
function ActiveEngine({ view, language, savedLanguage, pending, languageIssue, busy, api, run, keyState, onLanguage }: {
  view: SetupView;
  /** The language the list is being judged against: chosen, even if not yet stored. */
  language: InputLanguage;
  /** The language transcription is actually running with right now. */
  savedLanguage: InputLanguage;
  pending: boolean;
  languageIssue?: string;
  busy: boolean;
  api: SetupApi;
  run: (operation: () => Promise<SetupView>) => Promise<void>;
  /** The key answer for the selected provider; null while a local model is selected. */
  keyState: KeyState | null;
  onLanguage: (id: InputLanguage) => void;
}) {
  const selection = view.settings.transcription.selection;
  const isLocal = selection.provider === 'local';
  const model = view.models.find(m => m.id === selection.model);
  const worker = view.runtime.worker;
  const execution = view.runtime.execution;
  const loadedHere = worker.state === 'ready' && worker.model === selection.model;
  const workerTone = worker.state === 'ready' ? 'ok' : worker.state === 'failed' || worker.state === 'missing_runtime' || worker.state === 'missing_weights' ? 'bad' : 'off';
  const workerChip = worker.state === 'ready' ? 'Loaded' : worker.state === 'starting' ? 'Loading' : worker.state === 'stopping' ? 'Unloading' : worker.state === 'failed' ? 'Failed to load' : worker.state === 'missing_weights' ? 'Weights missing' : worker.state === 'missing_runtime' ? 'Runtime missing' : 'Not loaded';

  return <section className="setup-active" aria-label="Active transcription engine">
    <div className="setup-active-head">
      <span className="setup-active-glyph">{isLocal ? <Cpu size={17} aria-hidden /> : <Cloud size={17} aria-hidden />}</span>
      <div className="setup-active-name">
        <strong>{isLocal ? model?.display_name ?? selection.model : selection.model}</strong>
        <div className="setup-chips">
          {isLocal
            ? <>
              <span className="badge">Local</span>
              {loadedHere && <span className="badge">{backendLabel(worker.backend)}</span>}
              <span className="badge"><span className={`dot ${workerTone}`} aria-hidden />{workerChip}</span>
            </>
            : <>
              <span className="badge">{providerName[selection.provider]}</span>
              <span className="badge">Cloud</span>
              {keyState && <KeyPill state={keyState} />}
            </>}
        </div>
      </div>
      <div className="setup-active-actions">
        {isLocal && worker.state === 'ready' && (
          <button type="button" className="button" disabled={busy} onClick={() => void run(() => api.runtimeAction('stop'))}>Unload model</button>
        )}
        {isLocal && (worker.state === 'failed' || (worker.state === 'idle' && !view.first_run) || worker.state === 'missing_weights') && (
          <button type="button" className="button" disabled={busy} onClick={() => void run(() => api.runtimeAction('retry'))}>Retry loading model</button>
        )}
      </div>
    </div>

    <p className="setup-active-state" role="status" aria-label="Active engine status">
      {isLocal ? describeWorker(view) : keyState?.text}
    </p>
    {isLocal && execution?.fallback_reason && (
      <p className="setup-note" role="status">
        Automatic selected CPU for the same model: {execution.fallback_reason === 'vulkan_runtime_missing'
          ? 'Vulkan runtime is missing.'
          : execution.fallback_reason === 'metal_runtime_missing' ? 'Metal runtime is missing.'
          : describeWorkerError(execution.fallback_reason)}
      </p>
    )}
    {isLocal && worker.state === 'failed' && (
      <p className="setup-error" role="alert">
        {describeWorkerError(worker.error)}
        {worker.restarts > 0 ? ` (${worker.restarts} automatic restart${worker.restarts === 1 ? '' : 's'})` : ''}
      </p>
    )}
    {isLocal && worker.state === 'missing_runtime' && <p className="setup-error" role="alert">{describeRuntimeProblem(worker.detail)}</p>}

    {/* The single most consequential setting for a bilingual user, and the one
        the model list is filtered by. Whisper is told this language outright:
        say Hebrew while this says English and Hebrew comes back as English. */}
    <div className="setup-active-language">
      <span className="setup-language-label">I dictate in</span>
      <Segmented
        label="Speech language"
        value={language}
        disabled={busy}
        options={languageCatalog.presets.map(preset => ({ id: preset.id as InputLanguage, label: preset.label }))}
        onChange={onLanguage}
      />
    </div>
    <p className="setup-language-note">
      {pending
        ? `Still transcribing in ${languageLabel[savedLanguage]}. ${languageLabel[language]} needs a model that supports it — pick one below and both are saved together.`
        : languageCatalog.presets.find(preset => preset.id === language)?.description}
    </p>
    {languageIssue && <p className="setup-error" role="alert">{languageIssue}</p>}
  </section>;
}

interface LocalModelRowProps {
  model: ModelView;
  view: SetupView;
  language: InputLanguage;
  /** The best fit for this machine and language, from the shared recommender. */
  recommended: boolean;
  busy: boolean;
  api: SetupApi;
  run: (operation: () => Promise<SetupView>) => Promise<void>;
  expanded: boolean;
  onExpand: () => void;
  onUse: () => void;
  onImport?: () => void;
}

/**
 * One model, always visible in the list. The collapsed row carries what a
 * choice is made on — name, size, precision, languages, state — and one action
 * whose label is the next real step. Provenance, evidence and the destructive
 * actions live behind the disclosure, where they cannot be clicked by accident
 * and cannot bury the list.
 */
function LocalModelRow({ model, view, language, recommended, busy, api, run, expanded, onExpand, onUse, onImport }: LocalModelRowProps) {
  const [confirmDelete, setConfirmDelete] = useState(false);
  const supports = model.input_languages.includes(language);
  const weights = model.weights;
  const installed = weights.state === 'installed';
  const transferring = weights.state === 'downloading' || weights.state === 'verifying';
  const blocked = !!model.compatibility_issue || !supports;
  const inUse = model.selected && view.settings.transcription.input_language === language;
  const progress = weights.state === 'downloading' && weights.total_bytes
    ? Math.min(100, Math.round((100 * weights.downloaded_bytes) / weights.total_bytes))
    : null;
  /** A state that is actually happening, as opposed to one the section states. */
  const transient = transferring || weights.state === 'failed' || (weights.state === 'not_installed' && !!weights.resumable_bytes);
  const overRam = view.runtime.execution?.requested === 'cpu' && view.runtime.hardware?.available_ram_bytes != null && model.bytes > view.runtime.hardware.available_ram_bytes;

  return <li className={`setup-model ${model.selected ? 'selected' : ''}`} aria-label={model.display_name}>
    <div className="setup-model-main">
      <button type="button" className="setup-model-open" aria-expanded={expanded} onClick={onExpand}>
        <ChevronRight size={14} aria-hidden className="setup-model-chevron" />
        <span className="setup-model-title">
          <span className="setup-model-head">
            <strong>{model.display_name}</strong>
            {model.loaded && <span className="badge"><span className="dot ok" aria-hidden />Loaded</span>}
            {model.selected && <span className="badge">Selected</span>}
            {recommended && !model.selected && <span className="badge">Recommended</span>}
          </span>
          <span className="setup-model-facts">
            {formatBytes(model.bytes)} · {model.metadata?.precision.toUpperCase() ?? 'F16'} ·{' '}
            {model.input_languages.map(item => languageLabel[item]).join(', ')} · from {model.source_name} · {licenseName(model.license)}
          </span>
        </span>
      </button>
      <div className="setup-model-action">
        {weights.state === 'downloading' && <button type="button" className="button" disabled={busy} onClick={() => void run(() => api.modelAction(model.id, 'cancel'))}>Cancel download</button>}
        {weights.state === 'verifying' && <span className="badge">Verifying…</span>}
        {(weights.state === 'not_installed' || weights.state === 'failed') && <button type="button" className="button" disabled={busy} onClick={() => void run(() => api.modelAction(model.id, 'download'))}>
          {weights.resumable_bytes ? `Resume download (${formatBytes(weights.resumable_bytes)} kept)` : `Download ${formatBytes(model.bytes)}`}
        </button>}
        {installed && (inUse
          ? <span className="badge in-use"><Check size={12} aria-hidden />In use</span>
          : <button type="button" className="button primary" disabled={busy || blocked} onClick={onUse}>Use this model</button>)}
      </div>
    </div>

    {/* A row says what is happening to it, not what its section already said.
        "Not installed" under every catalog entry and a verification date under
        every installed one is a line per row that answers nothing and a list
        twice as long to scroll; the evidence moves into the disclosure. */}
    {/* The license requires this credit wherever the model is offered, not only when expanded. */}
    {model.metadata?.attribution && <p className="setup-attribution">{model.metadata.attribution}</p>}
    {transient && <p className="setup-state" role="status" aria-label={`${model.display_name} weights`}>{describeWeights(model)}</p>}
    {progress !== null && <div className="progress" aria-hidden><i style={{ width: `${progress}%` }} /></div>}
    {weights.state === 'failed' && <p className="setup-error" role="alert">{describeWeightsError(weights.error)}</p>}
    {model.compatibility_issue && <p className="setup-error" role="alert">{model.compatibility_issue}</p>}
    {!supports && <p className="setup-warning">Does not support {languageLabel[language]}, so it cannot be selected while you dictate in that language.</p>}
    {overRam && (
      <p className="setup-warning">The weights exceed currently available RAM. CPU loading may be slow or fail; the runtime also needs working memory.</p>
    )}

    {expanded && <div className="setup-model-detail">
      {!transient && <p className="setup-state" role="status" aria-label={`${model.display_name} weights`}>{describeWeights(model)}</p>}
      <p className="setup-notes">{model.notes}</p>
      <p className="setup-source">{formatBytes(model.bytes)} from {model.source_name} · SHA-256 {model.sha256.slice(0, 12)}… · {licenseName(model.license)}</p>

      {model.smoke_tests && <details className="setup-details">
        <summary>Reference sample tests</summary>
        <p>{model.smoke_tests.host} · {model.smoke_tests.date}</p>
        <p>{model.smoke_tests.scope}. Test your own dictation before choosing a model for accuracy.</p>
        <p>Word errors include added, missing, and replaced words.</p>
        <table><thead><tr><th>Language</th><th>Processor</th><th>Time</th><th>Word errors / reference words</th></tr></thead><tbody>
          {model.smoke_tests.tests.map(t => <tr key={`${t.sample}/${t.backend}`}><td>{languageLabel[t.language]}</td><td>{t.backend === 'cpu' ? 'CPU' : 'GPU (Vulkan)'}</td><td>{(t.elapsed_ms / 1000).toFixed(1)} s</td><td>{t.word_errors} / {t.reference_words}</td></tr>)}
        </tbody></table>
      </details>}

      <div className="setup-actions">
        {(weights.state === 'not_installed' || weights.state === 'failed' || installed) && <button type="button" className="button" disabled={busy || transferring} onClick={() => void run(() => api.modelAction(model.id, 'verify'))}>{installed ? 'Verify again' : 'Verify imported file'}</button>}
        {!installed && onImport && <button type="button" className="button" disabled={busy || transferring} onClick={onImport}>Import weights file…</button>}
        {(installed || weights.state === 'failed' || (weights.state === 'not_installed' && !!weights.resumable_bytes)) && <button type="button" className="button danger" disabled={busy || transferring} onClick={() => setConfirmDelete(true)}>Delete model files</button>}
      </div>

      {!installed && <p className="setup-import">{view.mode === 'desktop'
        ? `Already have the weights? Import copies your file, checks its size and SHA-256 against the catalog, refuses anything that does not match, and stores it as ${model.file_name}.`
        : `Already have the weights? Copy them as ${model.file_name} into ${view.runtime.models_dir ?? 'the models directory'} and verify.`}</p>}

      {confirmDelete && <div className="notice alert setup-confirm" role="group" aria-label="Confirm model deletion">
        <p>Delete {PRODUCT_NAME}’s copy of {model.display_name} and any partial download? Saved recordings, transcripts and your original import file are kept. A selected model stays selected and will need downloading again.</p>
        <div className="row">
          <button type="button" className="button" disabled={busy} onClick={() => setConfirmDelete(false)}>Keep model</button>
          <button type="button" className="button danger" disabled={busy} onClick={() => { setConfirmDelete(false); void run(() => api.modelAction(model.id, 'delete')); }}>Confirm deletion</button>
        </div>
      </div>}
    </div>}
  </li>;
}

function describeWeights(model: ModelView): string {
  const weights = model.weights;
  switch (weights.state) {
    case 'not_installed': return weights.resumable_bytes ? `Not installed · ${formatBytes(weights.resumable_bytes)} of a previous download kept` : 'Not installed';
    case 'downloading': return `Downloading ${formatBytes(weights.downloaded_bytes)} of ${formatBytes(weights.total_bytes)}`;
    case 'verifying': return `Verifying SHA-256 of ${formatBytes(weights.bytes)}…`;
    case 'installed': return `Installed and verified (${weights.origin}) on ${new Date(weights.verified_at_ms).toLocaleString()}`;
    case 'failed': return 'Not installed · last attempt failed';
  }
}

interface CloudProviderCardProps {
  cloud: CloudView;
  settings: Settings;
  language: InputLanguage;
  busy: boolean;
  desktop: boolean;
  /** What the provider has said about this key, already resolved for the pill. */
  state: KeyState;
  onUse: (model: string) => void;
  /** Opens API keys on this provider; absent where no page can write a key. */
  onOpenKey?: () => void;
}

/**
 * One provider, with its key state inside it and one step to change the key.
 *
 * The key status and the way to fix it stay on screen together, but the key is
 * changed in Settings › API keys (MW-SETTINGS-9): three pages each carrying
 * their own Add, Replace, Check and Remove made the same key look like three.
 */
function CloudProviderCard({ cloud, settings, language, busy, desktop, state, onUse, onOpenKey }: CloudProviderCardProps) {
  const current = settings.transcription.selection.provider === cloud.provider ? settings.transcription.selection.model : cloud.models[0];
  const [model, setModel] = useState(current);
  const present = cloud.credential !== 'absent' && cloud.credential !== 'unreadable';
  const name = providerName[cloud.provider];
  // The same gate the local rows use. A provider that does not publish this
  // language is refused here rather than selected and rejected on save.
  const supports = modelCapabilities({ provider: cloud.provider, model })?.input_languages.includes(language) ?? false;
  return <li className={`setup-cloud-row ${cloud.selected ? 'selected' : ''}`} aria-label={name}>
    <div className="setup-model-head">
      <KeyRound size={14} aria-hidden className="setup-cloud-glyph" />
      <strong>{name}</strong>
      {present && <span className="badge">Key stored</span>}
      {/* The answer someone came to this card for, beside the key it is about. */}
      <KeyPill state={state} />
      {cloud.selected && <span className="badge">Selected</span>}
    </div>

    <p className="setup-state" role="status" aria-label={`${name} key`}>{state.text}</p>
    {!supports && <p className="setup-warning">{name} does not support {languageLabel[language]} for {model}, so it cannot be selected while you dictate in that language.</p>}

    <div className="setup-actions">
      <label className="field setup-cloud-model">
        <span>{name} model</span>
        <select value={model} disabled={busy} onChange={event => setModel(event.target.value)}>
          {cloud.models.map(item => <option key={item} value={item}>{item}</option>)}
        </select>
      </label>
      {onOpenKey && <button type="button" className={`button ${present ? '' : 'primary'}`} onClick={onOpenKey}>{present ? 'Manage key' : 'Set up key'}</button>}
    </div>

    {/* The commit action gets its own row, so every provider card ends the same
        way however many key actions that provider happens to offer. */}
    <div className="setup-actions setup-cloud-commit">
      <button type="button" className="button primary" disabled={busy || !present || !supports} onClick={() => onUse(model)}>Use {name}</button>
    </div>

    {!onOpenKey && !present && (
      <p className="setup-import">
        {desktop
          ? 'Keys are entered through the native Windows dialog, which this window cannot reach right now.'
          : `This is the self-hosted browser client. Set ${cloud.provider === 'openai' ? 'OPENAI_API_KEY' : cloud.provider === 'grok' ? 'XAI_API_KEY' : 'GEMINI_API_KEY'} in the server environment; there is no route that would let this page write one.`}
      </p>
    )}
  </li>;
}
