import { Accessibility } from './Accessibility';
import { isMac } from './platform';
import { useState } from 'react';
import type { DesktopPreferences, PasteInto } from '@metrix-writer/contracts/desktop';
import { chordTokens } from '../desktop-preferences/chord';
import { BrowserModeNote, Group, Row, Segmented, Toggle } from './controls';
import { ShortcutCapture } from './ShortcutCapture';
import { PRODUCT_NAME } from '../../product';

const defaults = () => ({ recording: isMac() ? 'Fn' : 'Ctrl+Shift+F9', cancel: 'Ctrl+Shift+F10' });

function Chord({ chord }: { chord: string }) {
  return (
    <span className="chord">
      {chordTokens(chord).map((token, index, all) => (
        <span key={`${token}-${index}`} className="chord">
          <kbd>{token}</kbd>
          {index < all.length - 1 && <span className="plus">+</span>}
        </span>
      ))}
    </span>
  );
}

export function Shortcuts({ integrated, preferences, onSave, busy, error, status }: {
  integrated: boolean;
  preferences: DesktopPreferences | null;
  onSave: (next: DesktopPreferences) => void;
  busy: boolean;
  error: string;
  status: string;
}) {
  const [editing, setEditing] = useState<'recording' | 'cancel' | null>(null);

  if (!integrated) {
    return (
      <>
        <BrowserModeNote what="Global shortcuts" />
        <Group title="Recording">
          <Row title="Record and stop" detail="Set in the desktop app.">
            <Chord chord={defaults().recording} />
          </Row>
          <Row title="Discard dictation" detail="Set in the desktop app.">
            <Chord chord={defaults().cancel} />
          </Row>
        </Group>
      </>
    );
  }

  if (!preferences) {
    return <p className="notice" role="status">{error || 'Reading the shell’s shortcut settings…'}</p>;
  }

  const field = editing ?? 'recording';
  const pasteInto: PasteInto = preferences.paste_into ?? 'stop';

  return (
    <>
      <Group title="Recording">
        <Row
          title="Record and stop dictation"
          detail={preferences.toggle
            ? 'Press once to start, again to stop.'
            : isMac() ? 'Hold to dictate. With Accessibility enabled, release every key in the combination to stop.' : 'Hold to dictate. Windows waits for every key in the combination to be released before stopping.'}
        >
          <Chord chord={preferences.recording} />
          <button type="button" className="button" disabled={busy} onClick={() => setEditing('recording')}>Change…</button>
        </Row>

        <Row
          title="Discard dictation"
          detail="Throws the current dictation away: no transcript, and the audio recorded so far is deleted. Use Stop to keep it."
        >
          <Chord chord={preferences.cancel} />
          <button type="button" className="button" disabled={busy} onClick={() => setEditing('cancel')}>Change…</button>
        </Row>

        <Row
          title="How the shortcut behaves"
          detail={preferences.toggle
            ? 'Toggle: one press starts recording, the next stops it. Nothing has to stay held down.'
            : 'Hold: recording lasts exactly as long as you hold the keys, like a push-to-talk button.'}
          stacked
        >
          <Segmented
            label="Shortcut mode"
            value={preferences.toggle ? 'toggle' : 'hold'}
            disabled={busy}
            options={[{ id: 'hold', label: 'Hold to dictate' }, { id: 'toggle', label: 'Press to start and stop' }]}
            onChange={(mode) => onSave({ ...preferences, toggle: mode === 'toggle' })}
          />
        </Row>

        <Row
          title="Where the text goes"
          detail={pasteInto === 'start'
            ? `Into the window you started dictating in, even if you have moved on by the time you stop. ${PRODUCT_NAME} brings it back to the front to paste.`
            : 'Into the window in front of you when you stop, so you can start anywhere, go to where the text belongs and stop there. Command mode always works where you selected the text.'}
          stacked
        >
          <Segmented
            label="Where the text goes"
            value={pasteInto}
            disabled={busy}
            options={[{ id: 'stop' as PasteInto, label: 'Where I stop' }, { id: 'start' as PasteInto, label: 'Where I started' }]}
            onChange={(paste_into) => onSave({ ...preferences, paste_into })}
          />
        </Row>
        <Row
          title="Preserve clipboard after dictation"
          detail={preferences.private_clipboard
            ? 'Restore your previous clipboard after an observed paste. Restoration does not prevent other apps from reading dictation while it is on the clipboard; a failed or unobserved paste leaves it available for manual paste.'
            : isMac()
            ? `Restore your previous clipboard once the paste has read the dictation, so a clipboard history such as CopyQ lists your copy first and dictation second. Preservation is best effort: if the clipboard cannot be restored, dictation stays available on the clipboard or in ${PRODUCT_NAME} history.`
            : `Restore your previous clipboard ahead of dictation in Win+V when Windows can retain both items. Preservation is best effort: if the clipboard cannot be restored, dictation stays available on the clipboard or in ${PRODUCT_NAME} history.`}
        >
          <Toggle label="Preserve clipboard after dictation" checked={preferences.preserve_clipboard ?? false}
            disabled={busy} onChange={(preserve_clipboard) => onSave({ ...preferences, preserve_clipboard })} />
        </Row>
        <Row
          title="Private clipboard delivery"
          detail={isMac()
            ? `Ask compatible clipboard managers not to retain automatic dictation, using confidential and temporary-content markers. These are requests, not access controls: managers such as CopyQ may ignore them, and cross-device sync is not guaranteed to be blocked. Explicit Copy and ${PRODUCT_NAME} history are unchanged.`
            : `Exclude automatic dictation from Windows clipboard history (Win+V) and cloud clipboard sync. Other apps and third-party clipboard managers can still read or retain it. Explicit Copy and ${PRODUCT_NAME} history are unchanged.`}
        >
          <Toggle label="Private clipboard delivery" checked={preferences.private_clipboard ?? false}
            disabled={busy} onChange={(private_clipboard) => onSave({ ...preferences, private_clipboard })} />
        </Row>
      </Group>

      <p className="notice">
        {isMac() ? 'Use fn alone, two modifiers, or ⌘, ⌥, ⌃, ⇧ with a key. Modifier-only and fn shortcuts need Accessibility. Full chords keep working without it. Record and cancel need different combinations.' : 'Use Ctrl, Alt, Shift or Win with a key, or Win with another modifier (for example Win+Alt). Windows may refuse combinations it already uses. F12 is reserved for debuggers. Record and cancel need different combinations.'}
      </p>
      {isMac() && <Accessibility />}
      {error && <p className="notice alert" role="alert">{error}</p>}
      {status && <p className="notice" role="status">{status}</p>}

      {editing && (
        <ShortcutCapture
          title={field === 'recording' ? 'record and stop' : 'discard'}
          current={preferences[field]}
          otherChord={field === 'recording' ? preferences.cancel : preferences.recording}
          otherLabel={field === 'recording' ? 'discard dictation' : 'record and stop dictation'}
          onCommit={(chord) => { setEditing(null); onSave({ ...preferences, [field]: chord }); }}
          onCancel={() => setEditing(null)}
          onReset={() => { setEditing(null); onSave({ ...preferences, [field]: defaults()[field] }); }}
        />
      )}
    </>
  );
}
