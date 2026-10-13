import '@fontsource/inter/400.css';
import '@fontsource/inter/500.css';
import '@fontsource/inter/600.css';
import '@fontsource/assistant/400.css';
import '@fontsource/assistant/600.css';
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import type { Vocabulary } from '@metrix-writer/contracts';
import { Dictionary } from '../Dictionary';
import { activeVocabulary } from '../../vocabulary/api';
import { MenuHost } from '../../shell/menu';
import { useMotion } from '../../shell/motion';
import '../../../styles.css';

declare global {
  interface Window { leaveDictionary(): void; openDictionary(): void }
}

/*
 * Mirrors how App.tsx renders the section: `{tab === 'dictionary' && <Dictionary/>}`,
 * so leaving really unmounts. The two window hooks flip that without touching
 * the DOM, which keeps the unmount path distinguishable from a blur flush.
 */
function Harness() {
  const [shown, setShown] = useState(true);
  const [saves, setSaves] = useState(0);
  /* What App hands to dictation after each save: the switched-on entries only. */
  const [active, setActive] = useState<Vocabulary | null>(null);
  // As the browser build does: animations follow the system setting.
  useMotion(undefined, false);
  window.leaveDictionary = () => setShown(false);
  window.openDictionary = () => setShown(true);
  return (
    // The workspace frame without its title bar, so the section's own head,
    // scroller and list lay out as they do in the application.
    <div className="shell" style={{ gridTemplateRows: 'minmax(0, 1fr)' }}>
      <div className="shell-body" style={{ gridTemplateColumns: 'minmax(0, 1fr)' }}>
        {shown && <Dictionary onSaved={(value) => { setSaves((n) => n + 1); setActive(activeVocabulary(value)); }} />}
      </div>
      {/* The window mounts one menu surface for everything in it, as App does. */}
      <MenuHost />
      <p data-testid="saves" className="visually-hidden">{saves}</p>
      <p data-testid="active-words" className="visually-hidden">{active?.hints.join(', ') ?? ''}</p>
      <p data-testid="active-corrections" className="visually-hidden">
        {active?.corrections.map((rule) => `${rule.from}→${rule.to}`).join(', ') ?? ''}
      </p>
    </div>
  );
}

createRoot(document.getElementById('root')!).render(<Harness />);
