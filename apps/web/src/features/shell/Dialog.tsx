import { useEffect, useRef, type ReactNode } from 'react';
import { X } from 'lucide-react';

/**
 * One small modal for one decision.
 *
 * Settings is a large dialog with its own navigation; this is the other kind —
 * a short form that belongs to a page and closes as soon as it is answered. It
 * holds the keyboard the same way Settings does, because a dialog that can be
 * tabbed out of is a dialog the rest of the page is still competing with, and
 * it hands focus back to whatever opened it so the trigger can be pressed again
 * without reaching for the pointer.
 */
export function Dialog({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const panel = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;

  useEffect(() => {
    const opener = document.activeElement;
    panel.current?.querySelector<HTMLElement>('input, textarea, select, button')?.focus();
    return () => { if (opener instanceof HTMLElement) opener.focus(); };
  }, []);

  useEffect(() => {
    function key(event: KeyboardEvent) {
      if (event.key === 'Escape') { event.stopPropagation(); event.preventDefault(); close.current(); return; }
      if (event.key !== 'Tab' || !panel.current) return;
      const focusable = [...panel.current.querySelectorAll<HTMLElement>(
        'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      )].filter((element) => element.offsetParent !== null);
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (event.shiftKey && active === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && active === last) { event.preventDefault(); first.focus(); }
      else if (active instanceof HTMLElement && !panel.current.contains(active)) { event.preventDefault(); first.focus(); }
    }
    // Capture, so a surface underneath does not also act on the same Escape.
    window.addEventListener('keydown', key, true);
    return () => window.removeEventListener('keydown', key, true);
  }, []);

  return (
    <div className="dialog-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <div className="dialog" role="dialog" aria-modal="true" aria-label={title} ref={panel}>
        <div className="dialog-head">
          <h2>{title}</h2>
          <button type="button" className="icon-button" aria-label="Close" onClick={onClose}>
            <X size={15} />
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}
