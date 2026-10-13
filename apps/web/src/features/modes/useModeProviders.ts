import { useCallback, useEffect, useState } from 'react';
import type { ModelSelection } from '@metrix-writer/contracts';
import { request } from '../transcripts/api';
import type { Reasoning } from './api';
/** `custom` is present only for a model added by ID, with what each speed sends. */
export interface TextChoice { selection: ModelSelection; configured: boolean; execution_location: 'cloud' | 'self_hosted_server'; custom?: { fast: Reasoning; balanced: Reasoning } }
export function useModeProviders() {
  const [choices, setChoices] = useState<TextChoice[]>([]);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let mounted = true;
    const refresh = () => request<TextChoice[]>('/api/v1/modes/providers').then(value => { if (mounted) setChoices(previous => JSON.stringify(previous) === JSON.stringify(value) ? previous : value); }).catch(() => {});
    void refresh(); const timer = setInterval(() => void refresh(), 3000);
    return () => { mounted = false; clearInterval(timer); };
  }, [revision]);
  /** Reads the list now, rather than at the next poll, after something changed it. */
  const refresh = useCallback(() => setRevision(r => r + 1), []);
  return { choices, refresh };
}
