import { useState } from 'react';
import { Search } from 'lucide-react';
import type { Theme, ThemeKind } from '../../themes';
import { themeCssVariables } from '../../themes';
import { Group, Row } from './controls';

type Filter = 'all' | ThemeKind;

/** A miniature of the real frame: rail, transcript rows and the record button. */
function Preview({ theme }: { theme: Theme }) {
  const { ui } = theme;
  return (
    <div className="theme-preview" style={{ background: ui.bg }} aria-hidden>
      <div className="rail" style={{ background: ui.backdrop ?? ui.bgRail, borderInlineEnd: `1px solid ${ui.border}` }}>
        <i style={{ background: ui.accentSoft, width: '100%' }} />
        <i style={{ background: ui.borderStrong, width: '80%' }} />
        <i style={{ background: ui.borderStrong, width: '65%' }} />
      </div>
      <div className="body">
        <i style={{ background: ui.text, width: '78%', opacity: 0.85 }} />
        <i style={{ background: ui.textMuted, width: '92%' }} />
        <i style={{ background: ui.textMuted, width: '60%' }} />
        <span className="mic" style={{ background: ui.accent }} />
      </div>
    </div>
  );
}

export function Appearance({ theme, themes, onSelect }: {
  theme: Theme;
  themes: Theme[];
  onSelect: (id: string) => void;
}) {
  const [filter, setFilter] = useState<Filter>('all');
  const [query, setQuery] = useState('');
  const needle = query.trim().toLowerCase();
  const visible = themes.filter((item) =>
    (filter === 'all' || item.kind === filter) &&
    (!needle || item.name.toLowerCase().includes(needle) || item.description.toLowerCase().includes(needle)));

  return (
    <>
      <Group title="Interface">
        <Row
          title="Theme"
          detail="Applies to the whole application: the window, its title bar, the tray menu and the floating recording panel."
        >
          <span className="badge">{theme.name}</span>
        </Row>
      </Group>

      <section className="settings-group">
        <div className="theme-toolbar">
          <div className="segmented" role="group" aria-label="Filter themes">
            {(['all', 'dark', 'light'] as const).map((id) => (
              <button key={id} type="button" aria-pressed={filter === id} onClick={() => setFilter(id)}>
                {id === 'all' ? 'All' : id === 'dark' ? 'Dark' : 'Light'}
              </button>
            ))}
          </div>
          <label className="search-field">
            <Search size={13} aria-hidden />
            <span className="visually-hidden">Search themes</span>
            <input type="search" placeholder="Search themes" value={query} onChange={(event) => setQuery(event.target.value)} />
          </label>
        </div>

        <div className="theme-grid">
          {visible.map((item) => (
            <button
              key={item.id}
              type="button"
              className="theme-card"
              aria-pressed={theme.id === item.id}
              onClick={() => onSelect(item.id)}
              // The card paints itself in its own palette, so the grid is a set
              // of real samples rather than swatches described in words.
              style={themeCssVariables(item) as React.CSSProperties}
            >
              <Preview theme={item} />
              <figcaption>
                <span className="name">
                  <strong>{item.name}</strong>
                  <span>{item.kind}</span>
                </span>
                <p>{item.description}</p>
              </figcaption>
            </button>
          ))}
          {!visible.length && <p className="notice">No theme matches “{query}”.</p>}
        </div>
      </section>
    </>
  );
}
