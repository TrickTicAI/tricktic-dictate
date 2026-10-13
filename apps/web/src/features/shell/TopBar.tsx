import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Activity, ArrowDownToLine, BarChart3, BookA, Cpu, Home, Minus, RefreshCw, Search, Settings, Square, X } from 'lucide-react';
import { invoke, isTauri } from '../desktop-session/runtime/native';
import { isMac } from '../settings/platform';
import { Mark } from './Mark';

export const TABS = ['home', 'statistics', 'dictionary'] as const;
export type Tab = typeof TABS[number];

const ITEMS: { id: Tab; label: string; icon: typeof Home }[] = [
  { id: 'home', label: 'Home', icon: Home },
  { id: 'statistics', label: 'Statistics', icon: BarChart3 },
  { id: 'dictionary', label: 'Dictionary', icon: BookA },
];

export interface ConnectionStatus {
  tone: 'online' | 'offline' | 'checking' | 'busy' | 'warn';
  title: string;
  detail: string;
  /** What the primary action does, written on the button. */
  action: string;
}

/** Primary/secondary modifier as people read it on this platform. */
export const modKey = () => (isMac() ? '⌘' : 'Ctrl');

/*
 * The window's one bar: brand, sections, search, status and Settings, drawn as
 * page content so it follows the theme. It replaces the old two stacked bars
 * (a 34 px caption and a 59 px navigation bar).
 *
 * The bar is a Tauri drag region ("deep", so the gaps between controls drag
 * the window too); every control opts out with `data-tauri-drag-region="false"`
 * so pressing it never starts a drag. On macOS the native traffic lights sit
 * over the leading edge, so the bar leaves room for them and draws no window
 * buttons of its own.
 */
export function TopBar({ tab, onTab, settingsOpen, onSettings, status, onStatusAction, onEngineSettings, updateReady, onUpdate, onSearch }: {
  tab: Tab | null;
  onTab: (tab: Tab) => void;
  settingsOpen: boolean;
  onSettings: () => void;
  status: ConnectionStatus;
  onStatusAction: () => void;
  onEngineSettings: () => void;
  /** An update is downloaded and verified. */
  updateReady?: string | null;
  onUpdate?: () => void;
  onSearch?: () => void;
}) {
  const integrated = isTauri();
  const mac = isMac();
  const [popover, setPopover] = useState(false);
  const drag = integrated ? { 'data-tauri-drag-region': 'deep' } : {};
  const control: Record<string, string> = integrated ? { 'data-tauri-drag-region': 'false' } : {};
  return (
    <header className={`titlebar topbar${mac ? ' mac' : ''}`} {...drag}>
      {!mac && <span className="topbar-brand" aria-hidden><Mark size={18} /></span>}
      <Sections tab={settingsOpen ? null : tab} onTab={onTab} control={control} />
      <span className="topbar-spacer" />
      {onSearch && (
        <button type="button" className="topbar-search" onClick={onSearch} aria-keyshortcuts={mac ? 'Meta+K' : 'Control+K'} {...control}>
          <Search size={15} aria-hidden />
          <span>Search dictations, settings, actions</span>
          <span className="kbd" aria-hidden><kbd>{modKey()}</kbd><kbd>K</kbd></span>
        </button>
      )}
      <span className="topbar-spacer" />
      <div className="topbar-tools" {...control}>
        {updateReady && onUpdate && (
          <button type="button" className="update-pill" onClick={onUpdate} title={`Version ${updateReady} is downloaded and checked`}>
            <ArrowDownToLine size={14} aria-hidden /> Update ready
          </button>
        )}
        <span className="status-anchor">
          <button
            type="button"
            className="status-chip"
            data-tone={status.tone}
            aria-expanded={popover}
            aria-haspopup="dialog"
            title={`${status.title} · ${status.detail}`}
            onClick={() => setPopover((open) => !open)}
          >
            <span className={`status-dot ${status.tone}`} aria-hidden />
            <strong>{status.title}</strong>
            <span className="chip-detail">{status.detail}</span>
          </button>
          {popover && (
            <StatusPopover
              status={status}
              onClose={() => setPopover(false)}
              onAction={() => { setPopover(false); onStatusAction(); }}
              onEngineSettings={() => { setPopover(false); onEngineSettings(); }}
              updateReady={updateReady}
              onUpdate={onUpdate && (() => { setPopover(false); onUpdate(); })}
            />
          )}
        </span>
        <button
          type="button"
          className="icon-button topbar-gear"
          aria-label="Settings"
          aria-haspopup="dialog"
          aria-pressed={settingsOpen}
          aria-keyshortcuts={mac ? 'Meta+Comma' : 'Control+Comma'}
          title={`Settings (${modKey()} ,)`}
          onClick={onSettings}
        >
          <Settings size={18} aria-hidden />
        </button>
      </div>
      {integrated && !mac && <WindowButtons />}
    </header>
  );
}

/** The section switcher: one raised thumb slides to the current section. */
function Sections({ tab, onTab, control }: { tab: Tab | null; onTab: (tab: Tab) => void; control: Record<string, string> }) {
  const refs = useRef<Partial<Record<Tab, HTMLButtonElement | null>>>({});
  const [thumb, setThumb] = useState<{ left: number; width: number } | null>(null);
  useLayoutEffect(() => {
    const measure = () => {
      const element = tab ? refs.current[tab] : null;
      setThumb(element ? { left: element.offsetLeft, width: element.offsetWidth } : null);
    };
    measure();
    // Fonts can arrive after the first layout and change every label's width.
    void document.fonts?.ready.then(measure);
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
  }, [tab]);
  return (
    <nav className="sections" aria-label="Sections" {...control}>
      {thumb && <span className="sections-thumb" style={{ insetInlineStart: thumb.left, width: thumb.width }} aria-hidden />}
      {ITEMS.map(({ id, label, icon: Icon }, index) => (
        <button
          key={id}
          ref={(element) => { refs.current[id] = element; }}
          type="button"
          className="nav-item"
          aria-current={tab === id ? 'page' : undefined}
          aria-keyshortcuts={`${isMac() ? 'Meta' : 'Control'}+${index + 1}`}
          onClick={() => onTab(id)}
        >
          <Icon size={15} aria-hidden />
          {label}
        </button>
      ))}
    </nav>
  );
}

/**
 * What the status pill means and what can be done about it, in words. The pill
 * itself only has room for a dot and two words.
 */
function StatusPopover({ status, onClose, onAction, onEngineSettings, updateReady, onUpdate }: {
  status: ConnectionStatus;
  onClose: () => void;
  onAction: () => void;
  onEngineSettings: () => void;
  updateReady?: string | null;
  onUpdate?: () => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    panel.current?.querySelector<HTMLElement>('button')?.focus();
    const away = (event: MouseEvent) => { if (!panel.current?.parentElement?.contains(event.target as Node)) onClose(); };
    const key = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.stopPropagation(); onClose(); } };
    window.addEventListener('mousedown', away);
    window.addEventListener('keydown', key, true);
    return () => { window.removeEventListener('mousedown', away); window.removeEventListener('keydown', key, true); };
  }, [onClose]);
  const reconnect = status.tone === 'offline' || status.tone === 'checking';
  return (
    <div className="popover status-popover" role="dialog" aria-label="Status" ref={panel}>
      <div className="status-popover-head">
        <span className={`status-badge ${status.tone}`} aria-hidden><Activity size={18} /></span>
        <div>
          <strong>{status.title}</strong>
          <p>{status.detail}</p>
        </div>
      </div>
      <div className="popover-items">
        {reconnect
          ? <button type="button" className="popover-item" onClick={onAction}><RefreshCw size={16} aria-hidden /> {status.action}</button>
          : <button type="button" className="popover-item" onClick={onEngineSettings}><Cpu size={16} aria-hidden /> Speech engine settings</button>}
        {updateReady && onUpdate && (
          <button type="button" className="popover-item" onClick={onUpdate}><ArrowDownToLine size={16} aria-hidden /> Version {updateReady} is ready to install</button>
        )}
      </div>
    </div>
  );
}

/**
 * Windows caption buttons, drawn by the page so they follow the theme. They are
 * not in the Tab order: the keyboard path to them is the system menu (Alt+Space).
 */
function WindowButtons() {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    let active = true;
    const read = () => {
      void invoke<{ maximized: boolean }>('window_state')
        .then((state) => { if (active) setMaximized(state.maximized); })
        .catch(() => { /* The buttons still work; only the glyph is stale. */ });
    };
    read();
    window.addEventListener('resize', read);
    return () => { active = false; window.removeEventListener('resize', read); };
  }, []);
  const command = (name: 'minimize' | 'toggle-maximize' | 'close') => () => {
    void invoke<void>('window_command', { command: name })
      .then(() => { if (name === 'toggle-maximize') setMaximized((value) => !value); })
      .catch(() => { /* Nothing useful to say: the window simply did not move. */ });
  };
  return (
    <div className="window-buttons" role="group" aria-label="Window" data-tauri-drag-region="false">
      <button type="button" tabIndex={-1} aria-label="Minimize" onClick={command('minimize')}><Minus size={16} strokeWidth={1.4} /></button>
      <button type="button" tabIndex={-1} aria-label={maximized ? 'Restore' : 'Maximize'} onClick={command('toggle-maximize')}>
        {maximized ? <RestoreGlyph /> : <Square size={12} strokeWidth={1.4} />}
      </button>
      <button type="button" tabIndex={-1} className="close" aria-label="Close" onClick={command('close')}><X size={16} strokeWidth={1.4} /></button>
    </div>
  );
}

/** Windows' restore glyph: two offset squares. */
function RestoreGlyph() {
  return (
    <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.1" aria-hidden>
      <rect x="1.5" y="3.5" width="7" height="7" rx="1" />
      <path d="M3.5 3.5V2.5a1 1 0 0 1 1-1h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-1" />
    </svg>
  );
}

/**
 * The bar for screens that are not the workspace — startup and the first-run
 * guide — with the brand and the window controls only.
 */
export function PlainBar({ title }: { title: string }) {
  const integrated = isTauri();
  const mac = isMac();
  return (
    <header className={`titlebar topbar plain${mac ? ' mac' : ''}`} {...(integrated ? { 'data-tauri-drag-region': 'deep' } : {})}>
      {!mac && <span className="topbar-brand" aria-hidden><Mark size={18} /></span>}
      <span className="topbar-title">{title}</span>
      <span className="topbar-spacer" />
      {integrated && !mac && <WindowButtons />}
    </header>
  );
}
