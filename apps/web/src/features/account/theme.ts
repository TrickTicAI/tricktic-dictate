import { useEffect, useRef } from 'react';
import type { Theme } from '../../themes';
import { isTauri, listen } from '../desktop-session/runtime/native';
import { portableBridge } from './portable';

/**
 * Keeps the shell's copy of the theme in step with this window (MW-OUATH-6).
 * The shell's copy is what export and sync carry, as light or dark. When an
 * import or a synced change sets a different kind, this window adopts it; a
 * named theme of the same kind is the user's own choice and is kept.
 */
export function usePortableTheme(theme: Theme, select: (id: string) => void) {
  const ready = useRef(false);
  const current = useRef(theme);
  current.current = theme;
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    const adopt = () => portableBridge.status().then((status) => {
      if (disposed) return;
      const kept = status?.theme;
      if (kept && kept.kind !== current.current.kind) select(kept.id);
      else void portableBridge.theme(current.current.id, current.current.kind).catch(() => {});
    }).catch(() => {}).finally(() => { ready.current = true; });
    void adopt();
    void Promise.resolve()
      .then(() => listen<{ kind: string }>('writer-sync', (event) => { if (event?.kind === 'applied') void adopt(); }))
      .then((unlisten) => { if (disposed) unlisten(); else stop = unlisten; })
      .catch(() => { /* Older hosts publish no sync events. */ });
    return () => { disposed = true; stop?.(); };
  }, [select]);
  useEffect(() => {
    if (!isTauri() || !ready.current) return;
    void portableBridge.theme(theme.id, theme.kind).catch(() => { /* Export keeps the last theme it saw. */ });
  }, [theme.id, theme.kind]);
}
