import { isMac } from './platform';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '../desktop-session/runtime/native';
import type {
  DesktopPreferences, OverlayPlacement, OverlayThemeMessage, OverlayVisibility, PanelColors,
} from '@metrix-writer/contracts/desktop';
import { BrowserModeNote, Group, Row, Segmented, Toggle } from './controls';
import { PanelPreview } from './PanelPreview';
import { PRODUCT_NAME } from '../../product';

/** Matches the shell's clamp; a value outside it is refused there. */
const SCALE = { min: 0.75, max: 2.5, step: 0.05 };

/** Named colour pairs for the panel. `null` follows the app theme. */
const PRESETS: { id: string; label: string; colors: PanelColors | null }[] = [
  { id: 'theme', label: 'Match theme', colors: null },
  { id: 'white-on-black', label: 'White on black', colors: { background: '#000000', wave: '#ffffff' } },
  { id: 'black-on-white', label: 'Black on white', colors: { background: '#ffffff', wave: '#000000' } },
  { id: 'dracula', label: 'Dracula', colors: { background: '#282a36', wave: '#bd93f9' } },
];
const literalHex = /^#[0-9a-f]{6}$/i;
const sameColors = (a: PanelColors, b: PanelColors) =>
  a.background.toLowerCase() === b.background.toLowerCase() && a.wave.toLowerCase() === b.wave.toLowerCase();
/** Which chip a stored value is. Anything that is not a preset is custom. */
const presetOf = (colors: PanelColors | null) =>
  colors ? PRESETS.find((preset) => preset.colors && sameColors(preset.colors, colors))?.id ?? 'custom' : 'theme';
/** A colour the pickers can hold: `<input type="color">` only takes `#rrggbb`. */
const pickable = (value: string | undefined, fallback: string) =>
  value && /^#[0-9a-f]{6}/i.test(value) ? value.slice(0, 7).toLowerCase() : fallback;

interface SystemSettings { launch_at_login: boolean; sounds: boolean; duck_audio: boolean }

/**
 * The shell's own settings, shown in three places: the floating panel (its own
 * section), the start and stop sounds (Microphone & sound) and how the app
 * runs (General). One component, so the three share one way of saving.
 */
export function System({ part, integrated, preferences, panelTheme, onSave, busy, error, status, onQuit, onReplayGuide }: {
  part: 'panel' | 'sound' | 'app';
  integrated: boolean;
  preferences: DesktopPreferences | null;
  /** The app theme's colours as the panel receives them. The preview paints with them. */
  panelTheme?: OverlayThemeMessage;
  onSave: (next: DesktopPreferences) => void;
  busy: boolean;
  error: string;
  status: string;
  onQuit: () => void;
  onReplayGuide?: () => Promise<void>;
}) {
  const [system, setSystem] = useState<SystemSettings | null>(null);
  /*
   * The size slider and the colour pickers emit a value for every movement. The
   * preview follows each one; only the settled value is written to the shell.
   * Every panel edit is written as the latest known document plus whatever is
   * still waiting to settle, so a pending drag is never undone by a click
   * elsewhere in the group, nor the click by the drag.
   */
  const [scale, setScale] = useState(preferences?.scale ?? 1);
  const [colors, setColors] = useState<PanelColors | null>(preferences?.panel_colors ?? null);
  const [customOpen, setCustomOpen] = useState(() => presetOf(preferences?.panel_colors ?? null) === 'custom');
  const known = useRef(preferences);
  const pending = useRef<Partial<DesktopPreferences>>({});
  const settle = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const save = useRef(onSave);
  useEffect(() => { save.current = onSave; }, [onSave]);
  useEffect(() => {
    known.current = preferences;
    if (!preferences) return;
    // Adopt what the shell holds, except where a value is still being dragged.
    if (!('scale' in pending.current)) setScale(preferences.scale);
    if (!('panel_colors' in pending.current)) {
      const stored = preferences.panel_colors ?? null;
      setColors(stored);
      if (presetOf(stored) === 'custom') setCustomOpen(true);
    }
  }, [preferences]);
  function flush() {
    clearTimeout(settle.current);
    settle.current = undefined;
    const edits = pending.current;
    pending.current = {};
    if (!known.current || !Object.keys(edits).length) return;
    const next = { ...known.current, ...edits };
    // Later edits build on this until the shell's copy comes back.
    known.current = next;
    save.current(next);
  }
  function edit(change: Partial<DesktopPreferences>, settled = false) {
    pending.current = { ...pending.current, ...change };
    clearTimeout(settle.current);
    if (settled) settle.current = setTimeout(flush, 150);
    else flush();
  }
  // Leaving the section mid-drag still writes where the drag ended.
  useEffect(() => () => { if (settle.current) flush(); }, []);

  const [systemBusy, setSystemBusy] = useState(false);
  const [systemError, setSystemError] = useState('');
  useEffect(() => {
    if (!integrated) return;
    const refresh = () => void invoke<SystemSettings>('system_settings_get').then(setSystem)
      .catch(() => setSystemError('Application settings could not be loaded.'));
    refresh();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, [integrated]);
  /*
   * These settings show in two sections (General and Microphone & sound), each
   * with its own copy, so a save starts from what the shell holds now and
   * changes only the one field: a stale copy can never undo the other's edit.
   */
  async function saveSystem(next: SystemSettings) {
    setSystemBusy(true); setSystemError('');
    const changed: keyof SystemSettings = next.launch_at_login !== system?.launch_at_login ? 'launch_at_login' : next.sounds !== system?.sounds ? 'sounds' : 'duck_audio';
    try {
      const current = await invoke<SystemSettings>('system_settings_get').catch(() => system ?? next);
      await invoke<void>('system_settings_save', { settings: { ...current, [changed]: next[changed] }, changed });
    }
    catch (cause) { setSystemError(typeof cause === 'string' ? cause : 'Application settings could not be saved.'); }
    finally {
      try { setSystem(await invoke<SystemSettings>('system_settings_get')); } catch { /* Keep the save error visible. */ }
      setSystemBusy(false);
    }
  }
  if (!integrated) {
    if (part === 'app') return null;
    return <BrowserModeNote what={part === 'panel' ? 'The floating recording panel and its settings' : 'Start and stop sounds'} />;
  }

  if (!preferences) {
    return <p className="notice" role="status">{error || 'Reading the shell’s settings…'}</p>;
  }

  const panelShown = preferences.visibility !== 'off';
  // What "Match theme" paints with, and where the custom pickers start from.
  const themeColors: PanelColors = {
    background: pickable(panelTheme?.bg, '#08090b'),
    wave: pickable(panelTheme?.text, '#f5f5f6'),
  };
  const chosen = customOpen ? 'custom' : presetOf(colors);
  const custom = colors ?? themeColors;
  function choose(id: string) {
    if (id === 'custom') {
      // The pickers open on what the panel shows now; nothing is written until
      // a colour is actually picked.
      setCustomOpen(true);
      if (!colors) setColors(themeColors);
      return;
    }
    const preset = PRESETS.find((option) => option.id === id)!;
    setCustomOpen(false);
    setColors(preset.colors);
    edit({ panel_colors: preset.colors });
  }
  function pick(field: keyof PanelColors, value: string) {
    if (!literalHex.test(value)) return;
    const next = { ...custom, [field]: value.toLowerCase() };
    setColors(next);
    edit({ panel_colors: next }, true);
  }

  return (
    <>
      {part === 'panel' && <Group
        lead={<PanelPreview preferences={{ ...preferences, scale, panel_colors: colors }} theme={panelTheme} />}
        note="Animations follow General → Animations."
      >
        <Row
          title="When to show the panel"
          detail="A small rounded strip with recording levels and brief status while you dictate from other applications. Kept on screen, its record control starts a dictation without a shortcut."
          stacked
        >
          <Segmented
            label="When to show the floating panel"
            value={preferences.visibility}
            disabled={busy}
            options={[
              { id: 'recording' as OverlayVisibility, label: 'While dictating' },
              { id: 'always' as OverlayVisibility, label: 'Always on screen' },
              { id: 'off' as OverlayVisibility, label: 'Never' },
            ]}
            onChange={(visibility) => edit({ visibility })}
          />
        </Row>

        <Row title="Placement" detail="Where it appears, on whichever screen your pointer is on." stacked>
          <Segmented
            label="Panel placement"
            value={preferences.placement}
            disabled={busy || !panelShown}
            options={[
              { id: 'bottom' as OverlayPlacement, label: 'Bottom centre' },
              { id: 'top' as OverlayPlacement, label: 'Top centre' },
              { id: 'remembered' as OverlayPlacement, label: 'Where I dragged it' },
            ]}
            onChange={(placement) => edit({ placement })}
          />
        </Row>

        <Row title="Panel size" detail="Scales the whole panel, contents included.">
          <input
            type="range"
            min={SCALE.min}
            max={SCALE.max}
            step={SCALE.step}
            style={{ width: 160 }}
            aria-label="Panel size"
            value={scale}
            /* Not disabled while saving: a drag emits many values, and each one
             * is written by the shell. The slider stays live and the writes are
             * settled instead. */
            disabled={!panelShown}
            onChange={(event) => {
              const next = Number(event.target.value);
              if (!Number.isFinite(next) || next < SCALE.min || next > SCALE.max) return;
              setScale(next);
              edit({ scale: next }, true);
            }}
          />
          <span className="small faint mono">{Math.round(scale * 100)}%</span>
        </Row>

        <Row
          title="Colours"
          detail="The panel's fill and its voice animation. Match theme uses the theme chosen in Appearance."
          stacked
        >
          <div className="panel-colours">
            <div className="panel-colour-options" role="group" aria-label="Panel colours">
              {[...PRESETS, { id: 'custom', label: 'Custom', colors: null }].map((option) => {
                // Each chip carries a miniature of the pill in its own colours.
                // Custom has none of its own until it is the one in use.
                const swatch = option.id === 'custom'
                  ? chosen === 'custom' ? custom : null
                  : option.colors ?? themeColors;
                return (
                  <button
                    key={option.id}
                    type="button"
                    className="panel-colour"
                    aria-pressed={chosen === option.id}
                    disabled={busy || !panelShown}
                    onClick={() => choose(option.id)}
                  >
                    <span
                      className={`panel-swatch${swatch ? '' : ' any'}`}
                      style={swatch ? { background: swatch.background, borderColor: `${swatch.wave}55` } : undefined}
                      aria-hidden
                    >
                      {[0, 1, 2].map((bar) => <i key={bar} style={swatch ? { background: swatch.wave } : undefined} />)}
                    </span>
                    {option.label}
                  </button>
                );
              })}
            </div>
            {chosen === 'custom' && (
              <div className="panel-colour-fields">
                {/* Not disabled while saving, for the slider's reason: a drag
                    across the picker emits a value per movement. */}
                <label className="panel-colour-field">
                  <input
                    type="color"
                    aria-label="Panel background"
                    value={custom.background}
                    disabled={!panelShown}
                    onChange={(event) => pick('background', event.target.value)}
                  />
                  Panel background
                </label>
                <label className="panel-colour-field">
                  <input
                    type="color"
                    aria-label="Voice animation"
                    value={custom.wave}
                    disabled={!panelShown}
                    onChange={(event) => pick('wave', event.target.value)}
                  />
                  Voice animation
                </label>
              </div>
            )}
          </div>
        </Row>

        <Row title="Dismiss saved-audio reminders automatically" detail="Applies when audio is saved without transcription. Successful dictation and cancellation always hide immediately.">
          <Toggle
            checked={preferences.auto_dismiss}
            label="Dismiss saved-audio reminders automatically"
            disabled={busy || !panelShown}
            onChange={(auto_dismiss) => edit({ auto_dismiss })}
          />
        </Row>

        <Row title="Saved-audio reminder duration" detail="Between 0.3 and 60 seconds.">
          <input
            type="number"
            min={300}
            max={60000}
            step={100}
            style={{ width: 110 }}
            aria-label="Saved-audio reminder in milliseconds"
            value={preferences.completion_hold_ms}
            disabled={busy || !panelShown || !preferences.auto_dismiss}
            onChange={(event) => {
              const value = Number(event.target.value);
              if (Number.isFinite(value) && value >= 300 && value <= 60000) {
                edit({ completion_hold_ms: value });
              }
            }}
          />
          <span className="small faint">ms</span>
        </Row>
      </Group>}

      {part === 'sound' && <Group title="Sound">
        <Row
          title="Dictation and notification sounds"
          detail="Plays a short cue when recording starts and stops."
        >
          <Toggle checked={system?.sounds ?? false} label="Dictation and notification sounds" disabled={!system || systemBusy} onChange={(sounds) => system && void saveSystem({ ...system, sounds })} />
        </Row>

        <Row
          title="Reduce other audio while dictating"
          detail={isMac() ? "Unavailable on macOS: other apps do not expose restorable per-app session volumes. Recording sounds remain available." : "Lowers other applications to 20% of their current volume while recording, then restores them. Your manual volume changes are preserved."}
        >
          <Toggle checked={!isMac() && (system?.duck_audio ?? false)} label="Reduce other audio while dictating" disabled={isMac() || !system || systemBusy} onChange={(duck_audio) => system && void saveSystem({ ...system, duck_audio })} />
        </Row>
      </Group>}

      {part === 'app' && <Group title="Application">
        <Row
          title={`Closing the window keeps ${PRODUCT_NAME} running`}
          detail={`Closing or minimising hides the window; the session stays alive and the global shortcut keeps working. Use Quit below, or the tray menu, to stop ${PRODUCT_NAME} completely.`}
        >
          {/* Behaviour that already exists and is not a stored preference. */}
          <Toggle checked label={`Closing the window keeps ${PRODUCT_NAME} running`} disabled />
        </Row>

        <Row
          title={`Launch ${PRODUCT_NAME} at login`}
          detail={isMac() ? `Starts ${PRODUCT_NAME} hidden in the menu bar when you sign in. Managed by System Settings → General → Login Items & Extensions; return here to refresh its status.` : `Starts ${PRODUCT_NAME} hidden in the tray when you sign in to Windows.`}
        >
          <Toggle checked={system?.launch_at_login ?? false} label={`Launch ${PRODUCT_NAME} at login`} disabled={!system || systemBusy} onChange={(launch_at_login) => system && void saveSystem({ ...system, launch_at_login })} />
        </Row>

        {onReplayGuide && (
          <Row
            title="Run the setup guide again"
            detail="Reopens the first-run walkthrough: language, model or key, the shortcut, and one live dictation. Nothing already configured is reset."
          >
            <button
              type="button"
              className="button"
              disabled={systemBusy}
              onClick={() => {
                setSystemBusy(true); setSystemError('');
                void onReplayGuide()
                  .catch(() => setSystemError('The setup guide could not be reopened.'))
                  .finally(() => setSystemBusy(false));
              }}
            >Run setup guide</button>
          </Row>
        )}

        <Row
          title={`Quit ${PRODUCT_NAME}`}
          detail="Drains any recording in progress and saves it before exiting. The tray menu does the same thing."
        >
          <button type="button" className="button danger" onClick={onQuit}>Quit {PRODUCT_NAME}</button>
        </Row>
      </Group>}

      {systemError && <p className="notice alert" role="alert">{systemError}</p>}
      {error && <p className="notice alert" role="alert">{error}</p>}
      {status && <p className="notice" role="status">{status}</p>}
    </>
  );
}
