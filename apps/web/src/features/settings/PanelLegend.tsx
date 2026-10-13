import { Check } from 'lucide-react';

/*
 * What the floating panel can show, in one strip: a small drawing of each state
 * and what it means. It is a key to the panel above, not a second panel, so it
 * is drawn in the window's colours and does not move.
 */
const STATES: { id: string; label: string; meaning: string }[] = [
  { id: 'opening', label: 'Opening', meaning: 'The microphone is starting. Start talking in a moment.' },
  { id: 'listening', label: 'Listening', meaning: 'Bars move with your voice; a dashed line means no level is coming in.' },
  { id: 'working', label: 'Working', meaning: 'Transcribing, then polishing. You can start the next dictation.' },
  { id: 'done', label: 'Pasted', meaning: 'A tick, then the panel goes away.' },
  { id: 'saved', label: 'Saved', meaning: 'Kept without text. Transcribe it from Home when a model is ready.' },
  { id: 'failed', label: 'Failed', meaning: 'Your audio is kept. Hover the panel for Retry.' },
  { id: 'waiting', label: 'One more', meaning: 'The badge counts dictations still finishing behind this one.' },
];

function Glyph({ id }: { id: string }) {
  if (id === 'listening') return <span className="legend-bars">{[0.4, 0.9, 0.6, 1, 0.5].map((h, i) => <i key={i} style={{ height: `${h * 100}%` }} />)}</span>;
  if (id === 'opening') return <span className="legend-ring" />;
  if (id === 'working') return <span className="legend-dots"><i /><i /><i /></span>;
  if (id === 'done') return <Check size={14} strokeWidth={2.6} />;
  if (id === 'saved') return <span className="legend-saved">Saved</span>;
  if (id === 'failed') return <span className="legend-failed"><i /> Failed</span>;
  return <span className="legend-waiting"><span className="legend-dots"><i /><i /><i /></span><b>+1</b></span>;
}

export function PanelLegend() {
  return (
    <section className="settings-group panel-legend" aria-label="What the panel tells you">
      <h3>What the panel tells you</h3>
      <ul>
        {STATES.map((state) => (
          <li key={state.id}>
            <span className={`legend-pill ${state.id}`} aria-hidden><Glyph id={state.id} /></span>
            <span><strong>{state.label}</strong>{state.meaning}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
