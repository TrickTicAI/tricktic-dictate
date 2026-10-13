import { useEffect, useRef, useState } from 'react';
import { request } from '../transcripts/api';
import type { ProcessingAttempt } from './api';

export interface ModeResult {
  status: 'loading' | 'none' | 'pending' | 'succeeded' | 'failed' | 'unavailable';
  attempt?: ProcessingAttempt;
  delivery?: { attempt_id: string; original_used: boolean; fallback_used: boolean; copied: boolean; pasted: boolean; at_ms: number; app?: string | null };
}
export const loadingMode: ModeResult = { status: 'loading' };
/** Read the selected history result and immutable automatic-delivery receipt.
 * Never start processing or replay a saved recording on load. */
export function useModeResults(jobIds: string[]) {
  const signature = [...new Set(jobIds)].sort().join('|');
  const [results, setResults] = useState<Record<string, ModeResult>>({});
  const cache = useRef<Record<string, ModeResult>>({});
  const [reload, setReload] = useState(0);
  useEffect(() => {
    const ids = signature ? signature.split('|') : [];
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    // Results are published together, at most every 100 ms: one render per
    // result made a long history redraw itself hundreds of times while loading.
    let publish: ReturnType<typeof setTimeout> | undefined;
    const published = () => { clearTimeout(publish); publish = undefined; if (!controller.signal.aborted) setResults(cache.current); };
    const terminal = new Set(ids.filter(id => modeReady(cache.current[id])));
    const failures = new Map<string, number>();
    cache.current = Object.fromEntries(ids.flatMap(id => cache.current[id] ? [[id, cache.current[id]]] : []));
    setResults(cache.current);
    async function refresh() {
      const queue = ids.filter(id => !terminal.has(id));
      // History can contain many jobs. Bound concurrent reads; only unfinished
      // results are polled again, and failures stop after three attempts.
      await Promise.all(Array.from({ length: Math.min(4, queue.length) }, async () => {
        for (;;) {
          const id = queue.shift();
          if (!id || controller.signal.aborted) return;
          let value: ModeResult;
          try {
            const live = await request<{ bound: boolean; attempt: ProcessingAttempt | null; selected_attempt?: ProcessingAttempt | null; delivery?: ModeResult['delivery'] }>(`/api/v1/jobs/${encodeURIComponent(id)}/live-processing`, { signal: controller.signal });
            const attempt = live.selected_attempt ?? live.attempt;
            value = { status: attempt ? attempt.state.state : !live.bound ? 'none' : 'pending', attempt: attempt ?? undefined, delivery: live.delivery };
            if (value.status !== 'pending') terminal.add(id);
          } catch {
            value = { status: 'unavailable' };
            const count = (failures.get(id) ?? 0) + 1;
            failures.set(id, count);
            if (count >= 3) terminal.add(id);
          }
          if (!controller.signal.aborted) { cache.current = { ...cache.current, [id]: value }; publish ??= setTimeout(published, 100); }
        }
      }));
      published();
      if (!controller.signal.aborted && terminal.size < ids.length) timer = setTimeout(() => void refresh(), 1000);
    }
    void refresh();
    return () => { controller.abort(); clearTimeout(timer); clearTimeout(publish); };
  }, [signature, reload]);
  return { results, reload: () => { cache.current = {}; setReload(value => value + 1); } };
}

export function modeText(mode?: ModeResult): string | undefined {
  return mode?.attempt?.state.state === 'succeeded' ? mode.attempt.state.text : undefined;
}
export function modeReady(mode?: ModeResult): boolean {
  return mode?.status === 'none' || mode?.status === 'succeeded' || mode?.status === 'failed';
}
