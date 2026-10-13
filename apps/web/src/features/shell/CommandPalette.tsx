import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { CornerDownLeft, Search } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

export interface PaletteItem {
  id: string;
  group: 'Actions' | 'Settings' | 'Dictations';
  label: string;
  detail?: string;
  icon: LucideIcon;
  /** Keys shown on the trailing edge, already in this platform's notation. */
  keys?: string[];
  /** Text the query is matched against, beyond the label. */
  keywords?: string;
  dir?: 'ltr' | 'rtl' | 'auto';
  run: () => void;
}

const fold = (text: string) => text.toLowerCase().normalize('NFKD').replace(/[̀-ͯ]/g, '');

/** Every word of the query must appear at the start of some word in the item. */
export function matchesQuery(query: string, ...fields: (string | undefined)[]): boolean {
  const terms = fold(query).split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  if (!terms.length) return true;
  const haystack = fold(fields.filter(Boolean).join(' '));
  const wordsIn = haystack.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  return terms.every((term) => wordsIn.some((word) => word.startsWith(term)) || (term.length >= 3 && haystack.includes(term)));
}

/**
 * Search everything from one field (Ctrl/⌘ K): actions, settings and what was
 * said. Arrow keys move, Enter runs, Escape closes. Settings results open the
 * Settings search itself with the same words, so the two searches never
 * disagree about what exists.
 */
export function CommandPalette({ onClose, items }: {
  onClose: () => void;
  /** All candidates for a query; the palette groups and limits them. */
  items: (query: string) => PaletteItem[];
}) {
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const results = useMemo(() => {
    const all = items(query.trim());
    const grouped: Record<PaletteItem['group'], PaletteItem[]> = { Actions: [], Settings: [], Dictations: [] };
    for (const item of all) if (grouped[item.group].length < (item.group === 'Dictations' ? 6 : 5)) grouped[item.group].push(item);
    return (['Actions', 'Settings', 'Dictations'] as const).flatMap((group) => grouped[group]);
  }, [items, query]);
  useEffect(() => { setActive(0); }, [query]);
  useEffect(() => {
    const opener = document.activeElement;
    input.current?.focus();
    return () => { if (opener instanceof HTMLElement && opener.isConnected) opener.focus(); };
  }, []);
  useEffect(() => {
    list.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [active]);

  function key(event: React.KeyboardEvent) {
    if (event.key === 'ArrowDown') { event.preventDefault(); setActive((index) => Math.min(results.length - 1, index + 1)); }
    else if (event.key === 'ArrowUp') { event.preventDefault(); setActive((index) => Math.max(0, index - 1)); }
    else if (event.key === 'Enter') { event.preventDefault(); const item = results[active]; if (item) { onClose(); item.run(); } }
    else if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onClose(); }
  }

  let lastGroup = '';
  const rows: ReactNode[] = [];
  results.forEach((item, index) => {
    if (item.group !== lastGroup) {
      lastGroup = item.group;
      rows.push(<h3 key={`h-${item.group}`} className="palette-group">{item.group}</h3>);
    }
    const Icon = item.icon;
    rows.push(
      <button
        key={item.id}
        type="button"
        role="option"
        aria-selected={index === active}
        data-index={index}
        className="palette-item"
        onMouseMove={() => setActive(index)}
        onClick={() => { onClose(); item.run(); }}
      >
        <span className="palette-icon" aria-hidden><Icon size={16} /></span>
        <span className="palette-text">
          <span className="palette-label" dir={item.dir}>{item.label}</span>
          {item.detail && <span className="palette-detail">{item.detail}</span>}
        </span>
        {item.keys && <span className="kbd" aria-hidden>{item.keys.map((token, i) => <kbd key={`${token}-${i}`}>{token}</kbd>)}</span>}
        {index === active && !item.keys && <CornerDownLeft size={14} className="palette-enter" aria-hidden />}
      </button>,
    );
  });

  return (
    <div className="palette-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="Search">
        <div className="palette-query">
          <Search size={18} aria-hidden />
          <input
            ref={input}
            type="search"
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-results"
            aria-label="Search dictations, settings and actions"
            placeholder="Search dictations, settings and actions"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={key}
          />
          <span className="kbd" aria-hidden><kbd>Esc</kbd></span>
        </div>
        <div className="palette-results scroller" id="palette-results" role="listbox" ref={list}>
          {rows.length ? rows : <p className="palette-empty">Nothing matches “{query}”.</p>}
        </div>
      </div>
    </div>
  );
}
