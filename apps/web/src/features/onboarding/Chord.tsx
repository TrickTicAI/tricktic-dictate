import { isMac } from '../settings/platform';
import { Fragment, useState } from 'react';
import { Ban, Keyboard, PanelBottom } from 'lucide-react';
import type { DesktopPreferences } from '@metrix-writer/contracts/desktop';
import { chordTokens } from '../desktop-preferences/chord';
import { Segmented } from '../settings/controls';
import { ShortcutCapture } from '../settings/ShortcutCapture';
import { PRODUCT_NAME } from '../../product';

const defaults = () => ({ recording: isMac() ? 'Fn' : 'Ctrl+Shift+F9', cancel: 'Ctrl+Shift+F10' });

/** One combination drawn as physical keycaps, lit while it is actually held. */
export function Keycaps({ chord, held }: { chord: string; held?: boolean }) {
  const tokens = chordTokens(chord);
  return (
    <span className={`ob-keys ${held ? 'held' : ''}`} aria-label={chord}>
      {tokens.map((token, index) => (
        <Fragment key={`${token}-${index}`}>
          <kbd className="ob-key">{token}</kbd>
          {index < tokens.length - 1 && <span className="ob-plus" aria-hidden>+</span>}
        </Fragment>
      ))}
    </span>
  );
}

/**
 * The one gesture that matters, and what it does to the rest of Windows.
 *
 * This screen never asks for a press. The shortcut is already live, so a press
 * here would start a real dictation, and a stray recording during setup is
 * exactly the sort of thing this application promises not to do. The next
 * screen is where it is held, and there that recording is the point.
 */
export function Chord({ preferences, onSave, busy }: {
  preferences: DesktopPreferences;
  onSave: (next: DesktopPreferences) => void;
  busy: boolean;
}) {
  const [editing, setEditing] = useState<'recording' | 'cancel' | null>(null);
  const field = editing ?? 'recording';

  return (
    <div className="ob-step">
      <div className="ob-head">
        <span className="ob-glyph"><Keyboard size={20} aria-hidden /></span>
        <div>
          <h1>Your dictation shortcut</h1>
          <p>
            {isMac() ? 'Hold fn to record from another app, then release to transcribe and paste. Enable Accessibility for fn and automatic paste. Without it, use ⌃+⇧+F9 and paste the copied text manually.' : `This combination works everywhere in Windows, not only in this window. Use it in your editor, your browser or a chat box: ${PRODUCT_NAME} records, transcribes, and types the text straight into whatever had focus.`}
          </p>
        </div>
      </div>

      <div className="ob-card">
        <div className="ob-split">
          <div className="ob-grow">
            <h3>Record and stop</h3>
            <p>{preferences.toggle
              ? 'Press once to start, once more to stop.'
              : 'Hold it while you talk, let go when you are done. With the native keyboard hook, every key must be released.'}</p>
          </div>
          <div className="row">
            <Keycaps chord={preferences.recording} />
            <button type="button" className="button" disabled={busy} onClick={() => setEditing('recording')}>Change…</button>
          </div>
        </div>

        <div className="ob-subrow">
          <span>Shortcut mode</span>
          <Segmented
            label="Shortcut mode"
            value={preferences.toggle ? 'toggle' : 'hold'}
            disabled={busy}
            options={[{ id: 'hold', label: 'Hold to dictate' }, { id: 'toggle', label: 'Press to start and stop' }]}
            onChange={(mode) => onSave({ ...preferences, toggle: mode === 'toggle' })}
          />
        </div>
      </div>

      <div className="ob-card">
        <div className="ob-split">
          <div className="ob-grow">
            <h3><Ban size={13} aria-hidden /> Cancel</h3>
            <p>Stops the current dictation without transcribing it. Nothing is typed anywhere.</p>
          </div>
          <div className="row">
            <Keycaps chord={preferences.cancel} />
            <button type="button" className="button" disabled={busy} onClick={() => setEditing('cancel')}>Change…</button>
          </div>
        </div>
      </div>

      <div className="ob-card">
        <h3><PanelBottom size={13} aria-hidden /> While you dictate</h3>
        <p>
          A small panel appears with your microphone level and the current stage, so you can see
          what is happening without leaving the app you are typing into. Turn it off, move it or
          shrink it in Settings → Floating panel.
        </p>
      </div>

      <p className="ob-source">
        {isMac() ? 'Use fn alone, or at least two modifiers. A bare ⌘ or ⌥ cannot be a shortcut.' : 'Windows refuses combinations it already owns, and F12 belongs to debuggers.'} Record and cancel must differ. Dictation modes can have their own shortcuts in Settings → Modes.
      </p>

      {editing && (
        <ShortcutCapture
          title={field === 'recording' ? 'record and stop' : 'cancel'}
          current={preferences[field]}
          otherChord={field === 'recording' ? preferences.cancel : preferences.recording}
          otherLabel={field === 'recording' ? 'cancel and keep audio' : 'record and stop dictation'}
          onCommit={(chord) => { setEditing(null); onSave({ ...preferences, [field]: chord }); }}
          onCancel={() => setEditing(null)}
          onReset={() => { setEditing(null); onSave({ ...preferences, [field]: defaults()[field] }); }}
        />
      )}
    </div>
  );
}
