import { useCallback, useRef, useState, type ReactNode } from 'react';
import { Check, ChevronDown, Cloud, Command as CommandIcon, Cpu, Mic, Settings2, Sparkles, Square, Trash2 } from 'lucide-react';
import type { InputLanguage, ModelSelection, Settings } from '@metrix-writer/contracts';
import { formatDuration } from '../../recorder';
import { applyLanguagePreset, languageCatalog, modelCapabilities } from '../language';
import type { ModeKind, ModeProfile, ModeSettings } from '../modes/api';
import { Keys } from '../shell/Keys';
import { hasProviderLogo, ProviderLogo } from '../shell/ProviderLogo';
import { usePopover } from '../shell/usePopover';
import { hereLabel, modelName, shortModel, textModelName } from './language';
import type { Flow } from './useDictationFlow';

export interface DockProvider { id: string; name: string; configured: boolean; available: boolean; models?: string[] }

export const modeIcon = (kind: ModeKind | undefined) => kind === 'super' ? Sparkles : kind === 'command' ? CommandIcon : Mic;

const LANGUAGE_CODE: Record<InputLanguage, string> = { en: 'EN', he: 'HE', mixed: 'MIX' };
export const languageCode = (language: InputLanguage) => LANGUAGE_CODE[language];

/** Three dots that keep pulsing while something is under way. */
export function Liveness() {
  return <span className="liveness motion-steps" aria-hidden><i /><i /><i /></span>;
}

/**
 * What happens between letting go and the text arriving, as steps: recognise,
 * then whatever the mode does, then paste. The browser build never pastes.
 * The session reports recognition and "processing the text" (the mode and the
 * paste together), so the second step is the one shown as current for both.
 */
export function Steps({ flow, kind, textModel, deliver, long }: {
  flow: Flow;
  kind?: ModeKind;
  textModel?: string | null;
  deliver: boolean;
  long?: boolean;
}) {
  const steps = [long ? 'Transcribing' : 'Transcribe'];
  if (kind === 'super') steps.push(long ? (textModel ? `Polishing with ${textModel}` : 'Polishing') : 'Polish');
  if (kind === 'command') steps.push(long ? (textModel ? `Running your command with ${textModel}` : 'Running your command') : 'Command');
  if (deliver) steps.push(long ? 'Pasting' : 'Paste');
  const at = flow === 'transcribing' ? 0 : Math.min(1, steps.length - 1);
  return (
    <ol className="steps" aria-label="Progress">
      {steps.map((step, index) => (
        <li key={step} className="step" data-state={index < at ? 'done' : index === at ? 'now' : 'next'} aria-current={index === at ? 'step' : undefined}>
          {index > 0 && <span className="step-line" aria-hidden />}
          {index < at ? <Check size={14} aria-hidden /> : index === at ? <Liveness /> : null}
          {step}
        </li>
      ))}
    </ol>
  );
}

export interface DockProps {
  flow: Flow;
  levels: number[];
  elapsedMs: number;
  shortcut: string | null;
  /** True when the shortcut is pressed once to start and again to stop. */
  toggle: boolean;
  integrated: boolean;
  canStart: boolean;
  /** Why the record button cannot start, when it cannot. */
  startBlocked?: string;
  onStart: () => void;
  onStop: () => void;
  /** Present only where the dictation in progress can be thrown away. */
  onDiscard?: () => void;
  /** The finished dictation's outcome, shown in the middle for a few seconds. */
  outcome?: ReactNode;
  modes: ModeSettings | null;
  /** Whether a mode can be chosen now (a Super mode needs a ready text model). */
  modeReady: (profile: ModeProfile) => boolean;
  onMode: (id: string) => void;
  onOpenModes?: () => void;
  settings: Settings;
  providers: DockProvider[];
  /** The selected speech model can transcribe right now. */
  engineReady: boolean;
  onSettings?: (settings: Settings) => void;
  onOpenModels?: () => void;
}

/**
 * The one place to dictate from the window: a floating bar at the foot of the
 * reader. At rest it teaches the shortcut and holds the two decisions that
 * belong to the next dictation (mode, and language and model); while recording
 * it is the timer, the level and the way out; afterwards, the steps and then
 * the outcome. It never changes size, so nothing around it moves.
 */
export function Dock(props: DockProps) {
  const { flow, levels } = props;
  const recording = flow === 'recording';
  const live = recording || flow === 'opening';
  const working = flow === 'transcribing' || flow === 'processing';
  const level = levels[levels.length - 1] ?? 0;
  const [menu, setMenu] = useState<null | 'mode' | 'engine'>(null);
  const close = useCallback(() => setMenu(null), []);
  const active = props.modes?.profiles.find((profile) => profile.id === props.modes?.active);
  const ModeIcon = modeIcon(active?.kind);
  const { selection, input_language: language } = props.settings.transcription;
  const textModel = active?.text_model ? textModelName(active.text_model.model) : null;

  return (
    <div className="dock" data-state={live ? (recording ? 'recording' : 'opening') : working ? 'working' : flow} role="region" aria-label="Dictation">
      <button
        type="button"
        className="dock-mic"
        aria-label={live ? 'Stop' : 'Record'}
        title={live ? 'Stop and transcribe' : props.canStart ? (working ? 'Start the next dictation — this one keeps finishing' : 'Start dictation') : props.startBlocked}
        disabled={!live && !props.canStart}
        onClick={live ? props.onStop : props.onStart}
      >
        <span className="dock-ring" style={recording ? { transform: `scale(${1 + level * 0.18})` } : undefined} aria-hidden />
        {live ? <Square size={18} fill="currentColor" aria-hidden /> : <Mic size={22} aria-hidden />}
      </button>

      <div className="dock-mid" aria-live="polite">
        {flow === 'idle' && <IdleHint shortcut={props.shortcut} toggle={props.toggle} />}
        {flow === 'opening' && <span className="dock-hint"><Liveness /> Opening the microphone…</span>}
        {recording && (
          <>
            <span className="dock-timer" aria-label="Recording duration">{formatDuration(props.elapsedMs)}</span>
            <span className="dock-wave" aria-hidden>
              {levels.map((value, index) => <i key={index} style={{ height: `${Math.max(10, Math.round(value * 100))}%` }} />)}
            </span>
          </>
        )}
        {working && <Steps flow={flow} kind={active?.kind} textModel={textModel} deliver={props.integrated} />}
        {(flow === 'done' || flow === 'failed') && props.outcome}
      </div>

      <div className="dock-end">
        {live
          ? props.onDiscard && (
            <button type="button" className="button ghost danger dock-discard" onClick={props.onDiscard} title="Stop and delete this recording">
              <Trash2 size={15} aria-hidden /> Discard
            </button>
          )
          : (
            <>
              <ModeChip
                open={menu === 'mode'}
                onToggle={() => setMenu(menu === 'mode' ? null : 'mode')}
                onClose={close}
                label={active?.name ?? 'Mode'}
                icon={<ModeIcon size={15} className={active?.kind === 'regular' ? undefined : 'dock-spark'} aria-hidden />}
                modes={props.modes}
                ready={props.modeReady}
                onPick={(id) => { close(); props.onMode(id); }}
                onEdit={props.onOpenModes && (() => { close(); props.onOpenModes?.(); })}
              />
              <EngineChip
                open={menu === 'engine'}
                onToggle={() => setMenu(menu === 'engine' ? null : 'engine')}
                onClose={close}
                selection={selection}
                language={language}
                ready={props.engineReady}
                settings={props.settings}
                providers={props.providers}
                integrated={props.integrated}
                onSettings={props.onSettings && ((next) => { close(); props.onSettings?.(next); })}
                onOpen={props.onOpenModels && (() => { close(); props.onOpenModels?.(); })}
              />
            </>
          )}
      </div>
    </div>
  );
}

function IdleHint({ shortcut, toggle }: { shortcut: string | null; toggle: boolean }) {
  if (!shortcut) return <span className="dock-hint">Press the button and speak · press again when you are done</span>;
  return toggle
    ? <span className="dock-hint">Press <Keys chord={shortcut} /> in any app <b>· speak · press again</b></span>
    : <span className="dock-hint">Hold <Keys chord={shortcut} /> in any app <b>· speak · let go</b></span>;
}

function ModeChip({ open, onToggle, onClose, label, icon, modes, ready, onPick, onEdit }: {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  label: string;
  icon: ReactNode;
  modes: ModeSettings | null;
  ready: (profile: ModeProfile) => boolean;
  onPick: (id: string) => void;
  onEdit?: () => void;
}) {
  const anchor = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  usePopover(open, panel, anchor, onClose);
  return (
    <span className="dock-anchor">
      <button ref={anchor} type="button" className="dock-chip" aria-haspopup="menu" aria-expanded={open} disabled={!modes} onClick={onToggle} title="How the next dictation is processed">
        {icon} {label} <ChevronDown size={14} aria-hidden />
      </button>
      {open && modes && (
        <div className="popover dock-menu" role="menu" aria-label="Recording mode" ref={panel}>
          <h4>Next dictation is processed with</h4>
          {modes.profiles.map((profile) => {
            const Icon = modeIcon(profile.kind);
            const usable = ready(profile);
            const checked = profile.id === modes.active;
            return (
              <button
                key={profile.id}
                type="button"
                role="menuitemradio"
                aria-checked={checked}
                className="popover-item"
                disabled={!usable && !checked}
                onClick={() => { if (!checked) onPick(profile.id); else onClose(); }}
              >
                <Icon size={16} aria-hidden />
                <span className="popover-text">
                  {profile.name}
                  <small>{usable ? describeMode(profile) : 'Needs a ready text model — set it up in Modes'}</small>
                </span>
                {checked && <Check size={15} className="popover-trail" aria-hidden />}
                {!checked && profile.shortcut && <span className="popover-trail"><Keys chord={profile.shortcut} /></span>}
              </button>
            );
          })}
          {onEdit && (
            <>
              <hr />
              <button type="button" role="menuitem" className="popover-item" onClick={onEdit}><Settings2 size={16} aria-hidden /> Edit modes…</button>
            </>
          )}
        </div>
      )}
    </span>
  );
}

function describeMode(profile: ModeProfile): string {
  const model = profile.text_model ? textModelName(profile.text_model.model) : null;
  if (profile.kind === 'regular') return 'Your words, with your dictionary’s fixes';
  if (profile.kind === 'command') return model ? `Speak an instruction for the selected text · ${model}` : 'Speak an instruction for the selected text';
  return model ? `Punctuation and flow by ${model}` : 'Punctuation and flow';
}

function EngineChip({ open, onToggle, onClose, selection, language, ready, settings, providers, integrated, onSettings, onOpen }: {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  selection: ModelSelection;
  language: InputLanguage;
  ready: boolean;
  settings: Settings;
  providers: DockProvider[];
  integrated: boolean;
  onSettings?: (settings: Settings) => void;
  onOpen?: () => void;
}) {
  const anchor = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  usePopover(open, panel, anchor, onClose);
  const here = integrated ? hereLabel() : 'on your server';
  // A model the catalog does not describe is not claimed to be unable: the
  // server checks the pair when it is saved.
  const supports = (candidate: ModelSelection, input: InputLanguage) => modelCapabilities(candidate)?.input_languages.includes(input) ?? true;
  const same = (a: ModelSelection, b: ModelSelection) => a.provider === b.provider && a.model === b.model;

  // The model in use, the local model that is loaded, and the first model of
  // every cloud service with a key, among those that understand the language
  // being spoken. Everything else is one click away in the settings.
  const candidates: { selection: ModelSelection; detail: string }[] = [{
    selection,
    detail: selection.provider === 'local' ? `${here}${ready ? ' · ready' : ' · not ready'}` : `Cloud${ready ? '' : ' · not verified'}`,
  }];
  const loaded = providers.find((provider) => provider.id === 'local')?.models?.[0];
  if (loaded) {
    const local: ModelSelection = { provider: 'local', model: loaded };
    if (!same(local, selection) && supports(local, language)) candidates.push({ selection: local, detail: `${here} · loaded` });
  }
  for (const provider of providers) {
    if (provider.id === 'local' || !provider.configured) continue;
    const model = provider.models?.find((id) => supports({ provider: provider.id as ModelSelection['provider'], model: id }, language));
    if (!model) continue;
    const cloud: ModelSelection = { provider: provider.id as ModelSelection['provider'], model };
    if (!same(cloud, selection)) candidates.push({ selection: cloud, detail: provider.available ? `Cloud · ${provider.name}` : `Cloud · ${provider.name} · key not checked yet` });
  }

  return (
    <span className="dock-anchor">
      <button ref={anchor} type="button" className="dock-chip" aria-haspopup="menu" aria-expanded={open} onClick={onToggle} title={`${modelName(selection.model)} · ${selection.provider === 'local' ? here : 'cloud'}`}>
        <span className={`dot ${ready ? 'ok' : 'warn'}`} aria-hidden />
        {shortModel(selection.model)} <small>· {languageCode(language)}</small>
        <ChevronDown size={14} aria-hidden />
      </button>
      {open && (
        <div className="popover dock-menu engine" role="menu" aria-label="Language and speech model" ref={panel}>
          <h4>I speak</h4>
          <div className="dock-languages" role="group" aria-label="Spoken language">
            {languageCatalog.presets.map((preset) => {
              const fits = supports(selection, preset.id);
              return (
                <button
                  key={preset.id}
                  type="button"
                  role="menuitemradio"
                  aria-checked={language === preset.id}
                  className="chip"
                  disabled={!onSettings || (!fits && language !== preset.id)}
                  title={fits ? undefined : `${shortModel(selection.model)} does not understand ${preset.label}. Pick a model that does in the speech engine settings.`}
                  onClick={() => { if (language !== preset.id) onSettings?.(applyLanguagePreset(settings, preset.id)); }}
                >
                  {preset.label}
                </button>
              );
            })}
          </div>
          <h4>Speech model</h4>
          {candidates.map((candidate) => {
            const checked = same(candidate.selection, selection);
            return (
              <button
                key={`${candidate.selection.provider}/${candidate.selection.model}`}
                type="button"
                role="menuitemradio"
                aria-checked={checked}
                className="popover-item"
                disabled={!onSettings && !checked}
                onClick={() => { if (!checked) onSettings?.({ ...settings, transcription: { ...settings.transcription, selection: candidate.selection } }); else onClose(); }}
              >
                {candidate.selection.provider === 'local'
                  ? <Cpu size={16} aria-hidden />
                  : hasProviderLogo(candidate.selection.provider) ? <ProviderLogo id={candidate.selection.provider} size={16} /> : <Cloud size={16} aria-hidden />}
                <span className="popover-text">
                  {modelName(candidate.selection.model)}
                  <small>{candidate.detail}</small>
                </span>
                {checked && <Check size={15} className="popover-trail" aria-hidden />}
              </button>
            );
          })}
          {onOpen && (
            <>
              <hr />
              <button type="button" role="menuitem" className="popover-item" onClick={onOpen}><Settings2 size={16} aria-hidden /> Speech engine settings…</button>
            </>
          )}
        </div>
      )}
    </span>
  );
}
