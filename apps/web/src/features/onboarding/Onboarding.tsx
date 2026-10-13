import { isMac } from '../settings/platform';
import { Accessibility } from '../settings/Accessibility';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import {
  ArrowLeft, ArrowRight, BookA, Check, Cloud, Cpu, HardDriveDownload, Keyboard, Languages,
  Mic, Palette, PanelBottom, Power, Rocket, ShieldCheck, Sliders, Sparkles, Undo2,
} from 'lucide-react';
import type { InputLanguage, Settings } from '@metrix-writer/contracts';
import type { DesktopPreferences } from '@metrix-writer/contracts/desktop';
import type { SessionState } from '../desktop-session/contracts/protocol';
import type { CloudProvider } from '../desktop-preferences/bridge';
import { setupApi, type SetupApi } from '../transcription-setup/api';
import { Toggle } from '../settings/controls';
import { Chord, Keycaps } from './Chord';
import { Engine, engineReady } from './Engine';
import { Practice } from './Practice';
import { Voice } from './Voice';
import { desktopHost, useHotkeySignal, useSetupView, type OnboardingHost, type SystemSettings } from './host';
import './onboarding.css';
import { PRODUCT_NAME } from '../../product';
import { offersSignIn, useAccount } from '../account/account';

const STEPS = [
  { id: 'welcome', label: 'Welcome', icon: Sparkles },
  { id: 'voice', label: 'Your voice', icon: Languages },
  { id: 'engine', label: 'Engine', icon: Cpu },
  { id: 'accessibility', label: 'Accessibility', icon: ShieldCheck },
  { id: 'chord', label: 'Shortcut', icon: Keyboard },
  { id: 'practice', label: 'Try it', icon: Mic },
  { id: 'ready', label: 'Ready', icon: Rocket },
] as const;
export type StepId = typeof STEPS[number]['id'];

/** The live dictation the guide drives, supplied by the workspace that owns it. */
export interface DictationView {
  state: SessionState;
  sessionId: string | null;
  transcript: string | null;
  /** Why dictation is unavailable, or why the last one failed. */
  problem: string | null;
  levels: number[];
  start: () => void;
  stop: () => void;
}

export interface OnboardingProps {
  settings: Settings;
  onSettings: (settings: Settings) => void;
  preferences: DesktopPreferences | null;
  onPreferences: (next: DesktopPreferences) => void;
  preferencesBusy: boolean;
  dictation: DictationView;
  onFinish: () => void;
  host?: OnboardingHost;
  setup?: SetupApi;
  pollMs?: number;
  credentials?: { prompt: (provider: CloudProvider) => Promise<unknown> };
}

/**
 * The first-run guide.
 *
 * It replaces the workspace rather than floating over it, because two of its
 * six screens genuinely gate: an engine has to be ready and one dictation has
 * to have worked. Everything it configures is an ordinary setting that stays
 * reachable afterwards, and the last screen says where each one lives.
 */
export function Onboarding(props: OnboardingProps) {
  const steps = STEPS.filter(item => isMac() || item.id !== 'accessibility');
  const host = props.host ?? desktopHost;
  const { view, error, reload, apply } = useSetupView(props.setup ?? setupApi, props.pollMs);
  const { held } = useHotkeySignal(host);
  const [step, setStep] = useState<StepId>('welcome');
  // The furthest screen the gates have actually let through. The rail navigates
  // within it in both directions, so stepping back to change an answer does not
  // cost you the screens you had already finished.
  const [reached, setReached] = useState(0);
  const [practice, setPractice] = useState<'pending' | 'passed' | 'failed'>('pending');
  const index = steps.findIndex((item) => item.id === step);
  const body = useRef<HTMLDivElement>(null);

  // Every step starts at its top with focus on its heading, so a screen reader
  // announces where you are and the keyboard starts from there.
  useLayoutEffect(() => {
    const element = body.current;
    if (!element) return;
    element.scrollTop = 0;
    const heading = element.querySelector<HTMLElement>('h1');
    if (heading) { heading.tabIndex = -1; heading.focus({ preventScroll: true }); }
  }, [step]);

  /*
   * The first dictation's turnaround, from letting go to text, for the Ready
   * screen: measured here, from the session states the guide already watches.
   */
  const released = useRef<number | null>(null);
  const [first, setFirst] = useState<{ text: string; ms: number | null } | null>(null);
  const dictationState = props.dictation.state;
  useEffect(() => {
    if (step !== 'practice') return;
    if (dictationState === 'capturing' || dictationState === 'requesting') released.current = null;
    else if (['saving', 'saved', 'queued', 'transcribing'].includes(dictationState) && released.current === null) released.current = performance.now();
  }, [dictationState, step]);
  useEffect(() => {
    if (practice !== 'passed' || !props.dictation.transcript) return;
    setFirst((current) => current ?? { text: props.dictation.transcript!, ms: released.current === null ? null : performance.now() - released.current });
  }, [practice, props.dictation.transcript]);

  const ready = engineReady(view);
  const language = props.settings.transcription.input_language;
  // What will actually run, which is the saved document rather than a local
  // draft: the practice step must name the engine it is really about to use.
  const selection = (view?.settings ?? props.settings).transcription.selection;
  const engineLabel = `${selection.model} · ${selection.provider === 'local' ? 'on this PC' : 'cloud'}`;

  // A failed attempt does not open the gate. It only reveals the explicit way
  // past, so continuing past a failure is always a deliberate act.
  const blocked = step === 'engine' ? !ready
    : step === 'practice' ? practice !== 'passed'
    : false;
  const hint = step === 'engine' && !ready
    ? 'Finish the download, or add a key, before continuing. Recording already works either way.'
    : step === 'practice' && practice === 'pending'
      ? 'One dictation has to come back before the workspace opens.'
      : step === 'practice' && practice === 'failed'
        ? 'It did not finish. Try again, or continue and sort it out in Settings → Speech engine.'
        : '';

  const goTo = (position: number) => {
    const next = Math.min(steps.length - 1, Math.max(0, position));
    setReached((furthest) => Math.max(furthest, next));
    setStep(steps[next].id);
  };
  const go = (delta: number) => goTo(index + delta);

  return (
    <div className="ob" data-motion={props.preferences?.reduced_motion ? 'reduced' : 'full'}>
      {/* A centred stepper. Steps already reached are buttons, so stepping back
          to change an answer costs nothing; steps past a gate stay inert. */}
      <nav className="ob-stepper" aria-label="Setup progress">
        <ol>
          {steps.map((item, position) => {
            const state = position === index ? 'current' : position < index ? 'done' : 'todo';
            const mark = <span className="ob-mark">{state === 'done' ? <Check size={12} strokeWidth={3} aria-hidden /> : position + 1}</span>;
            const label = <>{mark}<span className="ob-step-label">{item.label}</span></>;
            return (
              <li key={item.id} data-state={state} aria-current={state === 'current' ? 'step' : undefined}>
                {position <= reached
                  ? <button type="button" className="ob-jump" onClick={() => goTo(position)}>{label}</button>
                  : <span className="ob-jump">{label}</span>}
              </li>
            );
          })}
        </ol>
      </nav>

      <div className="ob-panel">
        <div className="scroller ob-body" ref={body}>
          {step === 'welcome' && <Welcome />}
          {step === 'voice' && <Voice settings={props.settings} onSettings={props.onSettings} disabled={false} />}
          {step === 'engine' && (
            <Engine
              view={view}
              loadError={error}
              api={props.setup ?? setupApi}
              reload={reload}
              apply={apply}
              settings={props.settings}
              onSelected={props.onSettings}
              credentials={props.credentials}
            />
          )}
          {step === 'accessibility' && <div className="ob-step"><h1>Enable dictation and paste</h1><Accessibility /><p>You can skip this step and keep recording with the button or ⌃+⇧+F9.</p></div>}
          {step === 'chord' && (props.preferences
            ? <Chord preferences={props.preferences} onSave={props.onPreferences} busy={props.preferencesBusy} />
            : <p className="notice" role="status">Reading the shell’s shortcut settings…</p>)}
          {step === 'practice' && (
            <Practice
              language={language as InputLanguage}
              engine={engineLabel}
              chord={props.preferences?.recording ?? null}
              toggle={!!props.preferences?.toggle}
              held={held === 'recording'}
              state={props.dictation.state}
              sessionId={props.dictation.sessionId}
              transcript={props.dictation.transcript}
              problem={props.dictation.problem}
              levels={props.dictation.levels}
              onStart={props.dictation.start}
              onStop={props.dictation.stop}
              onOutcome={setPractice}
            />
          )}
          {step === 'ready' && (
            <Ready
              host={host}
              chord={props.preferences?.recording ?? null}
              engine={engineLabel}
              first={first}
              onFinish={props.onFinish}
            />
          )}
        </div>

        <div className="ob-actions">
          {/* Held to the same measure as the screen above it, so Back and
              Continue sit under the content rather than at the window edges. */}
          <div className="ob-actions-inner">
            {index > 0 && (
              <button type="button" className="button" onClick={() => go(-1)}>
                <ArrowLeft size={13} aria-hidden /> Back
              </button>
            )}
            <span className="spacer" />
            {hint && <span className="ob-hint">{hint}</span>}
            {step === 'ready' ? (
              <button type="button" className="button primary" onClick={props.onFinish}>
                Start using {PRODUCT_NAME} <ArrowRight size={13} aria-hidden />
              </button>
            ) : (
              <button type="button" className="button primary" disabled={blocked} onClick={() => go(1)}>
                {step === 'welcome' ? `Set up ${PRODUCT_NAME}` : 'Continue'} <ArrowRight size={13} aria-hidden />
              </button>
            )}
            {step === 'practice' && practice === 'failed' && (
              <button type="button" className="button ghost small" onClick={() => go(1)}>Continue anyway</button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function Welcome() {
  return (
    <div className="ob-step">
      <div className="ob-head">
        <span className="ob-glyph"><Sparkles size={20} aria-hidden /></span>
        <div>
          <h1>{isMac() ? 'Dictate on your Mac. Keep the text.' : `Talk anywhere in Windows. ${PRODUCT_NAME} types it.`}</h1>
          <p>
            Three quick choices and one real dictation. By the end you will have a working model,
            a shortcut you have used, and a transcript you produced yourself.
          </p>
        </div>
      </div>

      <div className="ob-card ob-points">
        <div className="ob-point">
          <Cpu size={16} aria-hidden />
          <div>
            <strong>Your model, your choice</strong>
            <p>Run speech recognition on this computer with a model you download once, or use an OpenAI or Gemini key. Swap between them whenever you like.</p>
          </div>
        </div>
        <div className="ob-point">
          <Languages size={16} aria-hidden />
          <div>
            <strong>English, Hebrew, or both in one sentence</strong>
            <p>The language you speak, the language you get back and the language of this window are three separate settings. Nothing is translated unless you ask.</p>
          </div>
        </div>
        <div className="ob-point">
          <ShieldCheck size={16} aria-hidden />
          <div>
            <strong>Your audio survives everything</strong>
            <p>Recording and transcription are separate steps. If a model fails, times out or is not configured at all, the recording is still on disk and can be retried.</p>
          </div>
        </div>
      </div>
      <p className="ob-source">
        <Undo2 size={12} aria-hidden /> Nothing here is permanent. Every choice has a home in Settings, and this guide can run again from Settings → General.
      </p>
    </div>
  );
}

const MAP: { icon: typeof Mic; title: string; detail: string; where: string }[] = [
  { icon: Keyboard, title: 'Record and cancel', detail: 'Hold or press to dictate anywhere; cancel drops a dictation without transcribing it.', where: 'Settings → Shortcuts & pasting' },
  { icon: Cpu, title: 'Models and keys', detail: 'The whole local library, imports, CPU or GPU, and provider keys.', where: 'Settings → Speech engine' },
  { icon: Sliders, title: 'Dictation modes', detail: 'Regular types what you said. Super rewrites it. Command acts on what you had selected. Each can have its own shortcut.', where: 'Settings → Modes' },
  { icon: BookA, title: 'Dictionary', detail: 'Names the model keeps mishearing, corrections it should always make, and short phrases that expand.', where: 'Dictionary' },
  { icon: PanelBottom, title: 'Floating panel', detail: 'The strip with your level and stage while you dictate. Move it, shrink it or turn it off.', where: 'Settings → Floating panel' },
  { icon: Mic, title: 'Microphone and sounds', detail: 'Which device records, and the start and stop sounds.', where: 'Settings → Microphone & sound' },
  { icon: Palette, title: 'Themes', detail: 'Repaints the whole window, including the floating panel.', where: 'Settings → Appearance' },
  { icon: HardDriveDownload, title: 'Your recordings', detail: 'Every dictation with its audio and every transcription attempt, searchable and retryable.', where: 'Home' },
  { icon: Cloud, title: 'Updates', detail: 'Check for and install signed updates.', where: 'Settings → About & updates' },
  { icon: Power, title: 'Closing and quitting', detail: `Closing the window keeps ${PRODUCT_NAME} running in the tray so the shortcut still works. Quit stops it properly.`, where: 'Tray menu, or Settings → General' },
];

function Ready({ host, chord, engine, first, onFinish }: {
  host: OnboardingHost;
  chord: string | null;
  engine: string;
  /** Your first dictation in the guide: what came back, and how fast. */
  first: { text: string; ms: number | null } | null;
  onFinish: () => void;
}) {
  const [system, setSystem] = useState<SystemSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState('');
  useEffect(() => {
    const refresh = () => void host.system().then(setSystem).catch(() => setProblem('Application settings could not be read.'));
    refresh();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, [host]);

  const save = useCallback(async (next: SystemSettings) => {
    setBusy(true); setProblem('');
    try { await host.saveSystem(next); }
    catch (cause) { setProblem(typeof cause === 'string' ? cause : 'That setting could not be saved.'); }
    finally {
      try { setSystem(await host.system()); } catch { /* Retain the operation error. */ }
      setBusy(false);
    }
  }, [host]);

  return (
    <div className="ob-step">
      <div className="ob-head">
        <span className="ob-glyph"><Rocket size={20} aria-hidden /></span>
        <div>
          <h1>You are set up</h1>
          <p>
            {chord ? <Keycaps chord={chord} /> : 'Your shortcut'} {isMac() ? 'dictates into the focused app with Accessibility enabled; text is also kept in history' : 'dictates into any application'},
            using {engine}. Here is where everything else lives.
          </p>
        </div>
      </div>

      {/* A moment of success: the words you just said, and how fast they came back. */}
      {first && (
        <div className="ob-card ob-first">
          <span className="ob-first-check" aria-hidden><Check size={18} strokeWidth={3} /></span>
          <div>
            <p className="ob-first-text" dir="auto">“{first.text.length > 160 ? `${first.text.slice(0, 157)}…` : first.text}”</p>
            <p>{first.ms !== null ? `Your first dictation came back ${(first.ms / 1000).toFixed(1)} s after you let go.` : 'Your first dictation came back.'} It is kept on Home with its audio.</p>
          </div>
        </div>
      )}

      <div className="ob-card">
        <div className="ob-split">
          <div className="ob-grow">
            <h3>{isMac() ? `Start ${PRODUCT_NAME} when I sign in` : `Start ${PRODUCT_NAME} when I sign in to Windows`} <span className="badge accent">Recommended</span></h3>
            <p>
              {isMac() ? `${PRODUCT_NAME} starts hidden in the menu bar. Approve it in System Settings → General → Login Items & Extensions if asked. You can turn it off there or here.` : `${PRODUCT_NAME} starts hidden in the tray, so the shortcut works the moment you log in without you having to think about it. This adds a per-user startup entry you can remove here or from Windows’ own Startup apps list.`}
            </p>
          </div>
          <Toggle
            checked={system?.launch_at_login ?? false}
            label={isMac() ? `Start ${PRODUCT_NAME} when I sign in` : `Start ${PRODUCT_NAME} when I sign in to Windows`}
            disabled={!system || busy}
            onChange={(launch_at_login) => system && void save({ ...system, launch_at_login })}
          />
        </div>
      </div>

      <AccountChoice onFinish={onFinish} />

      <div className="ob-card">
        <h3>Where everything is</h3>
        <p>All of it is optional. {PRODUCT_NAME} works with what you have already set.</p>
        <div className="ob-map" style={{ marginTop: 12 }}>
          {MAP.map((item) => {
            const Icon = item.icon;
            return (
              <div className="ob-map-item" key={item.title}>
                <Icon size={15} aria-hidden />
                <div>
                  <strong>{item.title}</strong>
                  <p>{isMac() && item.title === 'Dictation modes' ? 'Regular transcribes; Super optionally rewrites. Enable only the app, selection and clipboard context you want a mode to use.' : item.detail}</p>
                  <span className="where">{item.where}</span>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      <p className="ob-source">
        <Undo2 size={12} aria-hidden /> This guide can be reopened any time from Settings → General.
      </p>
      {problem && <p className="notice alert" role="alert">{problem}</p>}
    </div>
  );
}

/**
 * Optional Google sign-in, offered once at the end and never again unasked.
 * Continuing without an account is the default and loses nothing. Public
 * builds, which have no sign-in, show nothing here.
 */
function AccountChoice({ onFinish }: { onFinish: () => void }) {
  const account = useAccount();
  const { snapshot, notice, busy } = account;
  if (!offersSignIn(snapshot) || snapshot?.availability !== 'ready') return null;
  const who = snapshot.display?.name || snapshot.display?.email || 'your Google account';
  return (
    <div className="ob-card">
      <h3>Google account <span className="badge">Optional</span></h3>
      <p>
        {snapshot.state === 'signed_in'
          ? `Signed in as ${who}. Nothing was uploaded; everything stays on this ${isMac() ? 'Mac' : 'PC'}. Manage it in Settings → Account & sync.`
          : snapshot.state === 'signing_in'
            ? 'Finish in the browser tab that opened. Dictation already works while you wait.'
            : `${PRODUCT_NAME} works fully without an account. Signing in with Google uploads nothing; backup and sync will be a separate choice. You can sign in later from Settings → Account & sync.`}
      </p>
      {snapshot.state !== 'signed_in' && (
        <div className="row">
          {snapshot.state === 'signing_in'
            ? <button type="button" className="button" disabled={busy} onClick={account.cancel}>Cancel sign-in</button>
            : <button type="button" className="button" disabled={busy} onClick={account.signIn}>Sign in with Google</button>}
          <button type="button" className="button" onClick={onFinish}>Continue without an account</button>
        </div>
      )}
      {notice && <p className={`notice ${notice.tone === 'alert' ? 'alert' : ''}`} role={notice.tone === 'alert' ? 'alert' : 'status'}>{notice.text}</p>}
    </div>
  );
}
