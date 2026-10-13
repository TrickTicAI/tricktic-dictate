import { useCallback, useEffect, useRef, useState } from 'react';
import type { DesktopPreferences, OverlayThemeMessage } from '@metrix-writer/contracts/desktop';

/** The panel window's logical size at scale 1.0, from `preferences.rs`. */
const BASE_WIDTH = 112;
const BASE_HEIGHT = 44;

/*
 * One pass of the demonstration: listening, then the processing dots, then the
 * finished state, which hides the panel unless it is kept on screen. Each phase
 * is long enough to read and short enough that an edit shows up in the phase it
 * affects within a few seconds.
 */
const PHASES = [
  { state: 'capturing', ms: 2600 },
  { state: 'transcribing', ms: 1400 },
  { state: 'success', ms: 1200 },
] as const;
const CYCLE_MS = PHASES.reduce((total, phase) => total + phase.ms, 0);

/* The demonstration's identity. The panel needs one to accept levels for the
   session it is showing; none of it names a real recording. */
const EPOCH = '6b1f2c8e-4d3a-4f5b-9e7c-1a2b3c4d5e6f';
const SESSION = '0f9e8d7c-6b5a-4c3d-8e2f-1a0b9c8d7e6f';
const JOB = '3c2b1a09-8f7e-4d6c-9b5a-4e3d2c1b0a9f';

/** A speech-like microphone level: syllables inside words, short gaps between them. */
function syntheticLevel(seconds: number) {
  const words = Math.abs(Math.sin(seconds * 3.9)) ** 0.4;
  const syllables = 0.6 + 0.4 * Math.abs(Math.sin(seconds * 17.3 + Math.sin(seconds * 5.1)));
  const rms = 0.002 + 0.035 * words * syllables;
  return { rms, peak: Math.min(1, rms * 1.8) };
}

/**
 * The floating panel, running inside Settings.
 *
 * This is the panel's own document — `desktop-overlay.html` in its `?preview`
 * mode — so what is shown is what the panel draws, not a drawing of it. It is
 * sized in the same logical pixels as the panel's window, which makes it the
 * panel's real size on the screen Settings is on. The levels are generated
 * here; nothing opens a microphone.
 */
export function PanelPreview({ preferences, theme }: {
  preferences: DesktopPreferences;
  theme?: OverlayThemeMessage;
}) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [loaded, setLoaded] = useState(false);
  // The panel ignores a snapshot no newer than the last one it drew, so the
  // count outlives any one run of the demonstration.
  const revision = useRef(0);
  const shown = preferences.visibility !== 'off';

  const post = useCallback((name: string, payload: unknown) => {
    frame.current?.contentWindow?.postMessage({ source: 'writer-panel-preview', name, payload }, location.origin);
  }, []);

  // Keyed on content: the desktop shell re-renders Settings about once a
  // second, and each render builds these objects afresh.
  const themeKey = JSON.stringify(theme ?? null);
  const preferencesKey = JSON.stringify(preferences);
  useEffect(() => {
    if (loaded && themeKey !== 'null') post('writer-theme', JSON.parse(themeKey));
  }, [loaded, themeKey, post]);
  useEffect(() => {
    if (loaded) post('writer-preferences', JSON.parse(preferencesKey));
  }, [loaded, preferencesKey, post]);

  useEffect(() => {
    if (!loaded || !shown) return;
    let timer = 0;
    let started = 0;
    let phase = -1;
    let sequence = 0;
    const tick = () => {
      let elapsed = (performance.now() - started) % CYCLE_MS;
      let index = 0;
      while (elapsed >= PHASES[index].ms) { elapsed -= PHASES[index].ms; index += 1; }
      const { state } = PHASES[index];
      if (index !== phase) {
        phase = index;
        sequence = 0;
        revision.current += 1;
        post('writer-session', {
          version: 1, kind: 'snapshot', epoch: EPOCH, revision: revision.current, session_id: SESSION,
          job_id: state === 'capturing' ? null : JOB, state, audio_saved: state !== 'capturing', error_code: null,
        });
      }
      if (state === 'capturing') {
        sequence += 1;
        post('writer-session', {
          version: 1, kind: 'level', epoch: EPOCH, revision: revision.current, session_id: SESSION, sequence,
          ...syntheticLevel(performance.now() / 1000),
        });
      }
    };
    const stop = () => { window.clearInterval(timer); timer = 0; };
    const run = () => { stop(); started = performance.now(); phase = -1; tick(); timer = window.setInterval(tick, 50); };
    // A hidden window has nothing to show; it starts again from listening.
    const visibility = () => { if (document.hidden) stop(); else run(); };
    if (!document.hidden) run();
    document.addEventListener('visibilitychange', visibility);
    return () => { stop(); document.removeEventListener('visibilitychange', visibility); };
  }, [loaded, shown, post]);

  return (
    <figure className="panel-preview">
      <div className="panel-stage" data-placement={preferences.placement}>
        <iframe
          ref={frame}
          src="/desktop-overlay.html?preview"
          title="Floating panel preview"
          aria-hidden
          tabIndex={-1}
          hidden={!shown}
          onLoad={() => setLoaded(true)}
          // The same scheme the panel's document declares in preview mode.
          // Chromium paints an opaque backdrop behind a frame whose scheme
          // differs from its element's, which would square off the pill.
          style={{ width: BASE_WIDTH * preferences.scale, height: BASE_HEIGHT * preferences.scale, colorScheme: 'normal' }}
        />
        {!shown && <p className="panel-stage-note">The panel is turned off.</p>}
        {shown && preferences.placement === 'remembered' && (
          <p className="panel-stage-hint">Wherever you last dragged it</p>
        )}
        <div className="panel-stage-taskbar" aria-hidden><i /><i /><i /><i /><i /></div>
      </div>
      <figcaption>
        Actual size. A demonstration that repeats listening, transcribing and done; it does not use your microphone.
      </figcaption>
    </figure>
  );
}
