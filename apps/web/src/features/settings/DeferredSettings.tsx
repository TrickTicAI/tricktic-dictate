import { useEffect, useState } from 'react';
import { Dialog } from '../shell/Dialog';
import type { SettingsModal, SettingsModalProps } from './SettingsModal';

let loaded: typeof SettingsModal | undefined;

let loading: Promise<typeof SettingsModal> | undefined;
function loadSettings() {
  if (loading) return loading;
  const devUrl = './SettingsModal.tsx#' + performance.now();
  const request: Promise<typeof import('./SettingsModal')> = import.meta.env.DEV
    ? import(/* @vite-ignore */ devUrl)
    : import('./SettingsModal');
  loading = request.then(module => loaded = module.SettingsModal)
    .catch(error => { loading = undefined; throw error; });
  return loading;
}

export function DeferredSettings(props: SettingsModalProps) {
  const [Modal, setModal] = useState(() => loaded);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (Modal) return;
    let open = true;
    void loadSettings().then(component => {
      if (open) setModal(() => component);
    }).catch(() => { if (open) setFailed(true); });
    return () => { open = false; };
  }, [Modal, attempt]);

  if (Modal) return <Modal {...props} />;
  return <Dialog title="Settings" onClose={props.onClose}>
    {failed ? <>
      <p role="alert">Settings could not be opened.</p>
      <button className="button" onClick={() => { setFailed(false); setAttempt(value => value + 1); }}>Retry</button>
    </> : <p role="status">Opening Settings…</p>}
  </Dialog>;
}
