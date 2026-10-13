import { useCallback, useEffect, useState } from 'react';
import { AlertCircle, ArrowDownToLine, CheckCircle2, RefreshCw } from 'lucide-react';
import { invoke, isTauri } from '../desktop-session/runtime/native';
import { PRODUCT_NAME } from '../../product';

export type UpdateView = {
  installed: string; available: string | null; notes: string | null;
  stage: 'unchecked' | 'checking' | 'available' | 'current' | 'downloading' | 'ready' | 'installing' | 'error';
  downloaded: number; total: number | null; checked_at: number | null; error: string | null;
};

export function useUpdates() {
  const [view, setView] = useState<UpdateView>();
  const [error, setError] = useState<string>();
  const [acting, setActing] = useState(false);
  const action = useCallback(async (command: string) => {
    setActing(true); setError(undefined);
    try {
      if (command === 'updates_cancel') {
        await invoke(command);
        setView(await invoke<UpdateView>('updates_status'));
      }
      else setView(await invoke<UpdateView>(command));
    }
    catch (e) { setError(String(e)); }
    finally { setActing(false); }
  }, []);
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    const refresh = () => void invoke<UpdateView>('updates_status').then(v => { if (alive) setView(v); }).catch(() => {});
    refresh();
    const timer = setInterval(refresh, 1000);
    const first = setTimeout(() => { void action('updates_check'); }, 10000);
    const periodic = setInterval(() => { void action('updates_check'); }, 6 * 60 * 60 * 1000);
    return () => { alive = false; clearInterval(timer); clearTimeout(first); clearInterval(periodic); };
  }, [action]);
  return { view, error, acting, action, enabled: isTauri() };
}
export type UpdateController = ReturnType<typeof useUpdates>;

const MB = (bytes: number) => `${Math.round(bytes / 1024 / 1024)} MB`;

/**
 * The whole update surface, and the only thing About shows besides the version.
 * One line saying where this installation stands, then the single action that
 * moves it forward. Everything the old About page said about what Writer is
 * belongs next to the settings that control it, not here.
 */
export function UpdatesPanel({ updates }: { updates: UpdateController }) {
  if (!updates.enabled) return null;
  const { view, error, acting, action } = updates;
  const stage = view?.stage ?? 'unchecked';
  const busy = acting || ['checking', 'downloading', 'installing'].includes(stage);
  const problem = error || view?.error;

  const state = problem
    ? { icon: <AlertCircle size={16} />, text: stage === 'error' ? 'Could not check for updates' : 'Update failed' }
    : stage === 'ready'
      ? { icon: <ArrowDownToLine size={16} />, text: 'Ready to install' }
      : stage === 'downloading'
        ? { icon: <ArrowDownToLine size={16} />, text: `Downloading ${view?.available ?? ''}`.trim() }
        : stage === 'installing'
          ? { icon: <ArrowDownToLine size={16} />, text: 'Preparing to update and restart' }
          : stage === 'checking'
            ? { icon: <RefreshCw size={16} />, text: 'Checking for updates' }
            : view?.available
              ? { icon: <ArrowDownToLine size={16} />, text: `Version ${view.available} is available` }
              : stage === 'current'
                ? { icon: <CheckCircle2 size={16} />, text: `${PRODUCT_NAME} is up to date` }
                : { icon: <RefreshCw size={16} />, text: 'Not checked yet' };

  return (
    <section className="about-update" aria-label="Application updates">
      <div className="state" role="status">{state.icon}<span>{state.text}</span></div>

      <p>
        Installed <span>{view?.installed || 'Checking…'}</span>
        {view?.checked_at
          ? ` · last checked ${new Date(view.checked_at * 1000).toLocaleString()}`
          : ' · no successful check yet'}
      </p>

      {view?.notes && <p className="notes">{view.notes}</p>}

      {view && stage === 'downloading' && (
        <>
          <div className="progress" aria-hidden>
            <i style={{ width: view.total ? `${Math.round((100 * view.downloaded) / view.total)}%` : '35%' }} />
          </div>
          <p role="status">{MB(view.downloaded)}{view.total ? ` of ${MB(view.total)}` : ''} downloaded</p>
        </>
      )}
      {stage === 'ready' && <p>Finish any recording and transcription, then update and restart.</p>}
      {problem && <p role="alert" className="alert-text">{problem}</p>}

      <div className="actions">
        <button type="button" className="button" disabled={busy || stage === 'ready'} onClick={() => void action('updates_check')}>
          Check for updates
        </button>
        {['downloading', 'ready'].includes(stage) && (
          <button type="button" className="button" onClick={() => void action('updates_cancel')}>
            {stage === 'ready' ? 'Discard download' : 'Cancel download'}
          </button>
        )}
        {view?.available && stage !== 'ready' && (
          <button type="button" className="button primary" disabled={busy} onClick={() => void action('updates_download')}>
            Download {view.available}
          </button>
        )}
        {stage === 'ready' && (
          <button type="button" className="button primary" disabled={busy} onClick={() => void action('updates_install')}>
            Update and restart
          </button>
        )}
      </div>
    </section>
  );
}
