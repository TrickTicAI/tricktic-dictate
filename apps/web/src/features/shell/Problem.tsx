import { AlertTriangle, RefreshCw } from 'lucide-react';
import type { ReactNode } from 'react';

/**
 * One way to report that something did not load.
 *
 * Before this, a failed load reported itself three different ways: Modes showed
 * a bare grey line and no way forward, Dictionary showed an alert and then drew
 * an empty table underneath it as if the list were simply empty, and the
 * transcript reader got it right. A panel that cannot load is not the same
 * thing as a panel with nothing in it, and the difference has to be visible.
 *
 * `retry` is the way out. `keeps` is what is still true despite the failure —
 * in this application that is almost always the saved audio, and saying so is
 * the difference between a setback and an apparent data loss.
 */
export function Problem({ title, detail, keeps, onRetry, retryLabel = 'Retry' }: {
  title: string;
  detail?: ReactNode;
  keeps?: ReactNode;
  onRetry?: () => void;
  retryLabel?: string;
}) {
  return (
    <div className="problem" role="alert">
      <AlertTriangle size={16} aria-hidden />
      <div className="problem-text">
        <strong>{title}</strong>
        {detail && <p>{detail}</p>}
        {keeps && <p className="problem-keeps">{keeps}</p>}
      </div>
      {onRetry && (
        <button type="button" className="button" onClick={onRetry}>
          <RefreshCw size={13} aria-hidden /> {retryLabel}
        </button>
      )}
    </div>
  );
}
