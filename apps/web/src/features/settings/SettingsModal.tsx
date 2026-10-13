import { Fragment, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ArrowLeft, AudioLines, ChevronRight, Info, Keyboard, KeyRound, Mic, Palette, PictureInPicture2, Search, ShieldCheck, Sliders, Sparkles, UserRound, Wand2, X } from 'lucide-react';
import type { Settings } from '@metrix-writer/contracts';
import type { DesktopPreferences } from '@metrix-writer/contracts/desktop';
import type { Theme } from '../../themes';
import type { UpdateController } from '../shell/updates';
import { overlayTheme } from '../shell/theme';
import { useParam } from '../shell/urlState';
import { Dialog } from '../shell/Dialog';
import { ModeSettings } from '../modes/ModeSettings';
import { TextModels } from '../modes/TextModels';
import { modeApi, type ModeSettings as ModeDocument } from '../modes/api';
import { nativeCredentials } from '../desktop-preferences/bridge';
import { chordTokens } from '../desktop-preferences/chord';
import { invoke } from '../desktop-session/runtime/native';
import { Animations, General } from './General';
import { Keys } from './Keys';
import { Appearance } from './Appearance';
import { TranscriptionSettings, type ProviderStatus } from './Transcription';
import { Shortcuts } from './Shortcuts';
import { System } from './System';
import { About } from './About';
import { Account, DataPrivacy } from './DataPrivacy';
import { Group } from './controls';
import { Microphone } from './Microphone';
import { PanelLegend } from './PanelLegend';
import { PRODUCT_NAME } from '../../product';
import { harvest, revealSetting, searchSettings, SECTION_LABELS, type Chunk, type SearchResult } from './search';
import { setupApi, type CloudProviderId } from '../transcription-setup/api';
import { hereLabel, shortModel, textModelName } from '../home/language';
import { listMicrophones } from '../recorder/microphones';

export const SECTIONS = ['engine', 'shortcuts', 'microphone', 'panel', 'modes', 'text', 'keys', 'general', 'appearance', 'account', 'data', 'about'] as const;
export type Section = typeof SECTIONS[number];

/**
 * Where earlier section names land: links from before the regrouping, the tray
 * menu of an older shell, and bookmarks. `system` held the panel, the sounds and
 * the app behaviour; the panel is the part people went there for.
 */
const ALIASES: Record<string, Section> = { models: 'engine', transcription: 'engine', system: 'panel' };
export const resolveSection = (name: string | null | undefined): Section =>
  SECTIONS.includes(name as Section) ? name as Section : ALIASES[name ?? ''] ?? 'general';

type Group = 'dictation' | 'writing' | 'app' | 'you';
const GROUPS: { id: Group; label: string }[] = [
  { id: 'dictation', label: 'Dictation' },
  { id: 'writing', label: 'Writing' },
  { id: 'app', label: 'App' },
  { id: 'you', label: 'You' },
];
const NAV: { id: Section; icon: typeof Sliders; group: Group }[] = [
  { id: 'engine', icon: AudioLines, group: 'dictation' },
  { id: 'shortcuts', icon: Keyboard, group: 'dictation' },
  { id: 'microphone', icon: Mic, group: 'dictation' },
  { id: 'panel', icon: PictureInPicture2, group: 'dictation' },
  // A speech model hears you; a text model rewrites what was heard; a mode
  // says which, and how. Keys serve both.
  { id: 'modes', icon: Wand2, group: 'writing' },
  { id: 'text', icon: Sparkles, group: 'writing' },
  { id: 'keys', icon: KeyRound, group: 'writing' },
  { id: 'general', icon: Sliders, group: 'app' },
  { id: 'appearance', icon: Palette, group: 'app' },
  { id: 'account', icon: UserRound, group: 'you' },
  { id: 'data', icon: ShieldCheck, group: 'you' },
  { id: 'about', icon: Info, group: 'you' },
];

/** One sentence under each section's title: what the page is for. */
const PURPOSE: Record<Section, string> = {
  engine: 'The model that turns your voice into text, and where it runs.',
  shortcuts: 'The keys that start and stop dictation, and where the text goes.',
  microphone: 'Which microphone records you, and the sounds around a dictation.',
  panel: 'The small strip that shows a dictation while you work in other apps.',
  modes: 'What happens to your words once they are heard: as spoken, polished, or a command.',
  text: 'The language models that polish your words and run commands.',
  keys: 'Keys for the cloud services your models use. They are typed only into the system dialog.',
  general: 'Languages, motion and how the app runs.',
  appearance: 'One theme for the window, the floating panel and the tray.',
  account: 'Optional: back up and sync your dictionary and settings across devices.',
  data: 'Where your recordings, transcripts and keys are kept, and how to delete them.',
  about: 'Your version, updates and the legal notices.',
};

export interface SettingsModalProps {
  onClose: () => void;
  /** Opens with the search already filled in (from the Ctrl/⌘ K search). */
  initialQuery?: string;
  /** Saved recordings, for live state such as how much is kept on this computer. */
  recordings?: { bytes: number }[];
  version: string;
  integrated: boolean;
  settings: Settings;
  onSettings: (settings: Settings) => void;
  settingsDisabled: boolean;
  providers: ProviderStatus[];
  availabilityReason?: string;
  capabilityIssue?: string;
  theme: Theme;
  themes: Theme[];
  onTheme: (id: string) => void;
  preferences: DesktopPreferences | null;
  onPreferences: (next: DesktopPreferences) => void;
  preferencesBusy: boolean;
  preferencesError: string;
  preferencesStatus: string;
  onQuit: () => void;
  updates?: UpdateController;
  /** Reopens the first-run guide. Absent outside the desktop shell. */
  onReplayGuide?: () => Promise<void>;
}

/**
 * What each section says about itself in the list, read when Settings opens:
 * the model in use, the shortcut, how many keys are set. Anything that needs
 * you is marked, so the list doubles as a status page.
 */
function useLiveState(props: SettingsModalProps): Partial<Record<Section, { text: string; attention?: boolean }>> {
  const [modes, setModes] = useState<ModeDocument | null>(null);
  const [microphone, setMicrophone] = useState<string | null>(null);
  const [sounds, setSounds] = useState<boolean | null>(null);
  const [keys, setKeys] = useState<{ set: number; total: number } | null>(null);
  useEffect(() => {
    let live = true;
    void modeApi.settings().then((value) => { if (live) setModes(value); }).catch(() => { /* The row simply says less. */ });
    // Every cloud provider with a key, text-only ones included (OpenRouter).
    void setupApi.load().then((view) => {
      const cloud = view.cloud.filter((item) => item.provider !== 'local');
      if (live) setKeys({ set: cloud.filter((item) => item.credential === 'environment' || item.credential === 'protected_store').length, total: cloud.length });
    }).catch(() => { /* Falls back to the speech providers below. */ });
    if (props.integrated) {
      void listMicrophones().then((list) => {
        if (live) setMicrophone(list.devices.find((device) => device.id === list.selected_id)?.label ?? 'System default');
      }).catch(() => { /* As above. */ });
      void invoke<{ sounds: boolean }>('system_settings_get').then((value) => { if (live) setSounds(value.sounds); }).catch(() => { /* As above. */ });
    }
    return () => { live = false; };
  }, [props.integrated]);

  const { selection } = props.settings.transcription;
  const provider = props.providers.find((item) => item.id === selection.provider);
  const where = selection.provider === 'local' ? (props.integrated ? hereLabel() : 'on your server') : 'cloud';
  const cloud = props.providers.filter((item) => item.id !== 'local');
  const set = cloud.filter((item) => item.configured).length;
  const active = modes?.profiles.find((profile) => profile.id === modes.active);
  const preferences = props.preferences;
  const kept = props.recordings ?? [];
  const view = props.updates?.view;
  return {
    engine: provider
      ? { text: `${shortModel(selection.model)} · ${where}${provider.available ? '' : ' · not ready'}`, attention: !provider.available }
      : { text: `${shortModel(selection.model)} · ${where}` },
    shortcuts: preferences ? { text: `${chordTokens(preferences.recording).join(' ')} · ${preferences.toggle ? 'press to start' : 'hold to talk'}` } : undefined,
    microphone: microphone ? { text: `${microphone}${sounds === null ? '' : sounds ? ' · sounds on' : ' · sounds off'}` } : undefined,
    panel: preferences ? {
      text: preferences.visibility === 'off' ? 'Turned off'
        : `${preferences.visibility === 'always' ? 'Always on screen' : 'While dictating'} · ${preferences.placement === 'top' ? 'top' : preferences.placement === 'bottom' ? 'bottom' : 'where you left it'}`,
    } : undefined,
    modes: active ? { text: `Default: ${active.name}` } : undefined,
    text: active?.text_model ? { text: textModelName(active.text_model.model) } : undefined,
    keys: keys || cloud.length
      ? { text: `${keys?.set ?? set} of ${keys?.total ?? cloud.length} set`, attention: selection.provider !== 'local' && !provider?.configured }
      : undefined,
    general: { text: `${props.settings.ui_language === 'he' ? 'עברית' : 'English'} interface${preferences?.reduced_motion ? ' · reduced motion' : ''}` },
    appearance: { text: props.theme.name },
    // Sizes come from Storage on the page itself, which measures the disk.
    data: kept.length ? { text: `${kept.length.toLocaleString()} dictation${kept.length === 1 ? '' : 's'} kept` } : undefined,
    about: view?.available
      ? { text: view.stage === 'ready' ? `Update ${view.available} ready` : `Update ${view.available} available`, attention: true }
      : { text: `Version ${props.version}` },
  };
}

export function SettingsModal(props: SettingsModalProps) {
  const [param, setParam] = useParam('settings');
  const section = resolveSection(param);
  /*
   * The section is always written out, including the default one.
   *
   * It used to be dropped when it equalled the default, which is the right
   * behaviour for a tab in a page and the wrong one here: the workspace reads
   * an absent `settings` parameter as "Settings is closed", so choosing General
   * in the list closed the whole dialog instead of opening General.
   */
  const setSection = useCallback((next: Section) => setParam(next), [setParam]);
  const [modeSaving, setModeSaving] = useState(false);
  const [modeDirty, setModeDirty] = useState(false);
  const [modesVisited, setModesVisited] = useState(section === 'modes');
  const [confirmClose, setConfirmClose] = useState(false);
  useEffect(() => { if (section === 'modes') setModesVisited(true); }, [section]);
  const requestClose = useCallback(() => { if (modeSaving) return; if (modeDirty) setConfirmClose(true); else props.onClose(); }, [modeDirty, modeSaving, props.onClose]);
  const panel = useRef<HTMLDivElement>(null);
  const body = useRef<HTMLDivElement>(null);
  const pages = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState(props.initialQuery ?? '');
  const [target, setTarget] = useState<{ title: string } | null>(null);
  const [chunks, setChunks] = useState<Chunk[]>([]);
  const searching = query.trim() !== '';
  const results = searching ? searchSettings(query, chunks) : [];
  const live = useLiveState(props);
  // Sections fill in as their data arrives, so the text is read again on
  // every change while a search is open.
  useEffect(() => {
    const root = pages.current;
    if (!searching || !root) return;
    let frame = 0;
    const read = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(() => setChunks(harvest(root))); };
    read();
    const observer = new MutationObserver(read);
    observer.observe(root, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ['aria-label', 'placeholder', 'title'] });
    return () => { observer.disconnect(); cancelAnimationFrame(frame); };
  }, [searching]);
  const open = useCallback((result: SearchResult) => {
    setQuery('');
    setSection(result.section);
    // A fresh object each time, so choosing the same setting twice still scrolls.
    setTarget({ title: result.title });
  }, [setSection]);
  useEffect(() => (target && body.current ? revealSetting(body.current, target.title) : undefined), [target]);

  /*
   * Keys are added, replaced, checked and removed only in API keys
   * (MW-SETTINGS-9). Other sections link there for one provider, and API keys
   * then offers a way back to the section that sent you, kept mounted so it
   * comes back as you left it, scroll position included. Reaching API keys any
   * other way (the list, search, a link) shows no way back.
   */
  const [keysFrom, setKeysFrom] = useState<Section | null>(null);
  const [keyFocus, setKeyFocus] = useState<{ provider: CloudProviderId } | null>(null);
  const restore = useRef<{ section: Section; scroll: number } | null>(null);
  const openKeys = useCallback((provider?: CloudProviderId) => {
    if (section !== 'keys') {
      setKeysFrom(section);
      restore.current = { section, scroll: body.current?.scrollTop ?? 0 };
    }
    // A fresh object each time, so asking for the same provider again still scrolls to it.
    setKeyFocus(provider ? { provider } : null);
    setSection('keys');
  }, [section, setSection]);
  const backFromKeys = useCallback(() => { if (keysFrom) setSection(keysFrom); }, [keysFrom, setSection]);
  // Forgotten when you leave API keys, by whatever route.
  const onKeys = useRef(section === 'keys');
  useEffect(() => {
    if (onKeys.current && section !== 'keys') { setKeysFrom(null); setKeyFocus(null); }
    onKeys.current = section === 'keys';
  }, [section]);
  useLayoutEffect(() => {
    const back = restore.current;
    // API keys opens at its top, where the provider list starts.
    if (section === 'keys') { if (body.current) body.current.scrollTop = 0; return; }
    restore.current = null;
    // Back from API keys returns to where you were; any other section opens at its top.
    if (body.current) body.current.scrollTop = back?.section === section ? back.scroll : 0;
  }, [section]);
  const updateAvailable = !!props.updates?.view?.available;

  // A section name this build does not know — an old link, or a tray entry from
  // a previous version — resolves to a real one rather than leaving the URL
  // saying something the dialog is not showing.
  useEffect(() => {
    if (param !== null && !SECTIONS.includes(param as Section)) setParam(section);
  }, [param, section, setParam]);

  // `onClose` is a fresh arrow on every parent render, so keying the effect on
  // it re-ran the whole body constantly. Held in a ref instead, the key handler
  // is installed once and still calls the current close.
  const close = useRef(requestClose);
  useEffect(() => { close.current = requestClose; }, [requestClose]);

  // The dialog claims focus when it opens, and only then. This used to run on
  // every render, and the desktop shell re-renders about once a second while
  // the update poll is alive: focus was yanked back to the panel a moment after
  // any click, which shut a native `select` popup before it could be read and
  // dropped keystrokes out of the search fields.
  useEffect(() => { panel.current?.focus(); }, []);

  useEffect(() => {
    function key(event: KeyboardEvent) {
      if (event.key === 'Escape') { event.stopPropagation(); close.current(); return; }
      // A modal dialog has to hold the keyboard as well as the pointer. Escape
      // and the backdrop already closed it, but Tab walked straight out into
      // the page behind, where every control is still reachable and none of it
      // is what the dialog is asking about.
      if (event.key !== 'Tab' || !panel.current) return;
      const focusable = [...panel.current.querySelectorAll<HTMLElement>(
        'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      )].filter((element) => element.offsetParent !== null || element === document.activeElement);
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (event.shiftKey && (active === first || active === panel.current)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && active === last) {
        event.preventDefault();
        first.focus();
      } else if (active instanceof HTMLElement && !panel.current.contains(active)) {
        event.preventDefault();
        first.focus();
      }
    }
    // Capture phase so the shortcut-capture dialog above can stop this first.
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, []);

  const system = (part: 'panel' | 'sound' | 'app') => (
    <System
      part={part}
      integrated={props.integrated}
      preferences={props.preferences}
      panelTheme={overlayTheme(props.theme)}
      onSave={props.onPreferences}
      busy={props.preferencesBusy}
      error={props.preferencesError}
      status={props.preferencesStatus}
      onQuit={props.onQuit}
      onReplayGuide={props.onReplayGuide}
    />
  );

  function page(id: Section) {
    switch (id) {
      case 'general':
        return (
          <>
            <General value={props.settings} onChange={props.onSettings} disabled={props.settingsDisabled} onOpenSection={setSection} onOpenKeys={openKeys} />
            <Animations preferences={props.integrated ? props.preferences : null} onPreferences={props.onPreferences} busy={props.preferencesBusy} />
            {system('app')}
          </>
        );
      case 'appearance':
        return <Appearance theme={props.theme} themes={props.themes} onSelect={props.onTheme} />;
      case 'engine':
        return (
          <TranscriptionSettings
            integrated={props.integrated}
            onSettingsSaved={props.onSettings}
            settings={props.settings}
            onSettings={props.onSettings}
            providers={props.providers}
            disabled={props.settingsDisabled}
            availabilityReason={props.availabilityReason}
            capabilityIssue={props.capabilityIssue}
            onOpenKeys={openKeys}
          />
        );
      case 'microphone':
        return (
          <>
            <Group title="Microphone"><Microphone disabled={props.settingsDisabled} /></Group>
            {system('sound')}
          </>
        );
      case 'panel':
        return (
          <>
            {system('panel')}
            {props.integrated && props.preferences && <PanelLegend />}
          </>
        );
      case 'text':
        return (
          <TextModels
            onOpenSection={setSection}
            // The browser client has no native dialog and no key write route:
            // API keys says so, and this page says it in place.
            onOpenKeys={props.integrated ? openKeys : undefined}
          />
        );
      case 'keys':
        return <Keys
          settings={props.settings}
          credentials={props.integrated ? nativeCredentials : undefined}
          onOpenSection={setSection}
          focus={keyFocus}
        />;
      case 'modes':
        return <ModeSettings onOpenSection={setSection} onOpenKeys={openKeys} onDirtyChange={setModeDirty} onBusyChange={setModeSaving} />;
      case 'shortcuts':
        return (
          <Shortcuts
            integrated={props.integrated}
            preferences={props.preferences}
            onSave={props.onPreferences}
            busy={props.preferencesBusy}
            error={props.preferencesError}
            status={props.preferencesStatus}
          />
        );
      case 'account':
        return <Account integrated={props.integrated} />;
      case 'data':
        return <DataPrivacy integrated={props.integrated} recordings={props.recordings} />;
      case 'about':
        return <About integrated={props.integrated} version={props.version} updates={props.updates} />;
    }
  }

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => { if (event.target === event.currentTarget) requestClose(); }}
    >
      <div className="modal" role="dialog" aria-modal="true" aria-label="Settings" ref={panel} tabIndex={-1}>
        <nav className="modal-nav" aria-label="Settings sections">
          <label className="search-field modal-search">
            <Search size={14} aria-hidden />
            <span className="visually-hidden">Search settings</span>
            <input
              type="search"
              placeholder="Search settings"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                // The first Escape empties the search; the next one closes Settings.
                if (event.key === 'Escape' && query) { event.stopPropagation(); setQuery(''); }
                if (event.key === 'Enter' && results[0]) { event.preventDefault(); open(results[0]); }
                if (event.key === 'ArrowDown' && results[0]) {
                  event.preventDefault();
                  body.current?.querySelector<HTMLButtonElement>('.search-result')?.focus();
                }
              }}
            />
          </label>
          {GROUPS.map((group) => (
            <Fragment key={group.id}>
              <h2>{group.label}</h2>
              {NAV.filter((item) => item.group === group.id).map(({ id, icon: Icon }) => {
                const state = live[id];
                return (
                  <button
                    key={id}
                    type="button"
                    className={state?.attention ? 'attention' : undefined}
                    aria-current={!searching && section === id}
                    // Named by the section alone; its live state is the description.
                    aria-label={SECTION_LABELS[id]}
                    aria-describedby={state ? `nav-state-${id}` : undefined}
                    onClick={() => { setQuery(''); setSection(id); }}
                  >
                    <span className="nav-icon" aria-hidden><Icon size={16} /></span>
                    <span className="nav-text">
                      <span className="nav-label">{SECTION_LABELS[id]}</span>
                      {state && <span className="nav-state" id={`nav-state-${id}`}>{state.text}</span>}
                    </span>
                    {id === 'about' && updateAvailable && <span className="nav-flag" aria-label="Update available" />}
                  </button>
                );
              })}
            </Fragment>
          ))}

          {/* The version doubles as the way into About, so the number you read
              is the control that offers the newer one. */}
          <button type="button" className="modal-version" onClick={() => setSection('about')}>
            {PRODUCT_NAME} {props.version}
            {updateAvailable && <span className="nav-flag" aria-hidden />}
          </button>
        </nav>

        <div className="modal-panel">
          <div className="modal-head">
            <div className="modal-title">
              {/* In the header, not the page, so it is on screen however far down the provider is. */}
              {!searching && section === 'keys' && keysFrom && (
                <button type="button" className="modal-back" aria-label={`Back to ${SECTION_LABELS[keysFrom]}`} onClick={backFromKeys}>
                  <ArrowLeft size={14} aria-hidden /> {SECTION_LABELS[keysFrom]}
                </button>
              )}
              <div>
                <h2>{searching ? 'Search' : SECTION_LABELS[section]}</h2>
                {!searching && <p className="modal-purpose">{PURPOSE[section]}</p>}
              </div>
            </div>
            <button type="button" className="icon-button" aria-label="Close settings" disabled={modeSaving} onClick={requestClose}>
              <X size={18} />
            </button>
          </div>

          {modeDirty && (searching || section !== 'modes') && <div className="notice modal-notice" role="status">You have unsaved mode changes. <button type="button" className="linklike" onClick={() => setSection('modes')}>Return to Modes</button></div>}
          <div className="scroller modal-body" ref={body}>
            {searching && <SearchResults query={query} results={results} onOpen={open} />}
            <div ref={pages} hidden={searching}>
              {SECTIONS.map((id) => {
                // A search mounts every section, out of sight, so it can read
                // all of them. Modes also stays mounted once visited, holding
                // unsaved edits, and a section that opened API keys stays
                // mounted until you go back; the rest mount only while shown
                // or searched.
                const mounted = searching || section === id || (id === 'modes' && modesVisited) || id === keysFrom;
                return mounted && <div key={id} data-section={id} hidden={section !== id}>{page(id)}</div>;
              })}
            </div>
          </div>
        </div>
      </div>
      {confirmClose && <Dialog title="Unsaved mode changes" onClose={() => setConfirmClose(false)}>
        <p>Your mode changes have not been saved.</p>
        <div className="dialog-actions"><button type="button" className="button primary" onClick={() => { setConfirmClose(false); setSection('modes'); }}>Keep editing</button><button type="button" className="button" onClick={props.onClose}>Discard and close</button></div>
      </Dialog>}
    </div>
  );
}

function SearchResults({ query, results, onOpen }: {
  query: string;
  results: SearchResult[];
  onOpen: (result: SearchResult) => void;
}) {
  if (!results.length) {
    return <p className="search-empty" role="status">No settings match “{query.trim()}”.</p>;
  }
  return (
    <>
      <p className="visually-hidden" role="status">{results.length} {results.length === 1 ? 'setting' : 'settings'} found</p>
      <ul className="search-results" aria-label="Matching settings">
        {results.map((result) => (
          <li key={`${result.section}/${result.title}`}>
            <button
              type="button"
              className="search-result"
              onClick={() => onOpen(result)}
              onKeyDown={(event) => {
                // Arrow keys walk the list, the way they left the search field.
                const item = event.currentTarget.parentElement;
                const next = event.key === 'ArrowDown' ? item?.nextElementSibling : event.key === 'ArrowUp' ? item?.previousElementSibling : null;
                const button = next?.querySelector<HTMLButtonElement>('button');
                if (button) { event.preventDefault(); button.focus(); }
                else if (event.key === 'ArrowUp') {
                  event.preventDefault();
                  event.currentTarget.closest('.modal')?.querySelector<HTMLInputElement>('.modal-search input')?.focus();
                }
              }}
            >
              <strong>{result.title || result.sectionLabel}</strong>
              <span className="search-path">
                {result.sectionLabel}
                {result.group && result.group !== result.sectionLabel && <><ChevronRight size={11} aria-hidden />{result.group}</>}
              </span>
              {result.snippet && <span className="search-snippet">{result.snippet}</span>}
            </button>
          </li>
        ))}
      </ul>
    </>
  );
}
