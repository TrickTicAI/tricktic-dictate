import { apiFetch, sessionLost, sessionStarted } from './features/auth/session';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { Job, ProviderId, RecordingMetadata, Settings, Vocabulary } from '@metrix-writer/contracts';
import type { DesktopPreferences } from '@metrix-writer/contracts/desktop';
import { useRecorder, type Recording } from './recorder';
import { languageCapabilityIssue, languageCatalog } from './features/language';
import { activeVocabulary, vocabularyApi } from './features/vocabulary';
import { useTranscription } from './features/transcripts/useTranscription';
import { modeApi, type ModeProfile } from './features/modes/api';
import { SessionOwner, type SessionSettings } from './features/desktop-session/runtime/owner';
import { installOwnerBridge } from './features/desktop-session/runtime/bridge';
import { setupApi } from './features/transcription-setup';
import type { Snapshot } from './features/desktop-session/contracts/protocol';
import { invoke, isTauri, listen } from './features/desktop-session/runtime/native';

import { MenuHost } from './features/shell/menu';
import { PlainBar, TopBar, TABS, modKey, type ConnectionStatus, type Tab } from './features/shell/TopBar';
import { CommandPalette, matchesQuery, type PaletteItem } from './features/shell/CommandPalette';
import { BarChart3, BookA, Mic, RefreshCw, Settings as SettingsIcon, Square, Wand2, FileText } from 'lucide-react';
import { SYNONYMS, searchSettings } from './features/settings/search';
import { chordTokens } from './features/desktop-preferences/chord';
import { hereLabel, shortModel } from './features/home/language';
import { setParam, useEnumParam, useParam } from './features/shell/urlState';
import { overlayTheme, useTheme } from './features/shell/theme';
import { usePortableTheme } from './features/account/theme';
import { Statistics } from './features/statistics/Statistics';
import { Home } from './features/home/Home';
import { useLevels } from './features/home/useLevels';
import { Onboarding, useOnboardingGate, type DictationView } from './features/onboarding';
import { Dictionary } from './features/dictionary/Dictionary';
import { SettingsModal } from './features/settings/SettingsModal';
import { preferencesBridge, supportsDesktopPreferences, useDesktopPreferences } from './features/desktop-preferences/bridge';

import desktopConfig from '../../desktop/tauri/tauri.conf.json';
import { useUpdates } from './features/shell/updates';
import { useMotion } from './features/shell/motion';
import { PRODUCT_NAME } from './product';
const VERSION = desktopConfig.version;
const readSettingsSection = () => new URLSearchParams(location.search).get('settings') ?? 'general';
const integrated = isTauri() || location.origin === 'https://writer.metrix.invalid';

let desktopSettings: SessionSettings = { language: 'en', options: null, loading: true };
let desktopOwner: Promise<SessionOwner> | undefined;
function getDesktopOwner() {
  return desktopOwner ??= SessionOwner.create({ settings: () => desktopSettings }).then(owner => {
    installOwnerBridge(owner);
    return owner;
  }).catch(error => { desktopOwner = undefined; throw error; });
}

export function App() { return integrated ? <DesktopApp /> : <Workspace useJobs={useBrowserJobs} />; }

function DesktopApp() {
  const [owner, setOwner] = useState<SessionOwner>();
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  // A launch at sign-in loads this document while the desktop is still busy, so
  // the first attempt can fail for reasons that have already passed. Retrying
  // is what stops one such failure from leaving a window with no interface in
  // it until the user quits and reopens Writer; `getDesktopOwner` clears its own
  // memo on failure, so each attempt really does open the session again.
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    void getDesktopOwner().then(session => { if (!cancelled) setOwner(session); }).catch(() => {
      if (cancelled) return;
      setError(true);
      // Back off to a slow steady retry rather than hammering a busy machine.
      timer = setTimeout(() => setAttempt(next => next + 1), Math.min(1000 * 2 ** attempt, 30_000));
    });
    return () => { cancelled = true; if (timer !== undefined) clearTimeout(timer); };
  }, [attempt]);
  if (!owner) {
    return <div className="shell">
      <PlainBar title={PRODUCT_NAME} />
      <MenuHost />
      <main className="page startup">
        <div className="startup-card" role={error ? 'alert' : 'status'}>
          {error ? <RefreshCw size={22} aria-hidden /> : <span className="spinner motion-steps" style={{ ['--spinner-size' as string]: '22px' }} aria-hidden />}
          <strong>{error ? 'Your saved dictations could not be opened yet' : 'Opening your dictations…'}</strong>
          {error && <p>{PRODUCT_NAME} keeps retrying on its own. If this lasts, check free disk space and your profile's permissions. Nothing has been deleted.</p>}
          {error && <button type="button" className="button primary" onClick={() => setAttempt(next => next + 1)}>Retry now</button>}
        </div>
      </main>
    </div>;
  }
  return <Workspace owner={owner} useJobs={useDesktopJobs} />;
}

function useBrowserJobs(exportAudio: (id: string) => Promise<Blob>, _owner?: SessionOwner) {
  // `snapshot` exists on both shapes so the first-run guide, which only ever
  // runs in the desktop build, can read one session state without the browser
  // build having to grow a session it does not have.
  return { ...useTranscription(exportAudio), sessionBusy: false, snapshot: undefined as Snapshot | undefined };
}

function useDesktopJobs(_exportAudio: (id: string) => Promise<Blob>, owner?: SessionOwner) {
  const session = owner!;
  const [saved, setSaved] = useState(() => Object.fromEntries(session.jobs.list().map(a => [a.id, a])));
  const [snapshot, setSnapshot] = useState<Snapshot>(session.snapshot);
  const [errors, setErrors] = useState<Record<string, string>>({});
  useEffect(() => session.jobs.subscribe(() => setSaved(Object.fromEntries(session.jobs.list().map(a => [a.id, a])))), [session]);
  useEffect(() => session.subscribe(event => { if (event.kind === 'snapshot') setSnapshot(event); }), [session]);
  const failure = snapshot.state === 'error' && snapshot.session_id
    ? { [snapshot.session_id]: `Session failed (${snapshot.error_code}). Saved audio and previous text remain available; retry when ready.` }
    : {};
  const active = ['requesting', 'capturing', 'saving', 'queued', 'transcribing'].includes(snapshot.state);
  return {
    saved, ready: true, sessionBusy: active, snapshot, errors: { ...errors, ...failure },
    working: snapshot.session_id && active ? { [snapshot.session_id]: true } : {},
    transcribe: async (metadata: RecordingMetadata) => {
      const ack = await session.dispatch({ version: 1, kind: 'command', epoch: session.snapshot.epoch, command_id: crypto.randomUUID(), action: 'retry', session_id: metadata.id });
      setErrors(previous => ({ ...previous, [metadata.id]: ack.outcome === 'rejected' ? `Retry unavailable (${ack.error_code}).` : '' }));
    },
    updateJob: (job: Job) => {
      void session.jobs.updateJob(job).catch(() => setErrors(previous => ({ ...previous, [job.request.recording_id]: 'Could not save updated text. Retry after checking storage.' })));
    },
  };
}

type Provider = { id: ProviderId; name: string; configured: boolean; available: boolean; status: string; models?: string[] };
const emptyVocabulary: Vocabulary = { hints: [], corrections: [], snippets: [] };
const defaults: Settings = {
  ui_language: 'en',
  transcription: {
    selection: { provider: 'openai', model: 'gpt-transcribe' },
    input_language: 'en', output_language: { mode: 'preserve' },
    vocabulary: emptyVocabulary, cleanup: { mode: 'off' },
  },
};

function loadSettings(): Settings {
  try {
    const stored = JSON.parse(localStorage.getItem('writer-preferences') || 'null') as Settings | null;
    if (stored && ['en', 'he'].includes(stored.ui_language) && ['en', 'he', 'mixed'].includes(stored.transcription.input_language) &&
      languageCatalog.models.some(model => model.selection.provider === stored.transcription.selection.provider && model.selection.model === stored.transcription.selection.model) &&
      (stored.transcription.output_language.mode === 'preserve' || ['en', 'he'].includes(stored.transcription.output_language.language))) {
      return { ...stored, transcription: { ...stored.transcription, vocabulary: emptyVocabulary, cleanup: { mode: 'off' } } };
    }
  } catch { /* Invalid/unavailable preference storage falls back without touching audio. */ }
  return defaults;
}

function Workspace({ owner, useJobs }: { owner?: SessionOwner; useJobs: typeof useBrowserJobs }) {
  const updates = useUpdates();
  const installedVersion = updates.view?.installed || VERSION;
  const [settings, setSettings] = useState(() => owner ? defaults : loadSettings());
  const [settingsReady, setSettingsReady] = useState(!owner);
  const [initialSettingsSettled, setInitialSettingsSettled] = useState(!owner);
  const [initialVocabularySettled, setInitialVocabularySettled] = useState(false);
  const settingsRevision = useRef(0);
  const [vocabularyReady, setVocabularyReady] = useState(false);
  const [connection, setConnection] = useState<'checking' | 'online' | 'offline'>('checking');
  const [providers, setProviders] = useState<Provider[]>([]);
  const [refresh, setRefresh] = useState(0);
  const [providersReady, setProvidersReady] = useState(false);

  const [tab, setTab] = useEnumParam<Tab>('tab', TABS, 'home');
  const [settingsParam, setSettingsParam] = useParam('settings');
  const settingsOpen = settingsParam !== null;
  const [palette, setPalette] = useState(false);
  const [settingsQuery, setSettingsQuery] = useState('');
  const openSettings = useCallback((section: string, query = '') => { setSettingsQuery(query); setSettingsParam(section); }, [setSettingsParam]);

  const preferences = useDesktopPreferences(supportsDesktopPreferences());
  useMotion(preferences.value?.reduced_motion, supportsDesktopPreferences());

  // Resolved theme colours reach the floating panel over the settings bridge,
  // so an app in Dracula never keeps a beige overlay.
  const pushOverlayTheme = useCallback((theme: Parameters<typeof overlayTheme>[0]) => {
    if (!supportsDesktopPreferences()) return;
    void preferencesBridge.theme(overlayTheme(theme)).catch(() => { /* Panel keeps its last colours. */ });
  }, []);
  const { theme, themes, select: selectTheme } = useTheme(pushOverlayTheme);
  usePortableTheme(theme, selectTheme);

  const capture = useRecorder(settings.transcription.input_language, owner?.recorder);
  const transcription = useJobs(capture.exportRecording, owner);
  const auto = useRef<{ previous: Set<string>; options: Settings['transcription']; recording: boolean; mode: Promise<ModeProfile | undefined> } | null>(null);
  useEffect(() => {
    if (owner) return;
    const lost = () => { auto.current = null; capture.stop(); };
    const started = () => setRefresh(value => value + 1);
    window.addEventListener(sessionLost, lost);
    window.addEventListener(sessionStarted, started);
    return () => { window.removeEventListener(sessionLost, lost); window.removeEventListener(sessionStarted, started); };
  }, [owner, capture.stop]);
  const recording = capture.state === 'recording';
  const busy = capture.state !== 'idle' || transcription.sessionBusy;
  const provider = providers.find(item => item.id === settings.transcription.selection.provider);
  const localModelMismatch = provider?.id === 'local' && !!provider.models?.length && !provider.models.includes(settings.transcription.selection.model);
  const configured = !localModelMismatch && connection === 'online' && !!provider?.configured;
  const usable = configured && (provider?.id !== 'local' || provider.available);
  const capabilityIssue = languageCapabilityIssue(settings.transcription);

  const availabilityReason = owner && !settingsReady
    ? 'Transcription settings are unavailable. Reconnect the backend; recording and saved audio remain available.'
    : localModelMismatch ? `Select the configured local model: ${provider?.models?.join(', ')}.`
    : !provider?.configured
      ? settings.transcription.selection.provider === 'local'
        ? owner
          ? 'Choose and download or import a model in Settings → Transcription. Recording remains available.'
          : 'Configure the local model weights and GPU worker on your server, then reconnect. Recording remains available.'
        : 'No credentials or local model configured. Recording remains available.'
    : provider.id === 'local' && !provider.available
      ? owner
        ? 'The local model is not ready. Settings → Transcription shows the actual model and runtime state.'
        : 'The server model is not ready. Check that its model weights are readable, ffmpeg is installed, and the GPU worker is running, then reconnect.'
    : !provider.available ? 'Configured cloud provider; the first transcription is unverified and can be tried.'
    : undefined;
  // Shown alongside, never instead of, the availability reason.
  const capabilityAlert = capabilityIssue ?? undefined;

  // Tray entries reveal this window and land on the surface they name, so the
  // tray and the sidebar lead to the same places. An unknown settings section
  // falls back to General inside the modal rather than rendering nothing.
  useEffect(() => {
    if (!isTauri()) return;
    const stops: (() => void)[] = [];
    const track = (pending: Promise<() => void>) => {
      void pending.then((unlisten) => { stops.push(unlisten); })
        .catch(() => { /* The tray falls back to the standalone settings window. */ });
    };
    track(listen<string | null>('writer-open-settings', (section) =>
      setSettingsParam(typeof section === 'string' ? section : readSettingsSection())));
    track(listen<string>('writer-open-session', (session) => {
      setSettingsParam(null);
      setTab('home');
      setParam('session', session);
    }));
    return () => stops.forEach((stop) => stop());
  }, [setSettingsParam, setTab]);

  // Interface direction follows `ui_language`. Interface text is not translated
  // yet, so this mirrors the layout rather than claiming a Hebrew interface.
  useEffect(() => {
    document.documentElement.lang = settings.ui_language;
    document.documentElement.dir = settings.ui_language === 'he' ? 'rtl' : 'ltr';
  }, [settings.ui_language]);

  useEffect(() => {
    if (owner) return;
    try { localStorage.setItem('writer-preferences', JSON.stringify({ ...settings, transcription: { ...settings.transcription, vocabulary: emptyVocabulary } })); }
    catch { /* Preferences are a convenience; saved audio is unaffected. */ }
  }, [settings, owner]);

  // Ready settings are immutable per recording. Starts during initialization
  // bind once the first complete settings/provider/vocabulary load arrives.
  desktopSettings = {
    language: settings.transcription.input_language,
    loading: !!owner && (!initialSettingsSettled || !initialVocabularySettled || !providersReady),
    options: owner && settingsReady && vocabularyReady && configured && !capabilityIssue ? settings.transcription : null,
  };

  useEffect(() => { owner?.settingsLoaded(); }, [owner, settings, settingsReady, vocabularyReady, initialSettingsSettled, initialVocabularySettled, providersReady, configured, capabilityIssue]);

  useEffect(() => {
    if (!owner) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      const revision = settingsRevision.current;
      try {
        const view = await setupApi.load();
        if (active && revision === settingsRevision.current) {
          setSettings(previous => ({ ...view.settings, transcription: { ...view.settings.transcription, vocabulary: previous.transcription.vocabulary } }));
          setSettingsReady(true);
        }
      } catch { if (active && revision === settingsRevision.current) setSettingsReady(false); }
      finally { if (active) setInitialSettingsSettled(true); }
      if (active) { setRefresh(v => v + 1); timer = setTimeout(() => void poll(), 3000); }
    };
    void poll();
    return () => { active = false; clearTimeout(timer); };
  }, [owner]);

  useEffect(() => {
    const controller = new AbortController();
    if (!owner) setConnection('checking');
    Promise.all([
      apiFetch('/api/v1/health', { signal: controller.signal }).then(async response => { if (!response.ok || (await response.json()).status !== 'ok') throw new Error('API unavailable'); }),
      apiFetch('/api/v1/providers', { signal: controller.signal }).then(async response => {
        if (!response.ok) throw new Error('Providers unavailable');
        const body = await response.json() as { providers: Provider[] };
        if (!Array.isArray(body.providers)) throw new Error('Invalid providers');
        return body.providers;
      }),
    ]).then(([, values]) => { if (!controller.signal.aborted) { setProviders(values); setProvidersReady(true); setConnection('online'); } })
      .catch(() => { if (!controller.signal.aborted) { setProvidersReady(true); setConnection('offline'); } });
    vocabularyApi.load().then(value => {
      if (!controller.signal.aborted) {
        setSettings(previous => ({ ...previous, transcription: { ...previous.transcription, vocabulary: activeVocabulary(value) } }));
        setVocabularyReady(true); setInitialVocabularySettled(true);
      }
    }).catch(() => { if (!controller.signal.aborted) { setVocabularyReady(false); setInitialVocabularySettled(true); } });
    return () => controller.abort();
  }, [refresh, owner]);

  useEffect(() => {
    if (owner) return;
    const pending = auto.current;
    if (!pending) return;
    if (recording) pending.recording = true;
    if (capture.state !== 'idle' || !pending.recording) return;
    const completed = capture.recordings.find(item => !pending.previous.has(item.id) && item.metadata.capture_state !== 'capturing' && item.bytes > 0);
    if (!completed) return;
    auto.current = null;
    if (usable && transcription.ready && vocabularyReady && !languageCapabilityIssue(pending.options)) {
      void pending.mode.then(profile => transcription.transcribe(completed.metadata, pending.options, profile));
    }
  }, [capture.state, capture.recordings, recording, transcription.ready, transcription.transcribe, usable, vocabularyReady, owner]);

  function start() {
    if (owner) { void capture.start(); return; }
    // Resolve the selected profile at capture start, then persist it with the saved submit before processing.
    const mode = modeApi.settings().then(s => structuredClone(s.profiles.find(p => p.id === s.active))).catch(() => undefined);
    auto.current = { previous: new Set(capture.recordings.map(item => item.id)), options: structuredClone(settings.transcription), recording: false, mode };
    void capture.start();
  }

  function cancel() {
    if (!owner) return;
    const id = owner.snapshot.session_id;
    if (!id) return;
    void owner.dispatch({ version: 1, kind: 'command', epoch: owner.snapshot.epoch, command_id: crypto.randomUUID(), action: 'cancel', session_id: id });
  }

  const applySettings = useCallback((next: Settings) => {
    setSettings(next);
    if (!owner) return;
    settingsRevision.current++;
    void setupApi.saveSettings(next)
      .then(persisted => { setSettings(previous => ({ ...persisted, transcription: { ...persisted.transcription, vocabulary: previous.transcription.vocabulary } })); setSettingsReady(true); })
      .catch(() => setSettingsReady(false));
  }, [owner]);

  // Pressing the chip has to do something the chip named. Offline and checking
  // retry the backend and report it in place; a reachable backend opens the
  // settings section that owns whatever the chip is describing.
  const reconnect = useCallback(() => {
    setConnection('checking');
    setRefresh((value) => value + 1);
  }, []);

  const status: ConnectionStatus = busy
    ? {
      tone: 'busy',
      title: recording ? 'Recording' : 'Working',
      detail: recording ? 'Listening · audio is saved as it is captured' : 'Turning your words into text',
      action: 'Open model settings',
    }
    : connection === 'online'
      ? {
        tone: usable ? 'online' : 'warn',
        title: usable ? 'Ready' : 'Model not ready',
        detail: usable
          ? `${shortModel(settings.transcription.selection.model)} · ${provider?.id === 'local' ? (owner ? hereLabel() : 'on your server') : provider?.name ?? 'cloud'}`
          : owner && configured ? `${PRODUCT_NAME} will try to transcribe. Your recording is kept if it cannot finish.` : 'Recording still works, but nothing will be transcribed.',
        action: 'Open model settings',
      }
      : connection === 'offline'
        ? {
          tone: 'offline',
          title: 'Offline',
          detail: `${PRODUCT_NAME} can't reach its speech service. Recording still works and your audio is safe.`,
          action: 'Reconnect',
        }
        : { tone: 'checking', title: 'Connecting…', detail: `Starting ${PRODUCT_NAME}'s speech service.`, action: 'Check again' };

  const transcribe = useCallback((item: Recording) => {
    void transcription.transcribe(item.metadata, settings.transcription);
  }, [transcription, settings.transcription]);

  // The first-run guide is a desktop surface: global chords, model downloads and
  // launch at login are all shell behaviour the browser client does not have.
  const onboarding = useOnboardingGate(!!owner);
  // Inert unless the guide is showing, so the workspace keeps metering its own.
  const guideLevels = useLevels(onboarding.open && recording, owner, capture.stream);
  const snapshot = transcription.snapshot;
  const practiceJob = snapshot?.session_id ? transcription.saved[snapshot.session_id]?.job : undefined;
  const dictation: DictationView = {
    state: snapshot?.state ?? 'idle',
    sessionId: snapshot?.session_id ?? null,
    transcript: practiceJob?.status.state === 'succeeded' ? practiceJob.status.result.transcript.raw_text : null,
    problem: snapshot?.state === 'error'
      ? (snapshot.session_id && transcription.errors[snapshot.session_id]) || 'The dictation did not finish. Saved audio is kept.'
      : availabilityReason ?? capabilityAlert ?? null,
    levels: guideLevels,
    start,
    stop: capture.stop,
  };

  // Window-wide keys: Ctrl/⌘ K search, Ctrl/⌘ , Settings, Ctrl/⌘ 1–3 sections.
  // Declared before the guide's early return, like every hook here: the number
  // of hooks must not change when the guide opens. The guide owns the keyboard
  // while it is open, so the keys do nothing then.
  const guideOpen = onboarding.open;
  useEffect(() => {
    if (guideOpen) return;
    function key(event: KeyboardEvent) {
      const primary = event.metaKey || event.ctrlKey;
      if (!primary || event.altKey) return;
      const name = event.key.toLowerCase();
      if (name === 'k') { event.preventDefault(); setPalette((open) => !open); }
      else if (name === ',') { event.preventDefault(); openSettings(settingsParam ?? 'general'); }
      else if (['1', '2', '3'].includes(name) && !event.shiftKey) {
        event.preventDefault();
        setSettingsParam(null);
        setTab(TABS[Number(name) - 1]);
      }
    }
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [guideOpen, openSettings, settingsParam, setSettingsParam, setTab]);

  if (onboarding.open) {
    return (
      <div className="shell">
        <PlainBar title={PRODUCT_NAME} />
        <MenuHost />
        <Onboarding
          settings={settings}
          onSettings={applySettings}
          preferences={preferences.value}
          onPreferences={(next: DesktopPreferences) => void preferences.save(next)}
          preferencesBusy={preferences.busy}
          dictation={dictation}
          onFinish={onboarding.finish}
        />
      </div>
    );
  }

  // Search everything (Ctrl/⌘ K): actions, settings and what was said.
  const paletteItems = (query: string): PaletteItem[] => {
    const mod = modKey();
    const actions: PaletteItem[] = ([
      recording
        ? { id: 'stop', group: 'Actions', label: 'Stop dictation', icon: Square, keys: preferences.value ? chordTokens(preferences.value.recording) : undefined, run: capture.stop }
        : { id: 'start', group: 'Actions', label: 'Start dictation', icon: Mic, keywords: 'record dictate speak', keys: preferences.value ? chordTokens(preferences.value.recording) : undefined, run: () => { setTab('home'); start(); } },
      { id: 'home', group: 'Actions', label: 'Go to Home', icon: FileText, keywords: 'history dictations', keys: [mod, '1'], run: () => { setSettingsParam(null); setTab('home'); } },
      { id: 'statistics', group: 'Actions', label: 'Go to Statistics', icon: BarChart3, keywords: 'stats numbers words', keys: [mod, '2'], run: () => { setSettingsParam(null); setTab('statistics'); } },
      { id: 'dictionary', group: 'Actions', label: 'Go to Dictionary', icon: BookA, keywords: 'words corrections snippets vocabulary', keys: [mod, '3'], run: () => { setSettingsParam(null); setTab('dictionary'); } },
      { id: 'settings', group: 'Actions', label: 'Open Settings', icon: SettingsIcon, keywords: 'preferences options', keys: [mod, ','], run: () => openSettings('general') },
      ...(owner ? [{ id: 'guide', group: 'Actions' as const, label: 'Run the first-run guide again', icon: Wand2, keywords: 'onboarding setup tutorial', run: () => { void onboarding.replay(); } }] : []),
      ...(updates.enabled ? [{ id: 'updates', group: 'Actions' as const, label: 'Check for updates', icon: RefreshCw, keywords: 'version upgrade', run: () => { void updates.action('updates_check'); openSettings('about'); } }] : []),
    ] as PaletteItem[]).filter((item) => matchesQuery(query, item.label, item.keywords));
    const settingsResults: PaletteItem[] = query
      ? searchSettings(query, SYNONYMS.map((entry) => ({ section: entry.section, title: entry.title, texts: [entry.keywords] }))).map((result) => ({
        id: `setting-${result.section}-${result.title}`,
        group: 'Settings' as const,
        label: result.title,
        detail: result.sectionLabel,
        icon: SettingsIcon,
        run: () => openSettings(result.section, query),
      }))
      : [];
    const said: PaletteItem[] = query
      ? capture.recordings.flatMap((item) => {
        const job = transcription.saved[item.id]?.job;
        const text = job?.status.state === 'succeeded' ? job.status.result.transcript.raw_text : '';
        if (!text || !matchesQuery(query, text)) return [];
        return [{
          id: `said-${item.id}`,
          group: 'Dictations' as const,
          label: text.length > 90 ? `${text.slice(0, 90)}…` : text,
          detail: item.createdAt.toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' }),
          icon: FileText,
          dir: item.language === 'he' ? 'rtl' as const : item.language === 'mixed' ? 'auto' as const : 'ltr' as const,
          run: () => { setSettingsParam(null); setTab('home'); setParam('session', item.id); },
        }];
      })
      : [];
    return [...actions, ...settingsResults, ...said];
  };


  return (
    <div className="shell">
      <TopBar
        tab={tab}
        onTab={(next) => { setTab(next); setSettingsParam(null); }}
        onSettings={() => (settingsOpen ? setSettingsParam(null) : openSettings('general'))}
        settingsOpen={settingsOpen}
        status={status}
        onStatusAction={reconnect}
        onEngineSettings={() => openSettings('engine')}
        updateReady={updates.view?.stage === 'ready' ? updates.view.available ?? 'new' : null}
        onUpdate={() => openSettings('about')}
        onSearch={() => setPalette(true)}
      />
      <MenuHost />
      <div className="shell-body">
        {tab === 'home' && (
          <Home
            capture={{ ...capture, start, stop: capture.stop, select: (id) => void capture.select(id) }}
            jobs={{ ...transcription, transcribe }}
            settings={settings}
            integrated={!!owner}
            owner={owner}
            busy={busy}
            usable={usable && transcription.ready && vocabularyReady && !capabilityIssue}
            unavailableReason={availabilityReason}
            capabilityIssue={capabilityAlert}
            shortcut={preferences.value?.recording ?? null}
            onCancel={owner && busy ? cancel : undefined}
            onOpenModels={() => openSettings('engine')}
            onOpenModes={() => openSettings('modes')}
            onSettings={applySettings}
            providers={providers}
          />
        )}
        {tab === 'statistics' && <Statistics recordings={capture.recordings} saved={transcription.saved} loading={capture.loading || !transcription.ready} onOpenSession={id => { setParam('session', id); setTab('home'); }} onOpenSettings={(section) => openSettings(section)} onOpenHistory={(filter) => { setParam('filter', filter); setTab('home'); }} />}
        {tab === 'dictionary' && (
          <Dictionary onSaved={(value) => {
            setSettings(previous => ({ ...previous, transcription: { ...previous.transcription, vocabulary: activeVocabulary(value) } }));
            setVocabularyReady(true); setInitialVocabularySettled(true);
          }} />
        )}
      </div>

      {palette && <CommandPalette onClose={() => setPalette(false)} items={paletteItems} />}

      {settingsOpen && (
        <SettingsModal
          onClose={() => { setSettingsParam(null); setSettingsQuery(''); }}
          initialQuery={settingsQuery}
          version={installedVersion}
          integrated={!!owner}
          settings={settings}
          onSettings={applySettings}
          settingsDisabled={busy}
          providers={providers}
          availabilityReason={availabilityReason}
          capabilityIssue={capabilityAlert}
          theme={theme}
          themes={themes}
          onTheme={selectTheme}
          preferences={preferences.value}
          onPreferences={(next: DesktopPreferences) => void preferences.save(next)}
          preferencesBusy={preferences.busy}
          preferencesError={preferences.error}
          preferencesStatus={preferences.status}
          onQuit={() => void invoke<void>('request_quit_command').catch(() => { /* The tray menu still quits. */ })}
          updates={updates}
          recordings={capture.recordings}
          // Close Settings first, so finishing the guide lands on the workspace
          // rather than reopening the panel it was started from.
          onReplayGuide={owner ? async () => { setSettingsParam(null); await onboarding.replay(); } : undefined}
        />
      )}
    </div>
  );
}
