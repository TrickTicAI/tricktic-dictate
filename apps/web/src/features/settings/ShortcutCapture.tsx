import { isMac } from './platform';
import { isTauri, invoke, listen } from '../desktop-session/runtime/native';
import { useCallback, useEffect, useRef, useState } from 'react';
import { chordParts, chordProblem, conflictWith, draftFromEvent, formatChord, type ChordDraft } from '../desktop-preferences/chord';

const EMPTY: ChordDraft = { ctrl: false, alt: false, shift: false, meta: false, key: null };

/**
 * Press the combination; the app reads it. The old shell asked you to type
 * `Ctrl+Shift+F9` into a text box and parsed the string.
 *
 * Keys are read from `event.code`, the physical position, never `event.key`.
 * With a Hebrew layout active `event.key` for the E key is `ק`, and Caps Lock
 * changes it again; the native side registers physical keys, so the two must
 * agree. `preventDefault` keeps the browser from acting on the combination
 * while it is being captured.
 */
export function ShortcutCapture({ title, current, otherChord, otherLabel, onCommit, onCancel, onReset }: {
  title: string;
  current: string;
  otherChord: string;
  otherLabel: string;
  onCommit: (chord: string) => void;
  onCancel: () => void;
  onReset: () => void;
}) {
  const [draft, setDraft] = useState<ChordDraft>(EMPTY);
  const captured = useRef<string | null>(null);
  const nativeFn = useRef(false);
  const [problem, setProblem] = useState<string | null>(null);

  const finish = useCallback(() => {
    const chord = captured.current;
    if (!chord) return;
    captured.current = null;
    onCommit(chord);
  }, [onCommit]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // WebKit does not reliably send a DOM key event for bare fn. Only fn
    // edges, while this editor is open and Writer has focus, cross the bridge.
    if (isTauri()) void (async () => {
      if (isMac()) unlisten = await listen<ChordDraft & { pressed: boolean }>('writer-fn-capture', (event) => {
        if (disposed) return;
        nativeFn.current = event.pressed;
        if (!event.pressed) { finish(); return; }
        const next = { ...event, fn: true, key: null };
        setDraft(next);
        const chord = formatChord(next);
        const issue = chordProblem(next) ?? (chord ? conflictWith(chord, otherChord, otherLabel) : null);
        setProblem(issue);
        captured.current = issue ? null : chord;
      });
      if (disposed) { unlisten?.(); return; }
      await invoke('shortcut_capture', { active: true });
      if (disposed) await invoke('shortcut_capture', { active: false });
    })().catch(() => setProblem('Could not enable shortcut capture. Close this dialog and try again.'));

    function down(event: KeyboardEvent) {
      if (event.key === 'Escape' && !event.ctrlKey && !event.altKey && !event.shiftKey && !event.metaKey) { onCancel(); return; }
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat) return;
      const next = draftFromEvent(event);
      if (isMac() && nativeFn.current) next.fn = true;
      setDraft(next);
      const issue = chordProblem(next);
      setProblem(issue);
      const chord = issue ? null : formatChord(next);
      // Mirror the native complete-combination collision check.
      const collision = chord ? conflictWith(chord, otherChord, otherLabel) : null;
      if (collision) setProblem(collision);
      captured.current = chord && !collision ? chord : null;
    }
    function up(event: KeyboardEvent) {
      event.preventDefault();
      // Committing on release is what makes "hold the combination you want"
      // read naturally: the draft is visible the whole time it is held.
      if (captured.current) finish();
      else setDraft((previous) => ({ ...previous, ...draftFromEvent(event), key: previous.key }));
    }
    window.addEventListener('blur', onCancel);
    window.addEventListener('keydown', down, true);
    window.addEventListener('keyup', up, true);
    return () => {
      disposed = true;
      nativeFn.current = false;
      unlisten?.();
      if (isTauri()) void invoke('shortcut_capture', { active: false }).catch(() => {});
      window.removeEventListener('blur', onCancel);
      window.removeEventListener('keydown', down, true);
      window.removeEventListener('keyup', up, true);
    };
  }, [finish, onCancel, otherChord, otherLabel]);

  const parts = chordParts(draft);

  return (
    <div className="capture-dialog" role="dialog" aria-modal="true" aria-label={`Set the ${title} shortcut`}>
      <div className="capture-panel">
        <h3>Press the keys you want to use.</h3>
        <p>Release to save. Currently {current}.</p>
        {isMac() && <p>Capturing fn requires Accessibility access. You can also choose fn below and enable access later.</p>}

        <div className="capture-slot" aria-live="polite">
          {parts.length ? (
            parts.map((part, index) => (
              <span key={`${part}-${index}`} className="chord">
                <kbd>{part}</kbd>
                {index < parts.length - 1 && <span className="plus">+</span>}
              </span>
            ))
          ) : (
            <span className="waiting">Waiting for a combination…</span>
          )}
          {parts.length > 0 && !formatChord(draft) && <span className="waiting">+ waiting for a key…</span>}
        </div>

        {problem && <p className="notice alert" role="alert">{problem}</p>}

        <div className="capture-actions">
          <button type="button" className="button" onClick={onCancel}>Cancel</button>
          <button type="button" className="button" onClick={onReset}>Reset to default</button>
          {isMac() && <button type="button" className="button" onClick={() => {
            const issue = conflictWith('Fn', otherChord, otherLabel);
            if (issue) setProblem(issue); else onCommit('Fn');
          }}>Use fn</button>}
        </div>
      </div>
    </div>
  );
}
