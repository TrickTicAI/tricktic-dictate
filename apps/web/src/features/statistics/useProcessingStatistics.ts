import { useEffect, useState } from 'react';
import { request } from '../transcripts/api';
import type { ProcessingAttempt } from '../modes/api';

/** All saved processing attempts, including manual Super retries; read-only and bounded. */
export function useProcessingStatistics(jobIds: string[]) {
  const signature = [...new Set(jobIds)].sort().join('|');
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<{ attempts: Record<string, ProcessingAttempt[]>; failed: string[]; loading: boolean }>({ attempts: {}, failed: [], loading: true });
  useEffect(() => {
    const ids = signature ? signature.split('|') : [];
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const attempts: Record<string, ProcessingAttempt[]> = {};
    const failed = new Set<string>();
    setState({ attempts: {}, failed: [], loading: ids.length > 0 });
    async function load(queue: string[]) {
      await Promise.all(Array.from({ length: Math.min(4, queue.length) }, async () => {
        for (;;) {
          const id = queue.shift(); if (!id || controller.signal.aborted) return;
          try {
            const value = await request<ProcessingAttempt[]>(`/api/v1/jobs/${encodeURIComponent(id)}/processing`, { signal: controller.signal });
            if (!Array.isArray(value)) throw Error('Invalid processing history');
            attempts[id] = value; failed.delete(id);
          } catch { failed.add(id); }
        }
      }));
      if (controller.signal.aborted) return;
      setState({ attempts: { ...attempts }, failed: [...failed], loading: false });
      const pending = ids.filter(id => attempts[id]?.some(a => a.state.state === 'pending'));
      if (pending.length) timer = setTimeout(() => void load(pending), 3000);
    }
    void load([...ids]);
    return () => { controller.abort(); clearTimeout(timer); };
  }, [signature, revision]);
  return { ...state, reload: () => setRevision(r => r + 1) };
}
