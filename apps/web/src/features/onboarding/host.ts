import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke, isTauri, listen } from '../desktop-session/runtime/native';
import { setupApi, type SetupApi, type SetupView } from '../transcription-setup/api';

/**
 * Everything the setup guide needs from the shell that is not already covered
 * by the settings bridge or the setup API.
 *
 * Kept behind one interface so the guide can be mounted in a plain browser for
 * its own fixtures, and so the single place that talks to the shell is visible.
 */
export interface OnboardingStatus {
  /** Guide version this user finished. Zero means never. */
  completed_version: number;
  current_version: number;
}
export interface SystemSettings {
  launch_at_login: boolean;
  sounds: boolean;
  duck_audio: boolean;
}
export type HotkeyChord = 'recording' | 'cancel';
export interface HotkeyEdge {
  chord: HotkeyChord;
  pressed: boolean;
}

export interface OnboardingHost {
  status(): Promise<OnboardingStatus>;
  setCompleted(completed: boolean): Promise<OnboardingStatus>;
  system(): Promise<SystemSettings>;
  saveSystem(settings: SystemSettings): Promise<void>;
  /** Physical press and release of Writer's own chords, for the trainer. */
  onHotkey(listener: (edge: HotkeyEdge) => void): () => void;
}

export const desktopHost: OnboardingHost = {
  status: () => invoke<OnboardingStatus>('onboarding_get'),
  setCompleted: (completed) => invoke<OnboardingStatus>('onboarding_save', { completed }),
  system: () => invoke<SystemSettings>('system_settings_get'),
  saveSystem: (settings) => invoke<void>('system_settings_save', { settings, changed: 'launch_at_login' }),
  onHotkey: (listener) => {
    if (!isTauri()) return () => { /* No global chords outside the shell. */ };
    let live = true;
    let stop: (() => void) | undefined;
    void listen<HotkeyEdge>('writer-hotkey', (edge) => { if (live) listener(edge); })
      .then((unlisten) => { if (live) stop = unlisten; else unlisten(); })
      .catch(() => { /* The trainer falls back to the session states it can see. */ });
    return () => { live = false; stop?.(); };
  },
};

/**
 * Decides whether the guide opens, and records that it was finished.
 *
 * A shell that cannot answer is treated as already set up. Failing the other
 * way would trap someone behind a guide because one command was unavailable,
 * and the workspace has always been usable without it.
 */
export function useOnboardingGate(enabled: boolean, host: OnboardingHost = desktopHost) {
  const [status, setStatus] = useState<OnboardingStatus | null>(null);
  const [resolved, setResolved] = useState(false);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => { live.current = false; };
  }, []);
  useEffect(() => {
    if (!enabled) { setResolved(true); return; }
    void host.status()
      .then((value) => { if (live.current) setStatus(value); })
      .catch(() => { /* Treated as set up; see above. */ })
      .finally(() => { if (live.current) setResolved(true); });
  }, [enabled, host]);

  const open = enabled && resolved && !!status && status.completed_version < status.current_version;

  const finish = useCallback(() => {
    // The guide closes on the local answer: a shell that could not write it
    // shows the guide again next launch, which is the safe direction.
    setStatus((previous) => previous && { ...previous, completed_version: previous.current_version });
    void host.setCompleted(true)
      .then((value) => { if (live.current) setStatus(value); })
      .catch(() => { /* Nothing to say here; the workspace is already open. */ });
  }, [host]);

  const replay = useCallback(async () => {
    const value = await host.setCompleted(false);
    if (live.current) { setStatus(value); setResolved(true); }
  }, [host]);

  return { open, resolved, finish, replay };
}

/**
 * Which of Writer's chords is held right now, plus how many complete presses
 * have been seen, so the trainer can both light the keys and confirm progress.
 */
export function useHotkeySignal(host: OnboardingHost = desktopHost) {
  const [held, setHeld] = useState<HotkeyChord | null>(null);
  const [presses, setPresses] = useState(0);
  useEffect(() => host.onHotkey((edge) => {
    if (edge.pressed) { setHeld(edge.chord); setPresses((value) => value + 1); }
    else setHeld((current) => (current === edge.chord ? null : current));
  }), [host]);
  return { held, presses };
}

/**
 * The managed setup document, refreshed while the guide is open.
 *
 * Downloads, checksum verification and worker startup all report progress
 * through this one document, so the guide polls it faster while any of them is
 * running and slowly the rest of the time. Nothing here is cached across a
 * failure: an unreadable document is reported, not filled in.
 */
export function useSetupView(api: SetupApi = setupApi, pollMs = 2000) {
  const [view, setView] = useState<SetupView | null>(null);
  const [error, setError] = useState('');
  const [tick, setTick] = useState(0);
  const live = useRef(true);
  // Read in the scheduling tail only, so the poll rate can follow the document
  // without the document itself retriggering the fetch.
  const busy = useRef(false);
  useEffect(() => {
    live.current = true;
    return () => { live.current = false; };
  }, []);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    let cancelled = false;
    void api.load()
      .then((next) => {
        if (cancelled || !live.current) return;
        setView(next); setError('');
        busy.current = next.models.some((model) => model.weights.state === 'downloading' || model.weights.state === 'verifying')
          || ['starting', 'stopping'].includes(next.runtime.worker.state);
      })
      .catch((cause: unknown) => {
        if (cancelled || !live.current) return;
        setError(cause instanceof Error ? cause.message : 'Transcription setup could not be reached.');
      })
      .finally(() => {
        if (cancelled || !live.current) return;
        timer = setTimeout(() => setTick((value) => value + 1), busy.current ? pollMs : pollMs * 3);
      });
    return () => { cancelled = true; clearTimeout(timer); };
  }, [api, pollMs, tick]);
  // Every setup action answers with the whole updated document. Adopting it
  // directly is what makes a download bar appear on the click rather than on
  // whichever poll happens next.
  const apply = useCallback((next: SetupView) => {
    setView(next); setError('');
    busy.current = next.models.some((model) => model.weights.state === 'downloading' || model.weights.state === 'verifying')
      || ['starting', 'stopping'].includes(next.runtime.worker.state);
  }, []);
  return { view, error, apply, reload: useCallback(() => setTick((value) => value + 1), []) };
}
