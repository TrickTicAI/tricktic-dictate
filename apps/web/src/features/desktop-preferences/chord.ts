import { isMac } from '../settings/platform';

/*
 * A TypeScript mirror of the chord rules in
 * `apps/desktop/tauri/src/shell/preferences.rs` (`keys` and `validate`).
 *
 * This exists so the capture field can reject a combination before a round
 * trip and explain why. The native parser stays the final authority: anything
 * accepted here is still re-validated by `preferences_save`, which owns global
 * registration and can refuse a chord another application already holds.
 */

export const MAX_CHORD_LENGTH = 80;

/**
 * Physical key codes, never typed characters.
 *
 * `event.key` reports the character the layout produces, so with a Hebrew
 * layout active pressing E yields `ק`. `event.code` reports the key's position,
 * which is what the native side registers and what survives a layout change or
 * Caps Lock. This is the same reasoning recorded in the Rust parser.
 */
export function chordKeyFromCode(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^Numpad[0-9]$/.test(code)) return code.slice(6);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  if (code === 'Space') return 'Space';
  if (code === 'Escape') return 'Escape';
  if (code === 'Backspace') return 'Backspace';
  return null;
}

export const MODIFIER_CODES = new Set([
  'ControlLeft', 'ControlRight', 'AltLeft', 'AltRight', 'ShiftLeft', 'ShiftRight', 'MetaLeft', 'MetaRight',
]);

export interface ChordDraft {
  fn?: boolean;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
  key: string | null;
}

export function draftFromEvent(event: Pick<KeyboardEvent, 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey' | 'code'> & Partial<Pick<KeyboardEvent, 'getModifierState'>>): ChordDraft {
  return {
    fn: isMac() && (event.code === 'Fn' || !!event.getModifierState?.('Fn')),
    ctrl: event.ctrlKey,
    alt: event.altKey,
    shift: event.shiftKey,
    meta: event.metaKey,
    key: MODIFIER_CODES.has(event.code) ? null : chordKeyFromCode(event.code),
  };
}

/** The chord string the shell stores, e.g. `Ctrl+Shift+F9`. */
export function formatChord(draft: ChordDraft): string | null {
  if (isMac()) { if (chordProblem(draft)) return null; }
  else if (!draft.key && !(draft.meta && (draft.ctrl || draft.alt || draft.shift))) return null;
  const parts: string[] = [];
  if (isMac() && draft.fn) parts.push('Fn');
  if (draft.meta) parts.push('Win');
  if (draft.ctrl) parts.push('Ctrl');
  if (draft.alt) parts.push('Alt');
  if (draft.shift) parts.push('Shift');
  if (!parts.length) return null;
  return [...parts, ...(draft.key ? [draft.key] : [])].join('+');
}

/** The tokens a capture field shows while the user is still holding keys down. */
export function chordParts(draft: ChordDraft): string[] {
  const parts: string[] = [];
  if (isMac() && draft.fn) parts.push('fn');
  if (draft.ctrl) parts.push(isMac() ? '⌃' : 'Ctrl');
  if (draft.alt) parts.push(isMac() ? '⌥' : 'Alt');
  if (draft.shift) parts.push(isMac() ? '⇧' : 'Shift');
  if (draft.meta) parts.push(isMac() ? '⌘' : 'Win');
  if (draft.key) parts.push(draft.key);
  return parts;
}

/**
 * Returns an explanation when the draft cannot become a chord, or `null` when
 * it is acceptable. Every rule here also exists in Rust.
 */
export function chordProblem(draft: ChordDraft): string | null {
  if (isMac()) {
    const count = [draft.fn, draft.ctrl, draft.alt, draft.shift, draft.meta].filter(Boolean).length;
    if (!draft.key && !draft.fn && count < 2) return 'Use fn alone, or at least two modifiers. A bare ⌘ or ⌥ cannot be a shortcut.';
    if (draft.key && !count) return 'Add at least one of fn, ⌘, ⌥, ⌃ or ⇧.';
    if (draft.key && (!chordKeyFromCode(keyToCode(draft.key)) || /^F(2[1-4])$/.test(draft.key))) return 'Use a letter, digit, F1–F20, Space, Escape or Backspace.';
    return null;
  }

  if (draft.key === 'F12') return 'F12 is reserved for debuggers. Choose another key.';
  if (!draft.ctrl && !draft.alt && !draft.shift && !draft.meta) return 'Add at least one of Ctrl, Alt, Shift or Win.';
  if (!draft.key) return null;
  if (!chordKeyFromCode(keyToCode(draft.key))) return 'Use a letter, digit, function key (except F12), Space, Escape or Backspace.';
  const chord = formatChord(draft);
  if (chord && chord.length > MAX_CHORD_LENGTH) return 'That combination is too long to store.';
  return null;
}

function keyToCode(key: string): string {
  if (/^[A-Z]$/.test(key)) return `Key${key}`;
  if (/^[0-9]$/.test(key)) return `Digit${key}`;
  return key;
}

/** The final token of a chord. */
export function finalKey(chord: string): string {
  const parts = chord.split('+');
  return (parts[parts.length - 1] ?? '').trim().toUpperCase();
}

/**
 * Record and cancel cannot use the same complete combination.
 */
export function conflictWith(chord: string, other: string, otherLabel: string): string | null {
  if (!chord || !other) return null;
  const canonical = (value: string) => value.toUpperCase().split('+').map(p => p.trim().replace(/^CONTROL$/, 'CTRL').replace(/^(SUPER|META|CMD|COMMAND|⌘)$/, 'WIN').replace(/^(OPTION|OPT|⌥)$/, 'ALT').replace(/^⌃$/, 'CTRL').replace(/^⇧$/, 'SHIFT').replace(/^GLOBE$/, 'FN')).sort().join('+');
  return canonical(chord) === canonical(other)
    ? `${other} is already used to ${otherLabel}. Choose a different key, or change that shortcut first.`
    : null;
}

/** Splits a stored chord for display as individual keycap tokens. */
export function chordTokens(chord: string): string[] {
  return chord.split('+').map((part) => { const token = part.trim(); return isMac() ? ({ WIN: '⌘', SUPER: '⌘', META: '⌘', CMD: '⌘', COMMAND: '⌘', CTRL: '⌃', CONTROL: '⌃', ALT: '⌥', OPTION: '⌥', OPT: '⌥', SHIFT: '⇧', FN: 'fn', GLOBE: 'fn' }[token.toUpperCase()] ?? token) : token; }).filter(Boolean);
}
