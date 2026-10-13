import { useCallback, useEffect, useState } from 'react';
import { invoke, isTauri } from '../desktop-session/runtime/native';
import { PRODUCT_NAME } from '../../product';

type Access = { supported: boolean; trusted: boolean; tap_active: boolean; globe_action: number | null };
/** Live permission state, never persisted as a grant. The guide can skip it. */
export function Accessibility() {
  const [access, setAccess] = useState<Access | null>(null);
  const [problem, setProblem] = useState('');
  const reload = useCallback(() => {
    if (!isTauri()) return;
    void invoke<Access>('accessibility_status').then(value => { setAccess(value); setProblem(''); })
      .catch(() => setProblem('Could not check Accessibility. Recording with the button remains available.'));
  }, []);
  useEffect(() => {
    reload();
    window.addEventListener('focus', reload);
    const timer = setInterval(reload, 1500);
    return () => { window.removeEventListener('focus', reload); clearInterval(timer); };
  }, [reload]);
  return <div className="ob-card" aria-label="Accessibility access">
    <h3>Accessibility for dictation and paste</h3>
    <p>Allow {PRODUCT_NAME} in System Settings → Privacy &amp; Security → Accessibility to hold fn to dictate. This permission also enables automatic paste and selected-text context. Without it, completed text stays on the clipboard and in history. Microphone recording works independently.</p>
    <p role="status">{access?.tap_active ? 'Accessibility enabled. The fn shortcut is listening.' : access?.trusted ? `Accessibility is enabled, but the keyboard hook is not ready. Re-check or relaunch ${PRODUCT_NAME}.` : 'The fn shortcut is unavailable without Accessibility. Use ⌃+⇧+F9 to record while your recording shortcut needs this permission, or choose a full chord.'}</p>
    <p>If the top row controls media, hold fn too when using ⌃+⇧+F9 to send the F9 key.</p>
    <div className="row">
      <button type="button" className="button" onClick={() => void invoke('accessibility_open').then(reload).catch(() => setProblem(`Open System Settings → Privacy & Security → Accessibility and enable ${PRODUCT_NAME}.`))}>Open Accessibility settings</button>
      <button type="button" className="button" onClick={reload}>Re-check access</button>
    </div>
    <p>In System Settings → Keyboard, set “Press 🌐 key to” to “Do Nothing”. Quick fn taps can otherwise open the language switcher even while {PRODUCT_NAME} records. This matters especially in “Press to start and stop” mode. {PRODUCT_NAME} does not change this setting for you.</p>
    {access?.globe_action === 0 && <p>The Globe-key action is currently set to Do Nothing.</p>}
    {problem && <p className="notice alert" role="alert">{problem}</p>}
  </div>;
}
