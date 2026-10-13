import { Keyboard, Mic } from 'lucide-react';
import { Keys } from '../shell/Keys';

const ART = [30, 55, 80, 110, 70, 95, 120, 85, 60, 100, 72, 48, 30];

/**
 * Home before the first dictation: one sentence about what the app does, the
 * real shortcut as keycaps, and two ways to act. It disappears for good once
 * there is anything in history.
 */
export function EmptyHome({ shortcut, toggle, integrated, canStart, startBlocked, onStart, onShortcut, setup }: {
  shortcut: string | null;
  toggle: boolean;
  integrated: boolean;
  canStart: boolean;
  startBlocked?: string;
  onStart: () => void;
  onShortcut?: () => void;
  /** What the next dictation will use, in a few words. */
  setup: string;
}) {
  return (
    <div className="empty-hero">
      <div className="empty-hero-inner">
        <div className="empty-art" aria-hidden>
          {ART.map((height, index) => <i key={index} style={{ height, animationDelay: `${index * 0.12}s` }} />)}
        </div>
        <h1>Speak anywhere. It’s typed.</h1>
        <p>
          {shortcut
            ? toggle
              ? <>Press <Keys chord={shortcut} large /> in any app, say what you want to write, and press it again.</>
              : <>Hold <Keys chord={shortcut} large /> in any app, say what you want to write, and let go.</>
            : <>Press the button, say what you want to write, and press it again.</>}
          {' '}
          {integrated ? 'Your words land where your cursor is — and every dictation is kept here.' : 'Every dictation is kept here, ready to copy.'}
        </p>
        <div className="empty-actions">
          <button type="button" className="button primary large" onClick={onStart} disabled={!canStart} title={canStart ? undefined : startBlocked}>
            <Mic size={18} aria-hidden /> Try a dictation here
          </button>
          {onShortcut && (
            <button type="button" className="button large" onClick={onShortcut}>
              <Keyboard size={18} aria-hidden /> Change the shortcut
            </button>
          )}
        </div>
        <p className="empty-setup"><span className="badge">{setup}</span></p>
      </div>
    </div>
  );
}
